//! `textDocument/completion` handling for Tarn `.tarn.yaml` buffers.
//!
//! The completion provider answers four distinct context questions,
//! modelled directly on the VS Code extension's
//! `editors/vscode/src/language/CompletionProvider.ts`:
//!
//!   1. Inside `{{ env. }}` → offer every resolved env key, each with
//!      its resolved value as `detail`, sorted by resolution priority
//!      via [`CompletionItem::sort_text`].
//!   2. Inside `{{ capture. }}` → offer every capture declared by a
//!      strictly earlier step visible from the cursor (same rules as
//!      the hover provider's `collect_visible_captures`).
//!   3. Inside `{{ $... }}` → offer the five Tarn built-ins. Each
//!      function is a `Snippet`-flavoured item with tabstops for the
//!      arguments where applicable.
//!   4. On a blank YAML mapping-key line → offer the schema-valid
//!      top-level / test-group / step keys from
//!      `schemas/v1/testfile.json`.
//!
//! The module follows the NAZ-292 hover pattern exactly:
//!
//!   * [`resolve_completion_context`] is a pure function. It takes
//!     `&str + Position` and returns `Option<CompletionContext>`. No
//!     filesystem, no parser, no LSP types beyond [`Position`].
//!   * The per-context list builders (`env_completions`,
//!     `capture_completions`, `builtin_completions`,
//!     `schema_key_completions`) are pure functions over
//!     pre-computed inputs. Every completion-item field is unit-tested
//!     against those inputs.
//!   * [`text_document_completion`] is the thin I/O wrapper that
//!     reads the document from [`DocumentStore`], builds the inputs
//!     once, resolves the context, and dispatches to the right list
//!     builder.
//!
//! ## Nested-object completion
//!
//! Nested field completion inside `request.*` / `assert.body.*` is
//! explicitly out of scope for L1.4 — see `docs/TARN_LSP.md` L1.4 for
//! the followup pointer. The `YamlKey` classifier only fires on
//! root / test-group / step blank lines today.

use std::collections::BTreeMap;
use std::path::Path;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, Documentation, InsertTextFormat,
    MarkupContent, MarkupKind, Position, Url,
};
use tarn::env::{self, EnvEntry, EnvSource};
use tarn::parser;

use crate::hover::{collect_visible_captures, CapturePhase, VisibleCapture, BUILTIN_DOCS};
use crate::schema::{schema_key_cache, SchemaKeyCache};
use crate::server::DocumentStore;
use crate::token::{column_to_line_byte_offset, line_at_position};

/// Classification of the cursor's *completion* context.
///
/// Completion context is meaningfully different from hover context:
/// hover asks "is the cursor *inside* a closed interpolation?" while
/// completion asks "what identifier is the user mid-way through
/// typing?". A half-finished `{{ env.b` is a completion site even
/// though there is no matching `}}` yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionContext {
    /// Inside an open interpolation `{{ ... }}` with one of four
    /// sub-scopes. See [`InterpolationScope`].
    InsideInterpolation(InterpolationScope),
    /// On a blank YAML mapping-key line at the root, inside a test
    /// group, or inside a step. See [`YamlScope`].
    YamlKey(YamlScope),
}

/// The thing the user is typing inside an open `{{ … }}` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterpolationScope {
    /// The expression so far is empty (`{{`, `{{ `, `{{  `). Offer the
    /// three roots: `env.`, `capture.`, `$…`.
    Empty,
    /// `{{ env.<prefix>` — offer env keys.
    Env,
    /// `{{ capture.<prefix>` — offer captures visible from cursor.
    Capture,
    /// `{{ $<prefix>` — offer built-in functions.
    Builtin,
}

/// Which YAML scope a blank mapping-key line sits in.
///
/// The classifier picks this from the line's leading indentation and
/// the surrounding structural keys (`tests:`, `steps:`, `setup:`,
/// `teardown:`). It is deliberately conservative: when the scope
/// cannot be pinned down unambiguously we return `None` rather than
/// guess — a wrong completion list is worse than no completion list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YamlScope {
    /// Top-level mapping of the test file.
    Root,
    /// Inside a named `tests.<name>` mapping.
    Test,
    /// Inside a single step (under `setup:`, `teardown:`, `steps:`,
    /// or `tests.<name>.steps`).
    Step,
}

/// Classify the cursor at `position` into a [`CompletionContext`], or
/// `None` when no sensible completion applies. Pure: no filesystem,
/// no parser.
///
/// Order of precedence:
///
///   1. Interpolation context. If the line up to the cursor contains
///      an *unclosed* `{{`, we're inside an interpolation — classify
///      as one of the four interpolation sub-scopes.
///   2. Otherwise, if the line is empty or contains only leading
///      whitespace, treat it as a blank mapping-key line and classify
///      the enclosing YAML scope.
///   3. Otherwise, return `None`.
pub fn resolve_completion_context(source: &str, position: Position) -> Option<CompletionContext> {
    let (_line_start, line) = line_at_position(source, position)?;
    let col_bytes = column_to_line_byte_offset(line, position.character);
    let prefix = &line[..col_bytes];

    if let Some(scope) = detect_interpolation_scope(prefix) {
        return Some(CompletionContext::InsideInterpolation(scope));
    }

    if is_blank_prefix(prefix) {
        if let Some(scope) = detect_yaml_scope(source, position) {
            return Some(CompletionContext::YamlKey(scope));
        }
    }

    None
}

/// Walk `prefix` (the text on the current line up to the cursor) and
/// decide whether the cursor is inside an open `{{ … }}` pair and
/// which sub-scope applies.
///
/// This mirrors `detectInterpolationContext` in the VS Code provider
/// line-for-line so both editors behave identically.
fn detect_interpolation_scope(prefix: &str) -> Option<InterpolationScope> {
    let open_idx = prefix.rfind("{{")?;
    // If the prefix already contains a `}}` at or after that `{{`, the
    // token is closed — we are no longer "inside" it.
    if prefix[open_idx..].contains("}}") {
        return None;
    }
    let expr = prefix[open_idx + 2..].trim_start();

    if expr.is_empty() {
        return Some(InterpolationScope::Empty);
    }
    if expr == "env" || expr.starts_with("env.") {
        return Some(InterpolationScope::Env);
    }
    if expr == "capture" || expr.starts_with("capture.") {
        return Some(InterpolationScope::Capture);
    }
    if expr.starts_with('$') {
        return Some(InterpolationScope::Builtin);
    }
    None
}

/// True when `prefix` has nothing but whitespace. Blank-prefix lines
/// are where the YAML schema-key classifier fires.
fn is_blank_prefix(prefix: &str) -> bool {
    prefix.chars().all(char::is_whitespace)
}

/// Decide which YAML scope the cursor line lives in.
///
/// This classifier is intentionally lightweight: it walks backward
/// from `position.line` looking at each prior non-blank line to find
/// the nearest structural anchor (`tests:`, `steps:`, `setup:`,
/// `teardown:`, or the document root). The nearest anchor and the
/// cursor's indentation together pick the scope.
fn detect_yaml_scope(source: &str, position: Position) -> Option<YamlScope> {
    let (_line_start, cursor_line_text) = line_at_position(source, position)?;
    let cursor_indent = leading_spaces(cursor_line_text);
    let col = column_to_line_byte_offset(cursor_line_text, position.character);
    // The cursor must sit at or past the current indent — YAML keys
    // begin at their indentation level, not before it.
    if col < cursor_indent {
        return None;
    }

    // At the very top of the document with zero indent, we're at root.
    if cursor_indent == 0 && position.line == 0 {
        return Some(YamlScope::Root);
    }

    // Walk back through previous non-blank lines.
    let mut current_line = position.line as i64 - 1;
    let mut saw_steps_key: Option<usize> = None;
    let mut saw_tests_key: Option<usize> = None;

    while current_line >= 0 {
        let (_, text) = match line_at_position(source, Position::new(current_line as u32, 0)) {
            Some(pair) => pair,
            None => break,
        };
        current_line -= 1;

        if text.trim().is_empty() || text.trim_start().starts_with('#') {
            continue;
        }

        let indent = leading_spaces(text);
        // Strictly shallower lines redefine the context; equal-indent
        // siblings do not. YAML blocks are indent-scoped.
        if indent < cursor_indent {
            let key = mapping_key(text);
            match (key, indent) {
                // A bare top-level `tests:` mapping means we're inside
                // a test group IFF the cursor is exactly one step
                // deeper than the named test key.
                (Some("tests"), 0) => {
                    saw_tests_key = Some(indent);
                }
                (Some("setup"), _) | (Some("teardown"), _) => {
                    // These are step-array parents. Cursor is a step.
                    return Some(YamlScope::Step);
                }
                (Some("steps"), _) => {
                    saw_steps_key = Some(indent);
                }
                (Some(_), 0) => {
                    // A sibling root key. Cursor indent > 0 but the
                    // parent is the document root — ambiguous, bail.
                    if saw_steps_key.is_some() {
                        return Some(YamlScope::Step);
                    }
                    if saw_tests_key.is_some() {
                        // We passed through `tests:` but never found
                        // `steps:` under it — the cursor is inside a
                        // test group but not in its steps array.
                        return Some(YamlScope::Test);
                    }
                    return None;
                }
                _ => {}
            }
        }
    }

    // Fell off the top of the document. If we saw structural anchors
    // on the way up, use them; otherwise the cursor is at the root.
    if saw_steps_key.is_some() {
        return Some(YamlScope::Step);
    }
    if saw_tests_key.is_some() {
        return Some(YamlScope::Test);
    }
    if cursor_indent == 0 {
        Some(YamlScope::Root)
    } else {
        None
    }
}

/// Leading-space count for a line.
fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

/// If `line` is `<indent><key>:...` return `Some(<key>)`; otherwise
/// `None`. The check is conservative enough to skip list items
/// (`- name: ...`) and quoted strings.
fn mapping_key(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('-') {
        // List item. Could still declare a key after the `-`, but we
        // don't rely on that for scope detection — the `-` line is a
        // step anchor either way.
        return None;
    }
    let colon_pos = trimmed.find(':')?;
    let key = trimmed[..colon_pos].trim_end();
    if key.is_empty() {
        return None;
    }
    // A key must start with an identifier-ish character.
    let first = key.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    Some(key)
}

// ---------------------------------------------------------------------
// Per-context list builders
// ---------------------------------------------------------------------

/// Build completion items for every resolved environment variable.
///
/// Items are sorted by resolution priority (CLI > shell > local >
/// named > default > inline) via `CompletionItem::sort_text`. The
/// client receives them in alphabetical order inside each priority
/// bucket, so a priority-0 env key appears above a priority-5 key
/// with the same leading letters.
pub fn env_completions(env: &BTreeMap<String, EnvEntry>) -> Vec<CompletionItem> {
    let mut items = Vec::with_capacity(env.len());
    for (key, entry) in env {
        let priority = env_source_priority(&entry.source);
        items.push(CompletionItem {
            label: key.clone(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: Some(entry.value.clone()),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!(
                    "Source: {}\n\nResolved value: `{}`",
                    entry.source.label(),
                    entry.value
                ),
            })),
            insert_text: Some(key.clone()),
            sort_text: Some(format!("{priority}_{key}")),
            ..Default::default()
        });
    }
    items
}

/// Resolution-priority rank for `EnvSource`. Lower = higher priority
/// in completion ordering, matching the runtime precedence chain in
/// `tarn::env::resolve_env_with_sources`.
fn env_source_priority(source: &EnvSource) -> u8 {
    match source {
        EnvSource::CliVar => 0,
        EnvSource::LocalEnvFile { .. } => 1,
        EnvSource::NamedEnvFile { .. } | EnvSource::NamedProfileVars { .. } => 2,
        EnvSource::DefaultEnvFile { .. } => 3,
        EnvSource::InlineEnvBlock => 4,
    }
}

/// Build completion items for every capture visible from the cursor.
///
/// Later declarations win when two earlier steps shadow the same
/// name, matching the runtime merge behaviour in `tarn::runner`.
pub fn capture_completions(captures: &[VisibleCapture]) -> Vec<CompletionItem> {
    let mut seen: std::collections::BTreeMap<String, &VisibleCapture> =
        std::collections::BTreeMap::new();
    for cap in captures {
        seen.insert(cap.name.clone(), cap);
    }
    seen.into_iter()
        .map(|(name, cap)| {
            let scope_label = match (&cap.phase, &cap.test_name) {
                (CapturePhase::Setup, _) => "setup".to_owned(),
                (CapturePhase::Teardown, _) => "teardown".to_owned(),
                (CapturePhase::FlatSteps, _) => "this file".to_owned(),
                (CapturePhase::Test, Some(name)) => format!("test `{name}`"),
                (CapturePhase::Test, None) => "this file".to_owned(),
            };
            CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some(format!("capture from {scope_label}")),
                documentation: Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!(
                        "Set by step `{}` (index {}). Source: {}",
                        cap.step_name, cap.step_index, cap.source
                    ),
                })),
                insert_text: Some(name.clone()),
                ..Default::default()
            }
        })
        .collect()
}

/// Build completion items for every Tarn built-in function.
///
/// Each item has an `insert_text` snippet with tabstops for function
/// arguments. The VS Code provider in
/// `editors/vscode/src/language/CompletionProvider.ts` is the source
/// of truth for the exact insertText strings — we keep them in sync
/// so a single doc-site examples block applies to both editors.
pub fn builtin_completions() -> Vec<CompletionItem> {
    vec![
        builtin_item("uuid", "uuid", "$uuid", "Generate a UUID v4.", false),
        builtin_item(
            "timestamp",
            "timestamp",
            "$timestamp",
            "Current Unix timestamp (seconds).",
            false,
        ),
        builtin_item(
            "now_iso",
            "now_iso",
            "$now_iso",
            "Current time as ISO 8601.",
            false,
        ),
        builtin_item(
            "random_hex",
            "random_hex(${1:8})",
            "$random_hex(n)",
            "Generate `n` random hex characters.",
            true,
        ),
        builtin_item(
            "random_int",
            "random_int(${1:min}, ${2:max})",
            "$random_int(min, max)",
            "Random integer in `[min, max]` inclusive.",
            true,
        ),
    ]
}

fn builtin_item(
    label: &str,
    insert_text: &str,
    detail: &str,
    doc: &str,
    is_snippet: bool,
) -> CompletionItem {
    CompletionItem {
        label: label.to_owned(),
        kind: Some(CompletionItemKind::FUNCTION),
        detail: Some(detail.to_owned()),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: doc.to_owned(),
        })),
        insert_text: Some(insert_text.to_owned()),
        insert_text_format: Some(if is_snippet {
            InsertTextFormat::SNIPPET
        } else {
            InsertTextFormat::PLAIN_TEXT
        }),
        ..Default::default()
    }
}

/// Build completion items for a blank YAML mapping-key line.
///
/// Which set of keys to offer depends on which scope the classifier
/// reported. Every item uses `CompletionItemKind::Property` and pulls
/// the description out of the shared schema cache. The insert text
/// includes a trailing `: ` so the user can keep typing the value
/// without an extra keystroke, matching how modern YAML LSPs behave.
pub fn schema_key_completions(scope: YamlScope, cache: &SchemaKeyCache) -> Vec<CompletionItem> {
    let keys = match scope {
        YamlScope::Root => cache.root_keys(),
        YamlScope::Test => cache.test_keys(),
        YamlScope::Step => cache.step_keys(),
    };
    keys.iter()
        .map(|key| {
            let description = cache
                .description(key)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Tarn test file field `{key}`"));
            CompletionItem {
                label: (*key).to_owned(),
                kind: Some(CompletionItemKind::PROPERTY),
                detail: Some(format!("Tarn {} key", scope_label(scope))),
                documentation: Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: description,
                })),
                insert_text: Some(format!("{key}: ")),
                ..Default::default()
            }
        })
        .collect()
}

fn scope_label(scope: YamlScope) -> &'static str {
    match scope {
        YamlScope::Root => "root",
        YamlScope::Test => "test",
        YamlScope::Step => "step",
    }
}

/// Completion items to offer when the cursor is at an empty `{{ }}`
/// with no prefix. Matches the VS Code provider's behaviour: three
/// headline items (`env`, `capture`, `$uuid`) so the user learns
/// what's available.
fn empty_interpolation_completions() -> Vec<CompletionItem> {
    vec![
        CompletionItem {
            label: "env".to_owned(),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some("Environment variable".to_owned()),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "Expands to a key from the merged env resolution chain.".to_owned(),
            })),
            insert_text: Some("env.".to_owned()),
            ..Default::default()
        },
        CompletionItem {
            label: "capture".to_owned(),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some("Captured variable".to_owned()),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "Expands to a value captured by a prior step in the same test.".to_owned(),
            })),
            insert_text: Some("capture.".to_owned()),
            ..Default::default()
        },
        CompletionItem {
            label: "$uuid".to_owned(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some("Built-in function".to_owned()),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: BUILTIN_DOCS[0].doc.to_owned(),
            })),
            insert_text: Some("$uuid".to_owned()),
            ..Default::default()
        },
    ]
}

// ---------------------------------------------------------------------
// Request handler
// ---------------------------------------------------------------------

/// The `textDocument/completion` request entry point.
///
/// Returns `None` when the cursor does not resolve to any completion
/// context we know how to answer. LSP clients serialise `None` as
/// JSON `null`, which every client tolerates (unlike an empty
/// `CompletionList` which some clients cache as "nothing to offer"
/// for the rest of the session).
pub fn text_document_completion(
    store: &DocumentStore,
    uri: &Url,
    position: Position,
) -> Option<CompletionResponse> {
    let source = store.get(uri)?;
    let ctx = resolve_completion_context(source, position)?;

    let items = match ctx {
        CompletionContext::InsideInterpolation(InterpolationScope::Empty) => {
            empty_interpolation_completions()
        }
        CompletionContext::InsideInterpolation(InterpolationScope::Env) => {
            let env = build_env_context(source, uri);
            env_completions(&env)
        }
        CompletionContext::InsideInterpolation(InterpolationScope::Capture) => {
            let caps = build_captures_context(source, uri, position);
            capture_completions(&caps)
        }
        CompletionContext::InsideInterpolation(InterpolationScope::Builtin) => {
            builtin_completions()
        }
        CompletionContext::YamlKey(scope) => schema_key_completions(scope, schema_key_cache()),
    };

    if items.is_empty() {
        return None;
    }
    Some(CompletionResponse::Array(items))
}

/// Resolve the env chain for the document at `uri`. Matches how the
/// hover provider builds its env context — inline env block from the
/// file when it parses, merged with every file-based layer.
///
/// When the document fails to parse — which happens constantly during
/// a live edit session — we fall back to an empty inline env and let
/// the file-based layers contribute what they can. This mirrors how
/// the hover provider degrades gracefully under parse error.
fn build_env_context(source: &str, uri: &Url) -> BTreeMap<String, EnvEntry> {
    let path = uri_to_path(uri);
    let inline_env = parser::parse_str(source, &path)
        .map(|tf| tf.env)
        .unwrap_or_else(|_| inline_env_from_raw_yaml(source).unwrap_or_default());
    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    env::resolve_env_with_sources(
        &inline_env,
        None,
        &[],
        &base_dir,
        "tarn.env.yaml",
        &std::collections::HashMap::new(),
    )
    .unwrap_or_default()
}

/// Parse only the top-level `env:` mapping out of a YAML document.
///
/// The full parser is aggressive: a single unknown field elsewhere in
/// the file turns the whole parse into an `Err`, and completion needs
/// to keep working while the user is mid-edit. This helper runs a
/// permissive raw `serde_yaml` parse and extracts just the inline env
/// block, so a mistyped `body:` under a step (or any other transient
/// shape error) cannot blank out env completions.
fn inline_env_from_raw_yaml(source: &str) -> Option<std::collections::HashMap<String, String>> {
    let value: serde_yaml::Value = serde_yaml::from_str(source).ok()?;
    let env_value = value.get("env")?;
    let mapping = env_value.as_mapping()?;
    let mut out = std::collections::HashMap::new();
    for (k, v) in mapping {
        let key = k.as_str()?.to_owned();
        let val = match v {
            serde_yaml::Value::String(s) => s.clone(),
            serde_yaml::Value::Number(n) => n.to_string(),
            serde_yaml::Value::Bool(b) => b.to_string(),
            _ => continue,
        };
        out.insert(key, val);
    }
    Some(out)
}

/// Build the captures-in-scope list for the cursor position.
fn build_captures_context(source: &str, uri: &Url, position: Position) -> Vec<VisibleCapture> {
    let path = uri_to_path(uri);
    let Ok(test_file) = parser::parse_str(source, &path) else {
        return Vec::new();
    };
    let cursor_line_one_based = (position.line as usize) + 1;
    collect_visible_captures(&test_file, cursor_line_one_based)
}

fn uri_to_path(uri: &Url) -> std::path::PathBuf {
    uri.to_file_path()
        .unwrap_or_else(|_| std::path::PathBuf::from(uri.path()))
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -------- resolve_completion_context: interpolation scopes ---------

    fn ctx(source: &str, line: u32, col: u32) -> Option<CompletionContext> {
        resolve_completion_context(source, Position::new(line, col))
    }

    #[test]
    fn cursor_after_double_open_brace_is_empty_interpolation() {
        let src = "url: {{ \n";
        assert_eq!(
            ctx(src, 0, 8),
            Some(CompletionContext::InsideInterpolation(
                InterpolationScope::Empty
            ))
        );
    }

    #[test]
    fn cursor_after_env_dot_is_env_scope() {
        let src = "url: {{ env. \n";
        assert_eq!(
            ctx(src, 0, 12),
            Some(CompletionContext::InsideInterpolation(
                InterpolationScope::Env
            ))
        );
    }

    #[test]
    fn cursor_after_capture_dot_is_capture_scope() {
        let src = "url: {{ capture. \n";
        assert_eq!(
            ctx(src, 0, 16),
            Some(CompletionContext::InsideInterpolation(
                InterpolationScope::Capture
            ))
        );
    }

    #[test]
    fn cursor_after_dollar_sign_is_builtin_scope() {
        let src = "id: {{ $ \n";
        assert_eq!(
            ctx(src, 0, 8),
            Some(CompletionContext::InsideInterpolation(
                InterpolationScope::Builtin
            ))
        );
    }

    #[test]
    fn cursor_inside_partial_env_key_is_env_scope() {
        let src = "url: {{ env.base\n";
        assert_eq!(
            ctx(src, 0, 16),
            Some(CompletionContext::InsideInterpolation(
                InterpolationScope::Env
            ))
        );
    }

    #[test]
    fn cursor_after_env_dot_before_closing_braces_on_same_line() {
        // Regression test: `{{ env. }}/seed` — the closing `}}` exists
        // later on the same line, but the prefix up to the cursor ends
        // at the `.`, so the classifier must still see an open
        // interpolation.
        let src = "      url: \"{{ env. }}/seed\"";
        // Cursor right after the `.`.
        let dot_col = src.find("env.").unwrap() + 4;
        assert_eq!(
            ctx(src, 0, dot_col as u32),
            Some(CompletionContext::InsideInterpolation(
                InterpolationScope::Env
            ))
        );
    }

    #[test]
    fn cursor_after_closed_interpolation_is_not_interpolation() {
        let src = "url: {{ env.x }} more";
        // Cursor on `more` after the `}}` — no open interpolation.
        assert_eq!(ctx(src, 0, 19), None);
    }

    #[test]
    fn cursor_in_plain_value_returns_none() {
        let src = "url: plain-text\n";
        assert_eq!(ctx(src, 0, 10), None);
    }

    #[test]
    fn cursor_in_unclosed_interpolation_on_same_line_is_open() {
        let src = "url: {{ env.a\n";
        // Mid identifier.
        assert_eq!(
            ctx(src, 0, 13),
            Some(CompletionContext::InsideInterpolation(
                InterpolationScope::Env
            ))
        );
    }

    // -------- resolve_completion_context: YAML scopes ----------

    #[test]
    fn blank_line_at_document_root_is_root_scope() {
        let src = "name: x\n\nenv:\n  x: y\n";
        // Line 1 is blank, column 0.
        assert_eq!(
            ctx(src, 1, 0),
            Some(CompletionContext::YamlKey(YamlScope::Root))
        );
    }

    #[test]
    fn blank_line_inside_step_is_step_scope() {
        let src = "steps:\n  - name: s\n    \n";
        // Line 2 is `    ` (4 spaces). cursor col 4.
        assert_eq!(
            ctx(src, 2, 4),
            Some(CompletionContext::YamlKey(YamlScope::Step))
        );
    }

    #[test]
    fn blank_line_inside_named_test_group_is_test_scope() {
        let src = "tests:\n  auth:\n    \n";
        // Line 2 is `    ` (4 spaces). Cursor col 4.
        assert_eq!(
            ctx(src, 2, 4),
            Some(CompletionContext::YamlKey(YamlScope::Test))
        );
    }

    #[test]
    fn blank_line_inside_setup_step_is_step_scope() {
        let src = "setup:\n  - name: s\n    \n";
        assert_eq!(
            ctx(src, 2, 4),
            Some(CompletionContext::YamlKey(YamlScope::Step))
        );
    }

    #[test]
    fn blank_line_inside_teardown_step_is_step_scope() {
        let src = "teardown:\n  - name: t\n    \n";
        assert_eq!(
            ctx(src, 2, 4),
            Some(CompletionContext::YamlKey(YamlScope::Step))
        );
    }

    #[test]
    fn cursor_on_partially_typed_key_returns_none() {
        // A half-typed key like `nam` at col 3 isn't blank, so the
        // classifier bails rather than guessing which scope.
        let src = "nam";
        assert_eq!(ctx(src, 0, 3), None);
    }

    // -------- env_completions ----------

    fn entry(value: &str, source: EnvSource) -> EnvEntry {
        EnvEntry {
            value: value.to_owned(),
            source,
            declaration_range: None,
        }
    }

    #[test]
    fn env_completions_emit_variable_kind_with_detail_and_sort_text() {
        let mut env: BTreeMap<String, EnvEntry> = BTreeMap::new();
        env.insert(
            "base_url".to_owned(),
            entry(
                "http://localhost:3000",
                EnvSource::DefaultEnvFile {
                    path: "/p/tarn.env.yaml".to_owned(),
                },
            ),
        );
        let items = env_completions(&env);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.label, "base_url");
        assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));
        assert_eq!(item.detail.as_deref(), Some("http://localhost:3000"));
        assert_eq!(item.insert_text.as_deref(), Some("base_url"));
        // DefaultEnvFile maps to priority 3.
        assert_eq!(item.sort_text.as_deref(), Some("3_base_url"));
    }

    #[test]
    fn env_completions_sort_by_resolution_priority_across_sources() {
        let mut env: BTreeMap<String, EnvEntry> = BTreeMap::new();
        env.insert("cli_key".to_owned(), entry("from-cli", EnvSource::CliVar));
        env.insert(
            "inline_key".to_owned(),
            entry("from-inline", EnvSource::InlineEnvBlock),
        );
        env.insert(
            "default_key".to_owned(),
            entry(
                "from-default",
                EnvSource::DefaultEnvFile {
                    path: "/p/tarn.env.yaml".to_owned(),
                },
            ),
        );
        let items = env_completions(&env);
        // Collect sort_text in whatever order we emitted them (BTreeMap
        // iteration order is alphabetical) and check priority prefixes.
        let labels_and_sort: Vec<_> = items
            .iter()
            .map(|i| (i.label.clone(), i.sort_text.clone().unwrap()))
            .collect();
        // Verify each priority prefix is correct.
        let prefix_of = |label: &str| {
            labels_and_sort
                .iter()
                .find(|(l, _)| l == label)
                .map(|(_, s)| s.chars().next().unwrap())
                .unwrap()
        };
        assert_eq!(prefix_of("cli_key"), '0');
        assert_eq!(prefix_of("default_key"), '3');
        assert_eq!(prefix_of("inline_key"), '4');
    }

    // -------- capture_completions ----------

    fn cap(name: &str, phase: CapturePhase, test_name: Option<&str>) -> VisibleCapture {
        VisibleCapture {
            name: name.to_owned(),
            step_name: "s".to_owned(),
            step_index: 0,
            phase,
            test_name: test_name.map(ToOwned::to_owned),
            source: "JSONPath `$.id`".to_owned(),
        }
    }

    #[test]
    fn capture_completions_emit_variable_kind_with_scope_detail() {
        let caps = vec![cap("token", CapturePhase::Test, Some("auth"))];
        let items = capture_completions(&caps);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.label, "token");
        assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));
        assert!(item.detail.as_ref().unwrap().contains("test `auth`"));
        assert_eq!(item.insert_text.as_deref(), Some("token"));
    }

    #[test]
    fn capture_completions_dedupe_later_declarations_override_earlier() {
        let caps = vec![
            cap("token", CapturePhase::Setup, None),
            cap("token", CapturePhase::Test, Some("auth")),
        ];
        let items = capture_completions(&caps);
        assert_eq!(items.len(), 1);
        // The test-scope entry should win (inserted last).
        assert!(items[0].detail.as_ref().unwrap().contains("test `auth`"));
    }

    #[test]
    fn capture_completions_empty_input_yields_empty_list() {
        let items = capture_completions(&[]);
        assert!(items.is_empty());
    }

    // -------- builtin_completions ----------

    #[test]
    fn builtin_completions_emit_five_function_items() {
        let items = builtin_completions();
        assert_eq!(items.len(), 5);
        for item in &items {
            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert!(item.insert_text.is_some());
            assert!(item.detail.is_some());
        }
    }

    #[test]
    fn builtin_completions_random_hex_uses_snippet_insert_format() {
        let items = builtin_completions();
        let hex = items.iter().find(|i| i.label == "random_hex").unwrap();
        assert_eq!(hex.insert_text_format, Some(InsertTextFormat::SNIPPET));
        assert_eq!(hex.insert_text.as_deref(), Some("random_hex(${1:8})"));
        assert_eq!(hex.detail.as_deref(), Some("$random_hex(n)"));
    }

    #[test]
    fn builtin_completions_random_int_uses_two_snippet_placeholders() {
        let items = builtin_completions();
        let int = items.iter().find(|i| i.label == "random_int").unwrap();
        assert_eq!(
            int.insert_text.as_deref(),
            Some("random_int(${1:min}, ${2:max})")
        );
        assert_eq!(int.insert_text_format, Some(InsertTextFormat::SNIPPET));
    }

    #[test]
    fn builtin_completions_uuid_is_plain_text_not_snippet() {
        let items = builtin_completions();
        let uuid = items.iter().find(|i| i.label == "uuid").unwrap();
        assert_eq!(uuid.insert_text.as_deref(), Some("uuid"));
        assert_eq!(uuid.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
    }

    // -------- schema_key_completions ----------

    #[test]
    fn schema_key_completions_root_scope_includes_top_level_keys() {
        let cache = schema_key_cache();
        let items = schema_key_completions(YamlScope::Root, cache);
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"name"));
        assert!(labels.contains(&"env"));
        assert!(labels.contains(&"tests"));
        assert!(labels.contains(&"steps"));
        for item in &items {
            assert_eq!(item.kind, Some(CompletionItemKind::PROPERTY));
            assert!(item.insert_text.as_ref().unwrap().ends_with(": "));
        }
    }

    #[test]
    fn schema_key_completions_step_scope_includes_step_keys() {
        let cache = schema_key_cache();
        let items = schema_key_completions(YamlScope::Step, cache);
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"name"));
        assert!(labels.contains(&"request"));
        assert!(labels.contains(&"assert"));
        assert!(labels.contains(&"capture"));
    }

    #[test]
    fn schema_key_completions_test_scope_includes_test_keys() {
        let cache = schema_key_cache();
        let items = schema_key_completions(YamlScope::Test, cache);
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"steps"));
        assert!(labels.contains(&"description"));
    }

    // -------- empty_interpolation_completions ----------

    #[test]
    fn empty_interpolation_completions_offer_env_capture_uuid() {
        let items = empty_interpolation_completions();
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["env", "capture", "$uuid"]);
    }

    // -------- text_document_completion — direct DocumentStore calls ----------

    #[test]
    fn inline_env_from_raw_yaml_extracts_env_block_even_when_full_parse_fails() {
        // The full parser rejects `body:` as a step-level key, but the
        // permissive raw YAML parse still extracts the env block.
        let src = "name: x\nenv:\n  base_url: http://h\nsteps:\n  - name: s\n    request:\n      method: GET\n      url: /x\n    body:\n      broken: yes\n";
        let env = inline_env_from_raw_yaml(src).expect("raw YAML parse should succeed");
        assert_eq!(env.get("base_url").map(String::as_str), Some("http://h"));
    }

    #[test]
    fn text_document_completion_env_scope_returns_inline_env_keys() {
        let mut store = DocumentStore::new();
        let uri = Url::parse("file:///tmp/cmp-direct.tarn.yaml").unwrap();
        let src = "name: direct\nenv:\n  base_url: http://x\nsteps:\n  - name: s\n    request:\n      method: GET\n      url: \"{{ env. }}\"\n";
        store.open(uri.clone(), src.to_owned());
        // Cursor right after the `.` on line 7.
        let dot_col = src.lines().nth(7).unwrap().find("env.").unwrap() + 4;
        let response = text_document_completion(&store, &uri, Position::new(7, dot_col as u32))
            .expect("expected a completion response in env scope");
        let items = match response {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        };
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"base_url"),
            "expected base_url, got {labels:?}"
        );
    }
}
