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
//! ## Nested-object completion (new in L3.5)
//!
//! L3.5 adds schema-aware completion for cursors nested beyond the
//! top-level / test-group / step scopes. [`resolve_schema_path`]
//! walks the YAML source backward from the cursor and builds a
//! [`SchemaPath`] — a dot-path into the JSON Schema rooted at
//! `schemas/v1/testfile.json`. [`nested_schema_completions`] then
//! calls [`children_at_schema_path`] and emits one completion item
//! per valid child, using the schema's `description` field as the
//! item's documentation where available.
//!
//! The integration point is [`text_document_completion`]: when the
//! cursor is on a blank line (so the existing top-level / step
//! classifier would have fired), we also try the schema-path
//! resolver, and if the schema walker yields children for a path
//! deeper than the hard-coded top-level scopes we prefer those.
//! This keeps the original Root / Test / Step scopes (which are
//! themselves thin wrappers around hard-coded key lists) untouched
//! while adding nested support for `request.*`, `assert.*`,
//! `assert.body.*`, `capture.*`, `poll.*`, and every other nested
//! shape the schema describes.

use std::collections::BTreeMap;
use std::path::Path;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, Documentation, InsertTextFormat,
    MarkupContent, MarkupKind, Position, Url,
};
use tarn::env::{self, EnvEntry, EnvSource};
use tarn::parser;

use crate::hover::{collect_visible_captures, CapturePhase, VisibleCapture, BUILTIN_DOCS};
use crate::schema::{
    children_at_schema_path, schema_key_cache, PathSegment, SchemaField, SchemaFieldKind,
    SchemaKeyCache, SchemaPath,
};
use crate::server::{is_tarn_file_uri, DocumentStore};
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
// Nested schema-path completion (L3.5)
// ---------------------------------------------------------------------

/// Resolve a YAML cursor position to a [`SchemaPath`] — the sequence
/// of `properties` / `items` / `additionalProperties` steps the
/// completion walker needs to navigate to find valid children at
/// that point in the document.
///
/// Pure: no parser, no filesystem, no LSP types beyond [`Position`].
/// The walker is intentionally permissive — it works off raw lines
/// rather than a YAML parse tree, so half-finished documents (the
/// common case during live editing) still produce a usable path.
///
/// Returns `None` when the cursor line is not on a blank mapping-key
/// line (the only place nested completion makes sense). Returns
/// `Some(SchemaPath::default())` when the cursor is at the document
/// root — callers can still use the empty path, since
/// [`children_at_schema_path`] interprets an empty path as "root
/// top-level properties".
pub fn resolve_schema_path(source: &str, position: Position) -> Option<SchemaPath> {
    let (_line_start, cursor_line_text) = line_at_position(source, position)?;
    let col = column_to_line_byte_offset(cursor_line_text, position.character);
    let prefix = &cursor_line_text[..col];
    if !is_blank_prefix(prefix) {
        return None;
    }
    let cursor_indent = leading_spaces(cursor_line_text);
    // The cursor must sit at or past the current line's indent —
    // YAML keys begin at their indentation level, not before it.
    if col < cursor_indent {
        return None;
    }

    let mut target_indent = cursor_indent;
    let mut segments_rev: Vec<PathSegment> = Vec::new();

    let mut current_line = position.line as i64 - 1;
    while current_line >= 0 {
        let (_, text) = match line_at_position(source, Position::new(current_line as u32, 0)) {
            Some(pair) => pair,
            None => break,
        };
        current_line -= 1;

        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let indent = leading_spaces(text);
        if indent >= target_indent {
            // Siblings or deeper children — cannot contribute a
            // parent segment.
            continue;
        }

        // List-item marker. The `-` anchors an array element; the
        // element's interior begins at `indent + 2`, which is where
        // the first key (e.g. `name:`) lives on the same physical
        // line. The array segment's indent for the walker is the
        // `-` column, not the child column.
        let after_indent = &text[indent..];
        if after_indent.starts_with('-') {
            segments_rev.push(PathSegment::Index(0));
            target_indent = indent;
            if indent == 0 {
                // `-` at column 0 means we just crossed a top-level
                // sequence; the parent is the document root and we
                // still need to resolve the mapping key that wraps
                // this sequence. Keep walking.
                continue;
            }
            continue;
        }

        // Plain mapping key. Only lines of the form `<indent>key:`
        // (with optional trailing comment or empty value) contribute
        // a parent segment — a sibling line like `method: GET` on a
        // strictly shallower indent would be a sibling key, which is
        // impossible in a well-indented block so we defensively
        // skip it rather than push the wrong segment.
        if let Some(key) = mapping_key_for_nested(text) {
            segments_rev.push(PathSegment::Key(key));
            target_indent = indent;
            if indent == 0 {
                // Top-level key reached. Walk no further.
                break;
            }
            continue;
        }
        // Non-key, non-list line at a shallower indent — the YAML
        // is malformed in a way that doesn't help us. Skip and keep
        // looking for a structural anchor.
    }

    segments_rev.reverse();
    let mut path = SchemaPath::new();
    for seg in segments_rev {
        path.push(seg);
    }
    Some(path)
}

/// Build nested completion items from a resolved [`SchemaPath`].
///
/// Empty `Vec` when the path does not resolve to any schema node
/// (so callers can fall back to other completion sources). Items
/// are sorted alphabetically via `BTreeMap` inside
/// [`children_at_schema_path`]; we further sort matchers after
/// properties so the schema-proper fields appear first.
pub fn nested_schema_completions(path: &SchemaPath, cache: &SchemaKeyCache) -> Vec<CompletionItem> {
    let fields = children_at_schema_path(cache, path);
    fields.into_iter().map(schema_field_to_item).collect()
}

/// Convert a single [`SchemaField`] into a completion item, preserving
/// the schema description as documentation and tagging matchers with
/// a distinct detail string so the client can render them differently
/// from ordinary structural properties.
fn schema_field_to_item(field: SchemaField) -> CompletionItem {
    let detail = match field.kind {
        SchemaFieldKind::Property => "Tarn schema field",
        SchemaFieldKind::Matcher => "Tarn assertion matcher",
    };
    let documentation = field.description.map(|desc| {
        Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: desc,
        })
    });
    CompletionItem {
        label: field.name.clone(),
        kind: Some(CompletionItemKind::PROPERTY),
        detail: Some(detail.to_owned()),
        documentation,
        insert_text: Some(format!("{}: ", field.name)),
        ..Default::default()
    }
}

/// Mapping-key detector for the nested walker.
///
/// Accepts any `<indent>key:` or `<indent>"quoted key":` line. Stricter
/// than [`mapping_key`] because it must also accept quoted keys (e.g.
/// `"$.id":`) which the legacy top-level classifier never encountered.
fn mapping_key_for_nested(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('-') {
        return None;
    }

    // Quoted key: `"key":...` or `'key':...`.
    if let Some(rest) = trimmed.strip_prefix('"') {
        let end = rest.find('"')?;
        let key = &rest[..end];
        // After the closing quote we must find a `:` before any
        // other non-whitespace so we don't accidentally treat a
        // quoted scalar value as a key.
        let tail = &rest[end + 1..];
        let tail_trimmed = tail.trim_start();
        if !tail_trimmed.starts_with(':') {
            return None;
        }
        if key.is_empty() {
            return None;
        }
        return Some(key.to_owned());
    }
    if let Some(rest) = trimmed.strip_prefix('\'') {
        let end = rest.find('\'')?;
        let key = &rest[..end];
        let tail = &rest[end + 1..];
        let tail_trimmed = tail.trim_start();
        if !tail_trimmed.starts_with(':') {
            return None;
        }
        if key.is_empty() {
            return None;
        }
        return Some(key.to_owned());
    }

    // Unquoted key.
    let colon_pos = trimmed.find(':')?;
    let key = trimmed[..colon_pos].trim_end();
    if key.is_empty() {
        return None;
    }
    let first = key.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    // An unquoted mapping key can't contain whitespace in the middle.
    if key.chars().any(char::is_whitespace) {
        return None;
    }
    Some(key.to_owned())
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
    if !is_tarn_file_uri(uri) {
        return None;
    }
    let source = store.get(uri)?;

    // Interpolation contexts take precedence — they're the most
    // specific classifier and the top-level / nested YAML walkers
    // must not fire inside an open `{{ … }}`.
    if let Some(ctx) = resolve_completion_context(source, position) {
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
            CompletionContext::YamlKey(scope) => {
                // Top-level / test / step blank-line scopes keep their
                // hard-coded key lists (with full descriptions via
                // the schema cache). Only try the nested walker as a
                // fallback when the top-level classifier returned
                // `Step` but the cursor is actually deeper than the
                // step mapping itself.
                let top_items = schema_key_completions(scope, schema_key_cache());
                let nested_items = nested_schema_completions_at(source, position);
                let should_prefer_nested = matches!(scope, YamlScope::Step)
                    && !nested_items.is_empty()
                    && nested_path_depth(source, position) > 2;
                if should_prefer_nested {
                    nested_items
                } else {
                    top_items
                }
            }
        };
        if !items.is_empty() {
            return Some(CompletionResponse::Array(items));
        }
    }

    // Fallback: cursor is on a blank line the top-level classifier
    // did not recognise (e.g. inside `request:` three levels below
    // `steps:`). The nested walker is permissive — it'll happily
    // produce a schema path for these positions.
    let nested = nested_schema_completions_at(source, position);
    if !nested.is_empty() {
        return Some(CompletionResponse::Array(nested));
    }
    None
}

/// Run the nested walker for a cursor position and return the
/// completion items for its schema path. Thin helper to keep the
/// dispatcher body readable.
fn nested_schema_completions_at(source: &str, position: Position) -> Vec<CompletionItem> {
    let Some(path) = resolve_schema_path(source, position) else {
        return Vec::new();
    };
    if path.is_empty() {
        // Empty path = root. The existing Root scope already
        // renders root keys, so avoid double-emitting.
        return Vec::new();
    }
    nested_schema_completions(&path, schema_key_cache())
}

/// Depth of the resolved nested path. Used by the dispatcher to pick
/// between the legacy step-scope key list and the schema walker
/// output — a cursor inside `request.*` has a path of length 3
/// (`steps` → Index → `request`), strictly deeper than the step
/// mapping itself (length 2), so the walker owns it.
fn nested_path_depth(source: &str, position: Position) -> usize {
    resolve_schema_path(source, position).map_or(0, |p| p.len())
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

    // -------- resolve_schema_path ----------

    fn segs(path: &SchemaPath) -> Vec<String> {
        path.segments()
            .iter()
            .map(|seg| match seg {
                PathSegment::Key(k) => k.clone(),
                PathSegment::Index(i) => format!("[{i}]"),
            })
            .collect()
    }

    #[test]
    fn resolve_schema_path_inside_request_yields_steps_index_request() {
        // Cursor is on the blank line directly under `request:` — we
        // should get a path of `steps[0].request`.
        let src = "name: x\nsteps:\n  - name: s\n    request:\n      method: GET\n      \n";
        // Line 5 is `      ` (6 spaces). Cursor col 6.
        let path = resolve_schema_path(src, Position::new(5, 6)).expect("path");
        assert_eq!(segs(&path), vec!["steps", "[0]", "request"]);
    }

    #[test]
    fn resolve_schema_path_inside_assert_body_jsonpath_yields_matcher_path() {
        // Fixture: cursor inside `assert.body."$.id":` one level deeper
        // than the JSONPath key itself, so the matcher grammar applies.
        // The blank line carries explicit 10-space padding so the
        // classifier sees the expected indent — real YAML buffers
        // midway through an edit always have at least the
        // auto-indentation whitespace in place.
        let src = concat!(
            "name: x\n",
            "steps:\n",
            "  - name: s\n",
            "    assert:\n",
            "      body:\n",
            "        \"$.id\":\n",
            "          \n",
        );
        let line_idx = 6u32;
        let col = 10u32;
        // Sanity: that line really is the 10-space blank line.
        assert_eq!(src.lines().nth(line_idx as usize).unwrap(), "          ");
        let path = resolve_schema_path(src, Position::new(line_idx, col)).expect("path");
        assert_eq!(segs(&path), vec!["steps", "[0]", "assert", "body", "$.id"]);
    }

    #[test]
    fn resolve_schema_path_inside_capture_yields_steps_index_capture() {
        let src = "name: x\nsteps:\n  - name: s\n    capture:\n      \n";
        // Line 4 = `      `, col 6.
        let path = resolve_schema_path(src, Position::new(4, 6)).expect("path");
        assert_eq!(segs(&path), vec!["steps", "[0]", "capture"]);
    }

    #[test]
    fn resolve_schema_path_inside_poll_yields_steps_index_poll() {
        let src = "name: x\nsteps:\n  - name: s\n    poll:\n      \n";
        // Line 4, col 6.
        let path = resolve_schema_path(src, Position::new(4, 6)).expect("path");
        assert_eq!(segs(&path), vec!["steps", "[0]", "poll"]);
    }

    #[test]
    fn resolve_schema_path_on_broken_yaml_still_returns_permissive_path() {
        // Middle of the document is mangled (stray `???` line) but
        // the walker should still produce a usable path for the
        // blank line at the bottom.
        let src = concat!(
            "name: x\n",
            "steps:\n",
            "  - name: s\n",
            "    request:\n",
            "      method: GET\n",
            "      ??? broken line\n",
            "      \n",
        );
        // Line 6 is `      ` (6 spaces). Cursor col 6.
        assert_eq!(src.lines().nth(6).unwrap(), "      ");
        let path = resolve_schema_path(src, Position::new(6, 6)).expect("path");
        assert_eq!(segs(&path), vec!["steps", "[0]", "request"]);
    }

    #[test]
    fn resolve_schema_path_on_blank_line_inside_nested_request_works() {
        // Regression test: the classifier previously returned
        // `YamlScope::Step` for a cursor deep inside `request:`.
        // Confirm the nested walker produces a proper path even
        // when the enclosing top-level scope is Step.
        let src = "name: x\nsteps:\n  - name: s\n    request:\n      headers:\n        \n";
        // Line 5 = `        ` (8 spaces). Cursor col 8.
        let path = resolve_schema_path(src, Position::new(5, 8)).expect("path");
        assert_eq!(segs(&path), vec!["steps", "[0]", "request", "headers"]);
    }

    #[test]
    fn resolve_schema_path_on_non_blank_line_returns_none() {
        let src = "steps:\n  - name: xxxxx\n";
        // Cursor is in the middle of `xxxxx` — not a blank line.
        assert!(resolve_schema_path(src, Position::new(1, 12)).is_none());
    }

    #[test]
    fn resolve_schema_path_at_document_root_returns_empty_path() {
        let src = "name: x\n\nsteps: []\n";
        let path = resolve_schema_path(src, Position::new(1, 0)).expect("path");
        assert!(path.is_empty());
    }

    // -------- nested_schema_completions ----------

    #[test]
    fn nested_schema_completions_inside_request_offer_request_fields() {
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("steps".into()));
        path.push(PathSegment::Index(0));
        path.push(PathSegment::Key("request".into()));
        let items = nested_schema_completions(&path, schema_key_cache());
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"method"));
        assert!(labels.contains(&"url"));
        assert!(labels.contains(&"headers"));
        assert!(labels.contains(&"body"));
        assert!(labels.contains(&"form"));
        assert!(labels.contains(&"multipart"));
        for item in &items {
            assert_eq!(item.kind, Some(CompletionItemKind::PROPERTY));
            assert!(item.insert_text.as_ref().unwrap().ends_with(": "));
        }
        // Descriptions must come through.
        let url = items.iter().find(|i| i.label == "url").unwrap();
        assert!(url.documentation.is_some());
    }

    #[test]
    fn nested_schema_completions_inside_assert_body_jsonpath_offer_matchers() {
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("steps".into()));
        path.push(PathSegment::Index(0));
        path.push(PathSegment::Key("assert".into()));
        path.push(PathSegment::Key("body".into()));
        path.push(PathSegment::Key("$.id".into()));
        let items = nested_schema_completions(&path, schema_key_cache());
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        for want in [
            "eq", "gt", "matches", "length", "type", "is_uuid", "contains",
        ] {
            assert!(
                labels.contains(&want),
                "missing `{want}` in assert.body matcher completions: {labels:?}"
            );
        }
    }

    #[test]
    fn nested_schema_completions_inside_poll_offer_pollconfig_fields() {
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("steps".into()));
        path.push(PathSegment::Index(0));
        path.push(PathSegment::Key("poll".into()));
        let items = nested_schema_completions(&path, schema_key_cache());
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"until"));
        assert!(labels.contains(&"interval"));
        assert!(labels.contains(&"max_attempts"));
    }

    #[test]
    fn nested_schema_completions_empty_for_unknown_path() {
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("this_is_not_a_real_key".into()));
        let items = nested_schema_completions(&path, schema_key_cache());
        assert!(items.is_empty());
    }

    #[test]
    fn text_document_completion_inside_request_returns_request_fields() {
        let mut store = DocumentStore::new();
        let uri = Url::parse("file:///tmp/cmp-nested-request.tarn.yaml").unwrap();
        let src = "name: x\nsteps:\n  - name: s\n    request:\n      \n";
        store.open(uri.clone(), src.to_owned());
        let response = text_document_completion(&store, &uri, Position::new(4, 6))
            .expect("nested completion should fire");
        let items = match response {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        };
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"method"));
        assert!(labels.contains(&"url"));
        // Specifically must NOT offer the step-level keys here.
        assert!(!labels.contains(&"request"));
        assert!(!labels.contains(&"capture"));
    }
}
