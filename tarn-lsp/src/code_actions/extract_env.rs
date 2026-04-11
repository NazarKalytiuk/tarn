//! `Extract to env var…` code action (NAZ-303, Phase L3.2).
//!
//! Lifts a selected string literal inside a `.tarn.yaml` step into a
//! new env key. Triggers when the cursor / selection lands on a
//! scalar value that lives inside a request field (`url`, a header
//! value, a body field value, a query value, a form field, or a
//! step-level `url`) and is **not** already an interpolation.
//!
//! The action produces a fully-resolved [`CodeAction`] with the
//! following [`WorkspaceEdit`]:
//!
//!   1. One [`TextEdit`] replacing the literal with
//!      `"{{ env.new_env_key }}"` in the current buffer.
//!   2. One [`TextEdit`] inserting `new_env_key: <original-value>`
//!      into the file's inline `env:` block — creating the block at
//!      the top of the file when it does not already exist.
//!
//! Collisions against the set of env keys visible to the file
//! (`inline env: block ∪ tarn.env.yaml ∪ tarn.env.local.yaml ∪
//! tarn.env.{name}.yaml`) are resolved by suffixing the coined name
//! with a counter: `new_env_key`, `new_env_key_2`, `new_env_key_3`, …
//!
//! The module is deliberately split into three layers so every piece
//! is unit-testable in isolation:
//!
//!   * [`extract_env_code_action`] — the pure renderer that takes a
//!     [`CodeActionContext`] and returns `Option<CodeAction>`.
//!   * [`yaml_scalar_literal`] — the helper that escapes an arbitrary
//!     string into a valid YAML scalar for the env block.
//!   * [`pick_unique_env_name`] — the counter-suffix collision
//!     resolver.

use lsp_types::{CodeAction, CodeActionKind, Position, Range, TextEdit, Url, WorkspaceEdit};
use std::collections::HashMap;
use tarn::outline::{find_scalar_at_position, PathSegment, ScalarAtPosition, ScalarStyle};

use crate::code_actions::CodeActionContext;
use crate::identifier::is_valid_identifier;

/// Stable title used in the LSP `CodeAction.title` field. Clients
/// surface this verbatim in the refactor menu. The trailing ellipsis
/// matches VS Code's convention for "opens a follow-up UI" — the
/// current implementation uses a fixed placeholder name so there is
/// no follow-up, but we keep the ellipsis so a future interactive
/// variant can reuse the same string without breaking golden tests.
pub const EXTRACT_ENV_TITLE: &str = "Extract to env var…";

/// Default name for the coined env key. Collision resolution suffixes
/// this with `_2`, `_3`, … if it is already taken.
pub const DEFAULT_ENV_NAME: &str = "new_env_key";

/// Pure renderer for the extract-env-var code action.
///
/// Returns `None` for every soft-fail case (cursor not on a scalar,
/// scalar already an interpolation, scalar not inside a request field,
/// scalar not a string, unparseable buffer, …). A `None` return flows
/// out to the client as "no action offered here", which is the LSP
/// convention for "decline".
pub fn extract_env_code_action(
    uri: &Url,
    source: &str,
    range: Range,
    ctx: &CodeActionContext<'_>,
) -> Option<CodeAction> {
    // 1. Locate the scalar under the cursor / at the selection start.
    //    LSP positions are 0-based lines and columns; the outline
    //    walker works in 1-based coordinates.
    let line_one = (range.start.line as usize) + 1;
    let col_one = (range.start.character as usize) + 1;
    let scalar = find_scalar_at_position(source, line_one, col_one)?;

    // 2. Reject non-extractable scalars: blocks, empty strings,
    //    everything that already looks like an interpolation, and
    //    scalars whose YAML parse shape is not a string at all.
    if scalar.value.is_empty() {
        return None;
    }
    if is_already_interpolation(&scalar.value) {
        return None;
    }
    if is_numeric_or_bool_literal(&scalar) {
        return None;
    }
    if !is_extractable_request_field_path(&scalar.path) {
        return None;
    }

    // 3. A selection that spans multiple YAML nodes is not a clean
    //    extract target. We detect that by checking that the selection
    //    end is still inside the same scalar as the start.
    if range.start != range.end && !selection_within_scalar(&scalar, range) {
        return None;
    }

    // 4. Pick a unique name for the new env key. Walk the current
    //    resolved env map plus the inline env block keys; suffix with
    //    `_2`, `_3`, … until we find a free slot.
    let existing_names = collect_existing_env_names(ctx, source);
    let chosen_name = pick_unique_env_name(DEFAULT_ENV_NAME, &existing_names);
    // Guard against a malformed placeholder — every coined name should
    // pass the identifier validator by construction, but check anyway
    // so a future change to the suffix scheme can never produce an
    // invalid YAML key.
    if !is_valid_identifier(&chosen_name) {
        return None;
    }

    // Log the chosen name to stderr so users can see what was picked.
    // `eprintln!` mirrors every other `tarn-lsp` diagnostic log.
    eprintln!("tarn-lsp: extract env var chose name `{chosen_name}`");

    // 5. Build the replacement edit: swap the literal for a
    //    double-quoted `"{{ env.<name> }}"`.
    let literal_range = scalar_literal_range(&scalar);
    let replacement = format!("\"{{{{ env.{chosen_name} }}}}\"");
    let literal_edit = TextEdit {
        range: literal_range,
        new_text: replacement,
    };

    // 6. Build the env-block insertion edit. Two branches:
    //    a) inline `env:` block already present → append a new entry
    //       at the end of the block (preserving its indentation).
    //    b) no inline env block → insert a fresh `env:` block at the
    //       top of the file, above any `setup:` / `tests:` / `steps:`.
    let escaped_value = yaml_scalar_literal(&scalar.value);
    let env_edit = match find_inline_env_block(source) {
        Some(block) => TextEdit {
            range: Range::new(
                Position::new(block.insertion_line, 0),
                Position::new(block.insertion_line, 0),
            ),
            new_text: format!(
                "{indent}{chosen_name}: {escaped_value}\n",
                indent = " ".repeat(block.child_indent),
            ),
        },
        None => {
            let insertion_line = top_level_insertion_line(source);
            TextEdit {
                range: Range::new(
                    Position::new(insertion_line, 0),
                    Position::new(insertion_line, 0),
                ),
                new_text: format!("env:\n  {chosen_name}: {escaped_value}\n"),
            }
        }
    };

    // 7. Wrap everything into a WorkspaceEdit on the current URL.
    //    Edits are listed in reverse document order so a client that
    //    applies them sequentially does not invalidate earlier offsets.
    let mut edits = vec![literal_edit, env_edit];
    sort_edits_reverse(&mut edits);

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), edits);
    let workspace_edit = WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    };

    Some(CodeAction {
        title: EXTRACT_ENV_TITLE.to_owned(),
        kind: Some(CodeActionKind::REFACTOR_EXTRACT),
        edit: Some(workspace_edit),
        ..CodeAction::default()
    })
}

/// True when the scalar's textual value already looks like a Tarn
/// interpolation — `{{ env.x }}`, `{{ capture.y }}`, `{{ $uuid }}`,
/// etc. We reject these because "extract an interpolation into an env
/// var" is an identity / nonsense operation.
fn is_already_interpolation(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("{{") || trimmed.contains("{{")
}

/// True when the scalar parse shape is a YAML bool, int, float, or
/// null literal — none of those are string literals and we should not
/// offer the refactor. Quoted scalars are always strings, so we only
/// check plain scalars against the handful of YAML 1.2 core schema
/// aliases.
fn is_numeric_or_bool_literal(scalar: &ScalarAtPosition) -> bool {
    if scalar.style != ScalarStyle::Plain {
        return false;
    }
    let v = scalar.value.trim();
    if v.is_empty() {
        return true;
    }
    // Bool / null aliases from YAML 1.1 + 1.2 core schemas.
    matches!(
        v,
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE" | "null" | "Null" | "NULL" | "~"
    ) || is_numeric_literal(v)
}

/// True when `v` parses as an integer or float. Avoids pulling a full
/// YAML typer — the tiny grammar we care about is "optional sign,
/// digits, optional decimal point, optional exponent".
fn is_numeric_literal(v: &str) -> bool {
    if v.parse::<i64>().is_ok() {
        return true;
    }
    if v.parse::<f64>().is_ok() {
        return true;
    }
    false
}

/// True when `path` points at a YAML scalar inside a request field
/// the user would plausibly want to extract.
///
/// Generous but not over-broad:
///
///   * `steps[*].request.url`
///   * `steps[*].request.headers.*`
///   * `steps[*].request.body.*` (any depth — header values and
///     nested body field values are both valid extract targets)
///   * `steps[*].request.query.*`
///   * `steps[*].request.form.*`
///   * `tests.*.steps[*].request.*` and `setup[*].request.*` /
///     `teardown[*].request.*` — same rule, different parent.
///   * The step-level `url:` alias (some tests put `url:` at the step
///     level instead of inside `request:`).
///
/// Explicitly excluded:
///
///   * `name:` — the user almost never extracts a test or step name.
///   * `tags:`
///   * `capture:` — those are JSONPath expressions, not literals.
///   * `assert:` — assertion values are not env-able.
///   * `defaults:` — those are not inside a step.
fn is_extractable_request_field_path(path: &[PathSegment]) -> bool {
    // Find the deepest step container on the path. Every extractable
    // field sits under `request.*` inside one of the known step
    // containers.
    let step_container = match path.first() {
        Some(PathSegment::Key(k)) if k == "steps" || k == "setup" || k == "teardown" => {
            // path[0] = steps / setup / teardown, path[1] should be an
            // Index selecting one step.
            if !matches!(path.get(1), Some(PathSegment::Index(_))) {
                return false;
            }
            2
        }
        Some(PathSegment::Key(k)) if k == "tests" => {
            // path[0] = tests, path[1] = Key(name), path[2] = Key("steps"),
            // path[3] = Index, ...
            if !matches!(path.get(1), Some(PathSegment::Key(_))) {
                return false;
            }
            if !matches!(path.get(2), Some(PathSegment::Key(k)) if k == "steps") {
                return false;
            }
            if !matches!(path.get(3), Some(PathSegment::Index(_))) {
                return false;
            }
            4
        }
        _ => return false,
    };

    // What follows must be `request.*` or the step-level `url:`.
    let Some(next) = path.get(step_container) else {
        return false;
    };
    let PathSegment::Key(key) = next else {
        return false;
    };
    match key.as_str() {
        "url" => {
            // Step-level `url:` alias — only extract when the path
            // ends here or continues under the same key (scalar
            // directly under `url:`).
            path.len() == step_container + 1
        }
        "request" => {
            // Next segment must be a known request sub-field.
            let Some(PathSegment::Key(sub)) = path.get(step_container + 1) else {
                return false;
            };
            matches!(
                sub.as_str(),
                "url" | "headers" | "body" | "query" | "form" | "multipart"
            )
        }
        _ => false,
    }
}

/// Information about an existing inline `env:` block in the source.
#[derive(Debug, Clone)]
struct InlineEnvBlock {
    /// 0-based line on which to insert a new env entry. Always the
    /// line immediately after the last entry in the block (so the
    /// insertion lines up under the existing entries).
    insertion_line: u32,
    /// Column width of one level of indentation for the block's
    /// children. Always `>= 2` in practice.
    child_indent: usize,
}

/// Scan `source` for an inline `env:` block at the top level.
///
/// Returns the insertion line (zero-based) and the child indent in
/// columns. Returns `None` when the file has no `env:` block at the
/// top level, or when the block is present but not a mapping.
fn find_inline_env_block(source: &str) -> Option<InlineEnvBlock> {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    // 1. Find the `env:` line at column 0.
    let env_line_idx = lines
        .iter()
        .position(|line| line_starts_with_key_at_column_zero(line, "env"))?;

    // 2. Walk forward, tracking the maximum indentation we see under
    //    `env:` until a line at column 0 (or less indented than the
    //    block) ends it.
    let mut last_entry_line_idx = env_line_idx;
    let mut child_indent: Option<usize> = None;
    for (idx, line) in lines.iter().enumerate().skip(env_line_idx + 1) {
        // Strip trailing newline for analysis.
        let trimmed_end = line.trim_end_matches(['\n', '\r']);
        if trimmed_end.trim().is_empty() {
            // Blank line inside the block — keep scanning; the
            // structural end is the first non-blank line at column 0.
            continue;
        }
        let indent = trimmed_end.len() - trimmed_end.trim_start().len();
        if indent == 0 {
            // Back to top level. The block ends on the previous
            // non-blank line we saw.
            break;
        }
        // Inside the block.
        child_indent.get_or_insert(indent);
        if indent >= child_indent.unwrap() && is_plausible_key_line(trimmed_end) {
            last_entry_line_idx = idx;
        }
    }
    let child_indent = child_indent?;
    // Insertion line is the one right after the last entry.
    let insertion_line = (last_entry_line_idx as u32) + 1;
    Some(InlineEnvBlock {
        insertion_line,
        child_indent,
    })
}

/// True when `line` starts with `key:` at column 0 (the key is at the
/// root of the document).
fn line_starts_with_key_at_column_zero(line: &str, key: &str) -> bool {
    let trimmed = line.trim_end_matches(['\n', '\r']);
    if let Some(after) = trimmed.strip_prefix(key) {
        return after.starts_with(':');
    }
    false
}

/// Heuristic "does this line look like `key: value`" — used by the
/// env-block scanner to pick the last entry line.
fn is_plausible_key_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    // Reject comments and sequence entries.
    if trimmed.starts_with('#') || trimmed.starts_with('-') {
        return false;
    }
    // A plausible key line contains a colon somewhere.
    trimmed.contains(':')
}

/// Find the 0-based line at which a fresh top-level `env:` block
/// should be inserted when the file has none. Prefers the line right
/// after a file-level `name:` key, otherwise insert at the very top.
fn top_level_insertion_line(source: &str) -> u32 {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    // Walk the top-level keys until we find a suitable insertion
    // point: right after `name:` (if present) but before any of the
    // structural section keys.
    let section_keys = ["setup", "teardown", "steps", "tests"];
    let mut after_name: Option<u32> = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.starts_with("name") && trimmed[4..].starts_with(':') {
            after_name = Some((idx as u32) + 1);
            continue;
        }
        for sk in &section_keys {
            if trimmed.starts_with(sk)
                && trimmed.len() > sk.len()
                && &trimmed[sk.len()..sk.len() + 1] == ":"
            {
                return after_name.unwrap_or(idx as u32);
            }
        }
    }
    after_name.unwrap_or(0)
}

/// Build the LSP [`Range`] that covers the full literal of `scalar`
/// (quotes included for quoted scalars). Uses the 1-based span
/// returned by `find_scalar_at_position` and converts it to 0-based
/// LSP coordinates.
fn scalar_literal_range(scalar: &ScalarAtPosition) -> Range {
    Range::new(
        Position::new(
            scalar.start_line.saturating_sub(1) as u32,
            scalar.start_column.saturating_sub(1) as u32,
        ),
        Position::new(
            scalar.end_line.saturating_sub(1) as u32,
            scalar.end_column.saturating_sub(1) as u32,
        ),
    )
}

/// Check that the selection `range` stays within the scalar's span.
fn selection_within_scalar(scalar: &ScalarAtPosition, range: Range) -> bool {
    let end_line_zero = scalar.end_line.saturating_sub(1) as u32;
    let end_col_zero = scalar.end_column.saturating_sub(1) as u32;
    let start_line_zero = scalar.start_line.saturating_sub(1) as u32;
    let start_col_zero = scalar.start_column.saturating_sub(1) as u32;
    if range.start.line < start_line_zero || range.end.line > end_line_zero {
        return false;
    }
    if range.start.line == start_line_zero && range.start.character < start_col_zero {
        return false;
    }
    if range.end.line == end_line_zero && range.end.character > end_col_zero {
        return false;
    }
    true
}

/// Collect the full set of env key names the file currently sees —
/// the resolved env map from `ctx.env` plus any inline keys parsed
/// out of the buffer. The union is what collision detection checks
/// against.
fn collect_existing_env_names(
    ctx: &CodeActionContext<'_>,
    source: &str,
) -> std::collections::HashSet<String> {
    let mut out: std::collections::HashSet<String> = std::collections::HashSet::new();
    for k in ctx.env.keys() {
        out.insert(k.clone());
    }
    // Belt + braces: even when `ctx.env` is empty (because the
    // renderer was called with a synthetic context), scan the buffer
    // for inline env keys so the unit tests do not need to pre-fill
    // `ctx.env`.
    let inline = tarn::env::inline_env_locations_from_source(source, "");
    for k in inline.keys() {
        out.insert(k.clone());
    }
    out
}

/// Coin a unique env-key name from `base`. Returns `base` if it is
/// free; otherwise suffixes with `_2`, `_3`, … until we find a free
/// slot. Bounded walk — after 1000 attempts we give up and return
/// `base` as-is (the renderer then validates with `is_valid_identifier`).
pub fn pick_unique_env_name(base: &str, existing: &std::collections::HashSet<String>) -> String {
    if !existing.contains(base) {
        return base.to_owned();
    }
    for n in 2..1000u32 {
        let candidate = format!("{base}_{n}");
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
    base.to_owned()
}

/// Sort edits bottom-up so a client applying them sequentially never
/// shifts an earlier offset underneath a later one.
fn sort_edits_reverse(edits: &mut [TextEdit]) {
    edits.sort_by(|a, b| {
        b.range
            .start
            .line
            .cmp(&a.range.start.line)
            .then(b.range.start.character.cmp(&a.range.start.character))
    });
}

/// Escape a string into a valid YAML scalar literal suitable for use
/// on the right-hand side of `key: <literal>` inside an env block.
///
/// Rules:
///
///   * Empty strings become `""`.
///   * Strings containing a newline, tab, `"`, `'`, `\`, or a
///     leading character YAML treats as special (`-`, `?`, `:`, `,`,
///     `[`, `]`, `{`, `}`, `#`, `&`, `*`, `!`, `|`, `>`, `'`, `"`,
///     `%`, `@`, `\``) fall back to a double-quoted form with `\"`
///     and `\\` escaping.
///   * Values that look like a YAML bool / null / number are quoted
///     so the env value stays a string.
///   * Everything else is emitted as a bare plain scalar.
pub fn yaml_scalar_literal(s: &str) -> String {
    if s.is_empty() {
        return "\"\"".to_owned();
    }
    if needs_double_quoting(s) {
        return double_quote(s);
    }
    s.to_owned()
}

fn needs_double_quoting(s: &str) -> bool {
    if s.chars().any(|c| {
        matches!(
            c,
            '\n' | '\r' | '\t' | '"' | '\'' | '\\' | '\u{0}'..='\u{1F}'
        )
    }) {
        return true;
    }
    // Leading special character — YAML would parse these as something
    // other than a plain scalar.
    if let Some(first) = s.chars().next() {
        if matches!(
            first,
            '-' | '?'
                | ':'
                | ','
                | '['
                | ']'
                | '{'
                | '}'
                | '#'
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '%'
                | '@'
                | '`'
                | ' '
                | '\t'
        ) {
            return true;
        }
    }
    // Trailing whitespace would be trimmed by the YAML scanner.
    if s.ends_with(' ') || s.ends_with('\t') {
        return true;
    }
    // Bool / null / numeric aliases — quote them so the env value
    // survives as a string.
    if is_numeric_or_bool_literal_str(s) {
        return true;
    }
    false
}

/// Shadow of [`is_numeric_or_bool_literal`] that works on a raw
/// string without a `ScalarAtPosition`. Used by the YAML escaper.
fn is_numeric_or_bool_literal_str(v: &str) -> bool {
    let v = v.trim();
    if v.is_empty() {
        return false;
    }
    if matches!(
        v,
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE" | "null" | "Null" | "NULL" | "~"
    ) {
        return true;
    }
    is_numeric_literal(v)
}

fn double_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_actions::CodeActionContext;
    use lsp_types::CodeActionContext as LspCodeActionContext;
    use std::collections::{BTreeMap, HashSet};

    fn uri() -> Url {
        Url::parse("file:///tmp/fixture.tarn.yaml").unwrap()
    }

    fn empty_env() -> BTreeMap<String, tarn::env::EnvEntry> {
        BTreeMap::new()
    }

    fn empty_lsp_ctx() -> LspCodeActionContext {
        LspCodeActionContext {
            diagnostics: Vec::new(),
            only: None,
            trigger_kind: None,
        }
    }

    fn ctx_for<'a>(
        uri: &'a Url,
        source: &'a str,
        env: &'a BTreeMap<String, tarn::env::EnvEntry>,
        lsp_ctx: &'a LspCodeActionContext,
    ) -> CodeActionContext<'a> {
        CodeActionContext {
            uri,
            source,
            env,
            lsp_ctx,
        }
    }

    /// Build a cursor range (zero-width) at 0-based line/col.
    fn cursor(line: u32, col: u32) -> Range {
        Range::new(Position::new(line, col), Position::new(line, col))
    }

    // ---------- yaml_scalar_literal ----------

    #[test]
    fn yaml_scalar_literal_simple_scalar_is_bare() {
        assert_eq!(yaml_scalar_literal("hello"), "hello");
        assert_eq!(
            yaml_scalar_literal("http://example.com/a"),
            "http://example.com/a"
        );
    }

    #[test]
    fn yaml_scalar_literal_double_quotes_on_embedded_quote() {
        assert_eq!(
            yaml_scalar_literal("hello \"world\""),
            "\"hello \\\"world\\\"\""
        );
    }

    #[test]
    fn yaml_scalar_literal_escapes_backslashes() {
        assert_eq!(
            yaml_scalar_literal("path\\to\\thing"),
            "\"path\\\\to\\\\thing\""
        );
    }

    #[test]
    fn yaml_scalar_literal_escapes_newlines() {
        assert_eq!(yaml_scalar_literal("line1\nline2"), "\"line1\\nline2\"");
    }

    #[test]
    fn yaml_scalar_literal_empty_string_is_empty_quoted() {
        assert_eq!(yaml_scalar_literal(""), "\"\"");
    }

    #[test]
    fn yaml_scalar_literal_leading_special_char_gets_quoted() {
        assert_eq!(yaml_scalar_literal("-dash"), "\"-dash\"");
        assert_eq!(yaml_scalar_literal("*star"), "\"*star\"");
        assert_eq!(yaml_scalar_literal("&ref"), "\"&ref\"");
    }

    #[test]
    fn yaml_scalar_literal_numeric_and_bool_get_quoted() {
        assert_eq!(yaml_scalar_literal("42"), "\"42\"");
        assert_eq!(yaml_scalar_literal("3.14"), "\"3.14\"");
        assert_eq!(yaml_scalar_literal("true"), "\"true\"");
        assert_eq!(yaml_scalar_literal("null"), "\"null\"");
    }

    // ---------- pick_unique_env_name ----------

    #[test]
    fn pick_unique_env_name_returns_base_when_free() {
        let existing: HashSet<String> = HashSet::new();
        assert_eq!(
            pick_unique_env_name("new_env_key", &existing),
            "new_env_key"
        );
    }

    #[test]
    fn pick_unique_env_name_suffixes_with_2_on_first_collision() {
        let existing: HashSet<String> = ["new_env_key".to_owned()].into_iter().collect();
        assert_eq!(
            pick_unique_env_name("new_env_key", &existing),
            "new_env_key_2"
        );
    }

    #[test]
    fn pick_unique_env_name_walks_suffix_chain() {
        let existing: HashSet<String> = [
            "new_env_key".to_owned(),
            "new_env_key_2".to_owned(),
            "new_env_key_3".to_owned(),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            pick_unique_env_name("new_env_key", &existing),
            "new_env_key_4"
        );
    }

    // ---------- extract_env_code_action (happy paths) ----------

    #[test]
    fn extract_env_happy_path_with_no_env_block_creates_block() {
        let source = "name: fixture\nsteps:\n  - name: s1\n    request:\n      method: GET\n      url: http://example.com/items\n";
        let uri = uri();
        let env = empty_env();
        let lsp_ctx = empty_lsp_ctx();
        let ctx = ctx_for(&uri, source, &env, &lsp_ctx);
        // Cursor on `http://example.com/items` (line 6, col ~12).
        let range = cursor(5, 15);
        let action = extract_env_code_action(&uri, source, range, &ctx).expect("action");
        assert_eq!(action.title, EXTRACT_ENV_TITLE);
        assert_eq!(action.kind, Some(CodeActionKind::REFACTOR_EXTRACT));
        let edit = action.edit.expect("edit");
        let changes = edit.changes.expect("changes");
        let edits = changes.get(&uri).expect("edits for current uri");
        assert_eq!(edits.len(), 2, "literal replacement + env block insert");
        let literal_edit_text = edits
            .iter()
            .find(|e| e.new_text.contains("env.new_env_key"))
            .expect("literal edit");
        assert!(literal_edit_text.new_text.starts_with("\"{{"));
        assert!(literal_edit_text.new_text.ends_with("}}\""));
        let env_edit_text = edits
            .iter()
            .find(|e| e.new_text.starts_with("env:\n"))
            .expect("env block insert");
        assert!(env_edit_text
            .new_text
            .contains("new_env_key: http://example.com/items"));
    }

    #[test]
    fn extract_env_happy_path_with_existing_env_block_appends_entry() {
        let source = "name: fixture\nenv:\n  base_url: http://existing.example\nsteps:\n  - name: s1\n    request:\n      method: GET\n      url: http://other.example/items\n";
        let uri = uri();
        let env = empty_env();
        let lsp_ctx = empty_lsp_ctx();
        let ctx = ctx_for(&uri, source, &env, &lsp_ctx);
        // Cursor inside the URL literal on line 8.
        let range = cursor(7, 15);
        let action = extract_env_code_action(&uri, source, range, &ctx).expect("action");
        let edit = action.edit.expect("edit");
        let changes = edit.changes.expect("changes");
        let edits = changes.get(&uri).expect("edits for current uri");
        // The env-block insert should NOT start with `env:\n` — it
        // should be a two-space-indented entry appended after the
        // existing block.
        let insert = edits
            .iter()
            .find(|e| {
                e.new_text
                    .contains("new_env_key: http://other.example/items")
            })
            .expect("env block append edit");
        assert!(!insert.new_text.starts_with("env:"));
        assert!(insert.new_text.starts_with("  "));
    }

    #[test]
    fn extract_env_collision_suffixes_new_env_key_2() {
        let source = "name: fixture\nenv:\n  new_env_key: taken\nsteps:\n  - name: s1\n    request:\n      method: GET\n      url: http://example.com/\n";
        let uri = uri();
        let env = empty_env();
        let lsp_ctx = empty_lsp_ctx();
        let ctx = ctx_for(&uri, source, &env, &lsp_ctx);
        // Cursor on the step URL.
        let range = cursor(7, 15);
        let action = extract_env_code_action(&uri, source, range, &ctx).expect("action");
        let edit = action.edit.expect("edit");
        let changes = edit.changes.expect("changes");
        let edits = changes.get(&uri).expect("edits");
        assert!(edits
            .iter()
            .any(|e| e.new_text.contains("env.new_env_key_2")));
    }

    #[test]
    fn extract_env_collision_chains_suffixes_to_3() {
        let source = "name: fixture\nenv:\n  new_env_key: a\n  new_env_key_2: b\nsteps:\n  - name: s1\n    request:\n      method: GET\n      url: http://example.com/\n";
        let uri = uri();
        let env = empty_env();
        let lsp_ctx = empty_lsp_ctx();
        let ctx = ctx_for(&uri, source, &env, &lsp_ctx);
        let range = cursor(8, 15);
        let action = extract_env_code_action(&uri, source, range, &ctx).expect("action");
        let edit = action.edit.expect("edit");
        let changes = edit.changes.expect("changes");
        let edits = changes.get(&uri).expect("edits");
        assert!(edits
            .iter()
            .any(|e| e.new_text.contains("env.new_env_key_3")));
    }

    // ---------- no-op / decline branches ----------

    #[test]
    fn extract_env_declines_on_existing_interpolation() {
        let source = "steps:\n  - name: s\n    request:\n      method: GET\n      url: \"{{ env.base_url }}\"\n";
        let uri = uri();
        let env = empty_env();
        let lsp_ctx = empty_lsp_ctx();
        let ctx = ctx_for(&uri, source, &env, &lsp_ctx);
        let range = cursor(4, 20);
        assert!(extract_env_code_action(&uri, source, range, &ctx).is_none());
    }

    #[test]
    fn extract_env_declines_on_non_request_field_like_name() {
        let source =
            "steps:\n  - name: hello\n    request:\n      method: GET\n      url: http://x/\n";
        let uri = uri();
        let env = empty_env();
        let lsp_ctx = empty_lsp_ctx();
        let ctx = ctx_for(&uri, source, &env, &lsp_ctx);
        // Cursor on `hello` — step name, not a request field.
        let range = cursor(1, 12);
        assert!(extract_env_code_action(&uri, source, range, &ctx).is_none());
    }

    #[test]
    fn extract_env_declines_on_numeric_literal() {
        // A numeric literal is a YAML int, not a string.
        let source = "steps:\n  - name: s\n    request:\n      method: GET\n      url: http://x/\n    retries: 3\n";
        let uri = uri();
        let env = empty_env();
        let lsp_ctx = empty_lsp_ctx();
        let ctx = ctx_for(&uri, source, &env, &lsp_ctx);
        // Cursor on the `3` of `retries: 3`.
        let range = cursor(5, 13);
        assert!(extract_env_code_action(&uri, source, range, &ctx).is_none());
    }

    #[test]
    fn extract_env_declines_on_boolean_literal() {
        let source = "steps:\n  - name: s\n    request:\n      method: GET\n      url: http://x/\n    follow_redirects: true\n";
        let uri = uri();
        let env = empty_env();
        let lsp_ctx = empty_lsp_ctx();
        let ctx = ctx_for(&uri, source, &env, &lsp_ctx);
        // Cursor on `true`.
        let range = cursor(5, 22);
        assert!(extract_env_code_action(&uri, source, range, &ctx).is_none());
    }

    #[test]
    fn extract_env_cursor_only_selection_still_works() {
        // A zero-width cursor selection is the common case for
        // "cursor placed in a literal". The renderer must produce an
        // edit even when the LSP range is empty.
        let source = "steps:\n  - name: s\n    request:\n      method: GET\n      url: http://example.com/\n";
        let uri = uri();
        let env = empty_env();
        let lsp_ctx = empty_lsp_ctx();
        let ctx = ctx_for(&uri, source, &env, &lsp_ctx);
        let range = cursor(4, 15);
        assert!(extract_env_code_action(&uri, source, range, &ctx).is_some());
    }

    #[test]
    fn extract_env_selection_spanning_multiple_nodes_is_declined() {
        // A selection that spans from inside the url literal to the
        // next line should not produce an extract.
        let source = "steps:\n  - name: s\n    request:\n      method: GET\n      url: http://example.com/\n    retries: 3\n";
        let uri = uri();
        let env = empty_env();
        let lsp_ctx = empty_lsp_ctx();
        let ctx = ctx_for(&uri, source, &env, &lsp_ctx);
        let range = Range::new(Position::new(4, 15), Position::new(5, 5));
        assert!(extract_env_code_action(&uri, source, range, &ctx).is_none());
    }

    #[test]
    fn extract_env_extracts_header_value_inside_request_headers() {
        let source = "steps:\n  - name: s\n    request:\n      method: GET\n      url: http://x/\n      headers:\n        Authorization: \"Bearer abc\"\n";
        let uri = uri();
        let env = empty_env();
        let lsp_ctx = empty_lsp_ctx();
        let ctx = ctx_for(&uri, source, &env, &lsp_ctx);
        // Cursor inside `Bearer abc`.
        let range = cursor(6, 28);
        let action = extract_env_code_action(&uri, source, range, &ctx).expect("action");
        let edit = action.edit.expect("edit");
        let changes = edit.changes.expect("changes");
        let edits = changes.get(&uri).expect("edits");
        assert!(edits
            .iter()
            .any(|e| e.new_text.contains("new_env_key: Bearer abc")
                || e.new_text.contains("\"Bearer abc\"")));
    }

    #[test]
    fn extract_env_replacement_keeps_quotes_around_interpolation() {
        let source = "steps:\n  - name: s\n    request:\n      method: GET\n      url: http://example.com/\n";
        let uri = uri();
        let env = empty_env();
        let lsp_ctx = empty_lsp_ctx();
        let ctx = ctx_for(&uri, source, &env, &lsp_ctx);
        let range = cursor(4, 15);
        let action = extract_env_code_action(&uri, source, range, &ctx).expect("action");
        let edit = action.edit.expect("edit");
        let changes = edit.changes.expect("changes");
        let edits = changes.get(&uri).expect("edits");
        let literal = edits
            .iter()
            .find(|e| e.new_text.contains("env.new_env_key"))
            .expect("literal edit");
        // The replacement must be a fully-quoted interpolation so the
        // YAML parse shape stays a string.
        assert!(literal.new_text.starts_with("\"{{"));
        assert!(literal.new_text.ends_with("}}\""));
    }
}
