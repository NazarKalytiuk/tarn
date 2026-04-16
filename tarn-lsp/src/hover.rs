//! `textDocument/hover` handling for interpolation tokens and top-level
//! schema keys in `.tarn.yaml` documents.
//!
//! The hover provider is the first feature in Phase L1 that needs to peek
//! into the *content* of a buffer, not just re-run the validator on it. The
//! work decomposes into four pure helpers plus a thin I/O wrapper:
//!
//!   1. [`resolve_hover_token`] — classify the cursor position into one of
//!      `Env`, `Capture`, `Builtin`, or `SchemaKey` (or `None`). Pure; no
//!      filesystem, no parsing, no LSP types beyond [`Position`].
//!   2. [`collect_visible_captures`] — walk the already-parsed `TestFile`
//!      and return the captures that are in scope for a cursor *line*.
//!      Mirrors the TypeScript `collectVisibleCaptures` helper used by the
//!      VS Code extension. Pure apart from needing a parsed `TestFile`.
//!   3. [`hover_for_token`] — render a token + [`HoverContext`] into the
//!      final [`Hover`] value. Pure; unit-tested with synthetic contexts.
//!   4. [`text_document_hover`] — the request handler. Reads the buffer
//!      from [`DocumentStore`], builds a [`HoverContext`], calls the pure
//!      helpers, and returns the `Hover` the server should reply with.
//!
//! Everything except step (4) is unit-tested in this file. Step (4) is
//! covered by the integration tests in `tests/hover_test.rs`.
//!
//! ## Capture-value-from-report
//!
//! The ticket's "most recent report value" bullet is deferred to a
//! follow-up. Locating a report on disk is non-trivial (no stable path
//! convention yet) and the critical user story — "hover shows the
//! declaring step and its source" — already ships without it. See
//! `NOTES` in the commit message and the ticket for the follow-up.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Range, Url};
use tarn::env::{self, EnvEntry, EnvSource};
use tarn::jsonpath::{evaluate_path, JsonPathError};
use tarn::model::{Step, TestFile};
use tarn::outline::{find_scalar_at_position, outline_from_str, PathSegment, StepOutline};
use tarn::parser;

use crate::code_actions::response_source::{DiskResponseSource, RecordedResponseSource};
use crate::schema::schema_key_cache;
use crate::server::{is_tarn_file_uri, DocumentStore};
use crate::token::{
    byte_offset_to_position, find_line_end, find_subslice, is_identifier, position_to_byte_offset,
    position_to_line_start,
};

/// Static documentation for a single `$builtin` function. Mirrors the
/// `BUILTIN_FUNCTIONS` table shipped by the VS Code extension so hover
/// content stays in sync across clients. The `name` field is the raw
/// identifier after the leading `$` (e.g. `"uuid"`), `signature` is the
/// full call form shown in the hover tooltip, and `doc` is the
/// one-sentence description.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinDoc {
    pub name: &'static str,
    pub signature: &'static str,
    pub doc: &'static str,
}

/// The five built-in functions supported by `tarn::builtin::evaluate`.
///
/// Keeping the list colocated with the hover provider (rather than moving
/// it into `tarn::builtin`) lets us ship user-facing docstrings without
/// polluting the runtime crate.
pub const BUILTIN_DOCS: &[BuiltinDoc] = &[
    BuiltinDoc {
        name: "uuid",
        signature: "$uuid",
        doc: "Generate a random UUID v4 (36 characters, `8-4-4-4-12` format).",
    },
    BuiltinDoc {
        name: "timestamp",
        signature: "$timestamp",
        doc: "Current UNIX timestamp in seconds as a decimal integer.",
    },
    BuiltinDoc {
        name: "now_iso",
        signature: "$now_iso",
        doc:
            "Current UTC time as an RFC 3339 / ISO 8601 string (e.g. `2025-01-02T03:04:05+00:00`).",
    },
    BuiltinDoc {
        name: "random_hex",
        signature: "$random_hex(n)",
        doc: "Generate a random hexadecimal string of length `n` characters.",
    },
    BuiltinDoc {
        name: "random_int",
        signature: "$random_int(min, max)",
        doc: "Generate a random integer in the inclusive range `[min, max]`.",
    },
];

/// Classification of the token under the cursor for a hover request.
///
/// The enum is deliberately flat: it carries just enough information
/// (identifier string + kind) for the renderer to pick a template.
/// Precise source ranges are tracked separately in [`HoverTokenSpan`] so
/// the renderer and the LSP handler can share one shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoverToken {
    /// `{{ env.KEY }}`. Identifier is the bare `KEY` (may be empty when
    /// the user types `{{ env. }}` mid-edit).
    Env(String),
    /// `{{ capture.NAME }}`. Identifier is the bare `NAME`.
    Capture(String),
    /// `{{ $builtin }}` or `{{ $random_hex(8) }}`. Identifier is the
    /// function name with the leading `$` stripped.
    Builtin(String),
    /// Top-level schema key (`status`, `body`, `headers`, etc).
    /// Identifier is the bare key name.
    SchemaKey(String),
    /// A raw YAML scalar whose content starts with `$.` or `$[` —
    /// a JSONPath literal inside an `assert.body:` key, a `capture:`
    /// JSONPath value, or a `poll.until.jsonpath` value. NAZ-307
    /// (L3.6): when a sidecar response is available for the enclosing
    /// step, the hover evaluates the path and appends the result.
    JsonPathLiteral(String),
}

/// The enclosing-step metadata needed to look up a recorded response
/// for a JSONPath hover. Derived from the YAML path the scalar walker
/// emits (NAZ-303 `find_scalar_at_position`).
///
/// `test` uses the sentinel strings `"setup"` / `"teardown"` / `"<flat>"`
/// for non-test sections so the sidecar convention from NAZ-304 stays
/// consistent — the code-action renderers already rely on the same
/// sentinels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepContext {
    /// The enclosing test group's name, or a sentinel for
    /// setup/teardown/flat-steps sections.
    pub test: String,
    /// The step's `name:` value. Synthetic `<step N>` placeholders
    /// from unnamed steps are acceptable here — they simply won't
    /// match any sidecar file, which degrades to "no evaluation"
    /// rather than an error.
    pub step: String,
}

/// A hover token plus the source range that should be highlighted by
/// the LSP client when the hover is displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverTokenSpan {
    pub token: HoverToken,
    pub range: Range,
    /// Populated only for `HoverToken::JsonPathLiteral` so the renderer
    /// can look up the sidecar response for the enclosing step. `None`
    /// for every other token class — no other hover needs step context.
    pub step_context: Option<StepContext>,
}

/// Context data the [`hover_for_token`] renderer consults. All fields
/// are precomputed by the request handler so the renderer itself stays
/// I/O-free and easy to unit-test.
#[derive(Default)]
pub struct HoverContext {
    /// Resolved environment, keyed by variable name. Sorted so tests
    /// can assert ordering without sprinkling `sort_by_key` everywhere.
    pub env: BTreeMap<String, EnvEntry>,
    /// Env variable names (lowercased) flagged for redaction via the
    /// test file's `redaction.env:` block.
    pub redacted_env_keys: Vec<String>,
    /// Captures declared by earlier steps in the same test (plus setup).
    /// Empty when the cursor is outside any step or when the document
    /// has a parse error.
    pub visible_captures: Vec<VisibleCapture>,
    /// Top-level schema key -> `description` field from
    /// `schemas/v1/testfile.json`.
    pub schema_keys: HashMap<String, String>,
    /// True when the document failed to parse. Capture hovers fall
    /// back to a degraded "parse error" message in that case rather
    /// than claiming the capture is undefined.
    pub parse_errored: bool,
    /// Resolved captures for each name that appear anywhere in the
    /// document — not just in scope for the current position. Lets the
    /// renderer distinguish "undefined" from "out of scope".
    pub all_captures: Vec<VisibleCapture>,
    /// Pluggable recorded-response reader used by L3.6 JSONPath hover
    /// evaluation (NAZ-307). Shares the `RecordedResponseSource` trait
    /// and the sidecar convention introduced in NAZ-304 for the
    /// scaffold-assert code action. `None` means "no evaluation available",
    /// and the JSONPath hover gracefully falls back to a heading-only
    /// markdown.
    pub response_source: Option<Arc<dyn RecordedResponseSource>>,
    /// Absolute filesystem path of the document, used by the JSONPath
    /// hover to compute the sidecar path through the response reader.
    /// Empty for unit tests that synthesise a context from scratch.
    pub file_path: PathBuf,
}

/// One capture variable visible from a hover position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleCapture {
    pub name: String,
    pub step_name: String,
    pub step_index: usize,
    /// Which phase the declaring step is in.
    pub phase: CapturePhase,
    /// For `CapturePhase::Test`, the name of the enclosing test group.
    pub test_name: Option<String>,
    /// Short description of the capture source — either a JSONPath or a
    /// header/cookie/status/url/regex label. Shown in the hover body.
    pub source: String,
}

/// Section of the test file a step lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePhase {
    Setup,
    Test,
    FlatSteps,
    Teardown,
}

/// Classify the cursor at `position` into a [`HoverTokenSpan`], or return
/// `None` when the cursor is outside any recognised construct.
///
/// The helper is deliberately 100% pure — it never touches the filesystem,
/// never spawns a tarn parser, and never constructs types outside the
/// `lsp_types` surface it already depends on.
///
/// # Order of precedence
///
/// 1. Interpolation tokens (`{{ … }}`). These always win, even when the
///    cursor happens to sit inside a top-level key string that also
///    contains an interpolation. In practice this never happens — YAML
///    keys don't contain `{{`. The rule just keeps the classifier
///    consistent across files.
/// 2. Top-level schema keys. A cursor on a bare `status:`, `body:`,
///    `request:`, etc. on a line that looks like a mapping key resolves
///    to a `SchemaKey` token.
/// 3. JSONPath literals (L3.6, NAZ-307). A cursor on a YAML scalar
///    whose text begins with `$.` or `$[` resolves to a
///    [`HoverToken::JsonPathLiteral`] with step context filled in
///    from the YAML outline walker so the renderer can look up the
///    sidecar response for the enclosing step.
pub fn resolve_hover_token(source: &str, position: Position) -> Option<HoverTokenSpan> {
    if let Some(span) = find_interpolation_token(source, position) {
        return Some(span);
    }
    if let Some(span) = find_schema_key_token(source, position) {
        return Some(span);
    }
    resolve_jsonpath_at_position(source, position)
}

/// Resolve the cursor to a JSONPath literal scalar, if any.
///
/// Uses [`find_scalar_at_position`] to locate the innermost scalar
/// under the cursor and only accepts scalars whose unquoted text
/// starts with `$.` or `$[` — the two canonical openings for a
/// JSONPath expression. The enclosing step context is derived from
/// the scalar's YAML path combined with the file outline so the
/// hover renderer can look up the last recorded response via the
/// sidecar convention. Returns `None` for unparseable YAML, for
/// cursors outside any scalar, for scalars that are not JSONPath
/// literals, and for JSONPath scalars whose YAML path does not
/// resolve to a step (e.g. a literal typed into a free-form
/// `description:` field).
///
/// Strictly pure — no filesystem access. The outline is parsed from
/// the in-memory source buffer.
pub fn resolve_jsonpath_at_position(source: &str, position: Position) -> Option<HoverTokenSpan> {
    let line_one = (position.line as usize) + 1;
    let col_one = (position.character as usize) + 1;
    let scalar = find_scalar_at_position(source, line_one, col_one)?;
    if !is_jsonpath_literal(&scalar.value) {
        return None;
    }
    // JSONPath scalars appear in three expected shapes today:
    //   * keys of `assert.body.<jsonpath>:` mappings
    //   * values of `capture.<name>.jsonpath: "<jsonpath>"`
    //   * values of `poll.until.jsonpath: "<jsonpath>"`
    // Only accept one of those shapes so a literal pasted into a
    // random free-form field (e.g. `description: "$.foo"`) does not
    // pretend to be evaluable. The shape check is lenient — we walk
    // the YAML path for any anchor that indicates assert/body,
    // capture, or poll context.
    if !path_looks_like_jsonpath_carrier(&scalar.path) {
        return None;
    }
    let step_context = step_context_from_path_and_outline(source, &scalar.path);
    let start_line = scalar.start_line.saturating_sub(1) as u32;
    let start_col = scalar.start_column.saturating_sub(1) as u32;
    let end_line = scalar.end_line.saturating_sub(1) as u32;
    let end_col = scalar.end_column.saturating_sub(1) as u32;
    let range = Range::new(
        Position::new(start_line, start_col),
        Position::new(end_line, end_col),
    );
    Some(HoverTokenSpan {
        token: HoverToken::JsonPathLiteral(scalar.value.clone()),
        range,
        step_context,
    })
}

/// True for any scalar string whose first non-whitespace characters are
/// `$.` or `$[`. A bare `$` alone is also accepted so the hover
/// gracefully reports "whole body" evaluation. Everything else
/// (including quoted YAML scalars whose raw text happens to start with
/// `$` mid-string) returns `false`.
fn is_jsonpath_literal(value: &str) -> bool {
    let trimmed = value.trim_start();
    if trimmed == "$" {
        return true;
    }
    trimmed.starts_with("$.") || trimmed.starts_with("$[")
}

/// True when the YAML path points at a location where a JSONPath
/// literal is meaningful — namely inside an `assert.body` block, a
/// `capture.*.jsonpath` value, a bare capture JSONPath shorthand, or
/// a `poll.until.jsonpath` value.
fn path_looks_like_jsonpath_carrier(path: &[PathSegment]) -> bool {
    // `assert.body.<jsonpath>` — the key of the body mapping.
    let mut saw_assert_body = false;
    let mut saw_capture = false;
    let mut saw_poll_until = false;
    for (i, seg) in path.iter().enumerate() {
        if let PathSegment::Key(k) = seg {
            if k == "assert"
                && matches!(path.get(i + 1), Some(PathSegment::Key(k2)) if k2 == "body")
            {
                saw_assert_body = true;
            }
            if k == "capture" {
                saw_capture = true;
            }
            if k == "poll" && matches!(path.get(i + 1), Some(PathSegment::Key(k2)) if k2 == "until")
            {
                saw_poll_until = true;
            }
        }
    }
    saw_assert_body || saw_capture || saw_poll_until
}

/// Derive the `(test, step)` identifier pair from a YAML path and the
/// file outline so the hover can look up the sidecar response.
///
/// Mirrors the step locator in [`crate::code_actions::scaffold_assert`]
/// but looks up the step's actual `name:` through [`outline_from_str`]
/// so the resulting identifier matches what the sidecar writer stores.
/// Returns `None` when the path does not point at a step or when the
/// outline cannot be parsed — the renderer treats both as
/// "no evaluation available".
fn step_context_from_path_and_outline(source: &str, path: &[PathSegment]) -> Option<StepContext> {
    let outline = outline_from_str("<hover>", source)?;
    let first = path.first()?;
    match first {
        PathSegment::Key(k) if k == "setup" => {
            let PathSegment::Index(i) = path.get(1)? else {
                return None;
            };
            let step: &StepOutline = outline.setup.get(*i)?;
            Some(StepContext {
                test: "setup".to_owned(),
                step: step.name.clone(),
            })
        }
        PathSegment::Key(k) if k == "teardown" => {
            let PathSegment::Index(i) = path.get(1)? else {
                return None;
            };
            let step: &StepOutline = outline.teardown.get(*i)?;
            Some(StepContext {
                test: "teardown".to_owned(),
                step: step.name.clone(),
            })
        }
        PathSegment::Key(k) if k == "steps" => {
            let PathSegment::Index(i) = path.get(1)? else {
                return None;
            };
            let step: &StepOutline = outline.flat_steps.get(*i)?;
            Some(StepContext {
                test: "<flat>".to_owned(),
                step: step.name.clone(),
            })
        }
        PathSegment::Key(k) if k == "tests" => {
            let PathSegment::Key(test_name) = path.get(1)? else {
                return None;
            };
            // tests.<name>.steps.<index>
            let PathSegment::Key(steps_key) = path.get(2)? else {
                return None;
            };
            if steps_key != "steps" {
                return None;
            }
            let PathSegment::Index(i) = path.get(3)? else {
                return None;
            };
            let test = outline.tests.iter().find(|t| &t.name == test_name)?;
            let step = test.steps.get(*i)?;
            Some(StepContext {
                test: test_name.clone(),
                step: step.name.clone(),
            })
        }
        _ => None,
    }
}

/// Scan the source for every `{{ … }}` pair and return the one that
/// encloses the cursor.
///
/// Multiline tokens are supported: the scan is done over the whole
/// document text, so a `{{ env.base\n  _url }}` span that wraps a newline
/// still resolves. We do _not_ match mismatched `{{` without a closing
/// `}}`, so a typed-but-unfinished `{{ env.` returns `None` instead of
/// claiming the rest of the file as one token.
fn find_interpolation_token(source: &str, position: Position) -> Option<HoverTokenSpan> {
    let cursor_offset = position_to_byte_offset(source, position)?;
    let bytes = source.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Find the matching `}}`.
            if let Some(rel_end) = find_subslice(&bytes[i + 2..], b"}}") {
                let content_start = i + 2;
                let content_end = i + 2 + rel_end;
                let token_end = content_end + 2;
                if cursor_offset >= i && cursor_offset <= token_end {
                    let raw = &source[content_start..content_end];
                    let start_pos = byte_offset_to_position(source, i);
                    let end_pos = byte_offset_to_position(source, token_end);
                    let range = Range::new(start_pos, end_pos);
                    return classify_expression(raw.trim()).map(|token| HoverTokenSpan {
                        token,
                        range,
                        step_context: None,
                    });
                }
                i = token_end;
                continue;
            } else {
                return None;
            }
        }
        i += 1;
    }
    None
}

fn classify_expression(raw: &str) -> Option<HoverToken> {
    if raw.is_empty() {
        return None;
    }
    if let Some(rest) = raw.strip_prefix("env.") {
        return Some(HoverToken::Env(rest.trim().to_owned()));
    }
    if raw == "env" {
        return Some(HoverToken::Env(String::new()));
    }
    if let Some(rest) = raw.strip_prefix("capture.") {
        return Some(HoverToken::Capture(rest.trim().to_owned()));
    }
    if raw == "capture" {
        return Some(HoverToken::Capture(String::new()));
    }
    if let Some(rest) = raw.strip_prefix('$') {
        // Function name is everything up to `(`. `$random_hex(8)` → `random_hex`.
        let name = rest.split('(').next().unwrap_or("").trim();
        return Some(HoverToken::Builtin(name.to_owned()));
    }
    None
}

/// Match a cursor to a top-level schema key at the start of a line.
///
/// The classifier is intentionally conservative: it requires the cursor
/// to be on a line whose non-whitespace prefix is exactly `<key>:` where
/// `<key>` is one of the known schema keys. That rules out accidentally
/// classifying an ambiguous inline key inside a request body.
fn find_schema_key_token(source: &str, position: Position) -> Option<HoverTokenSpan> {
    let line_start_offset = position_to_line_start(source, position.line as usize)?;
    let line_end_offset = find_line_end(source, line_start_offset);
    let line = &source[line_start_offset..line_end_offset];
    let stripped = line.trim_start();
    let leading_spaces = line.len() - stripped.len();
    let colon_pos = stripped.find(':')?;
    let key = stripped[..colon_pos].trim_end();
    if key.is_empty() || !is_identifier(key) {
        return None;
    }
    if !SCHEMA_KEY_NAMES.contains(&key) {
        return None;
    }
    let key_start_col = leading_spaces;
    let key_end_col = leading_spaces + key.len();
    let cursor_col = position.character as usize;
    if cursor_col < key_start_col || cursor_col > key_end_col {
        return None;
    }
    let start_pos = Position::new(position.line, key_start_col as u32);
    let end_pos = Position::new(position.line, key_end_col as u32);
    Some(HoverTokenSpan {
        token: HoverToken::SchemaKey(key.to_owned()),
        range: Range::new(start_pos, end_pos),
        step_context: None,
    })
}

/// Top-level keys on `TestFile` plus the most load-bearing step-level
/// keys (`request`, `assert`, `capture`, `status`, `body`, `headers`).
/// Keep this in sync with `testfile.json` — tests assert the set is a
/// subset of what the schema file actually describes.
const SCHEMA_KEY_NAMES: &[&str] = &[
    "version",
    "name",
    "description",
    "tags",
    "env",
    "cookies",
    "redaction",
    "defaults",
    "setup",
    "teardown",
    "tests",
    "steps",
    "request",
    "capture",
    "assert",
    "status",
    "body",
    "headers",
    "method",
    "url",
    "form",
    "multipart",
    "auth",
    "graphql",
    "poll",
    "script",
    "retries",
    "timeout",
    "connect_timeout",
    "follow_redirects",
    "max_redirs",
    "delay",
    "include",
];

/// Schema-key `description` lookup, parsed once at first access.
///
/// Thin re-export over [`crate::schema::schema_key_cache`] so legacy
/// callers (and the hover tests) can still ask for the description
/// map directly. New code should prefer the shared cache so it can
/// also see the per-scope key lists.
pub fn schema_key_descriptions() -> &'static HashMap<String, String> {
    schema_key_cache().descriptions()
}

/// Which container of steps the cursor line is closest to. The
/// [`collect_visible_captures`] walker uses this to pick the one
/// vector that owns the cursor without needing per-section line-range
/// metadata.
#[derive(Debug, Clone)]
enum CursorSection<'a> {
    Setup,
    FlatSteps,
    Test(&'a str),
    Teardown,
}

/// Walk a parsed [`TestFile`] and return every capture that is visible
/// from the given 1-based cursor line.
///
/// The logic mirrors the TypeScript `collectVisibleCaptures` used by the
/// VS Code extension:
///   * Setup captures are visible from every step in every test.
///   * Within a test, captures from strictly earlier steps are visible.
///   * Same-step, later-step, and cross-test captures are not visible.
///
/// Because `Step.location.line` is the only per-step anchor we have, the
/// walker first picks the single step across the whole file whose line
/// is the greatest ≤ cursor line — the "closest preceding" step. That
/// step's enclosing section wins. This way a cursor that has drifted
/// into a named `tests:` block never mis-resolves to the earlier `setup:`
/// block just because setup[0]'s line is also ≤ cursor line.
pub fn collect_visible_captures(
    test_file: &TestFile,
    cursor_line_one_based: usize,
) -> Vec<VisibleCapture> {
    let setup_snapshot = collect_step_captures(&test_file.setup, CapturePhase::Setup, None);
    let Some((section, step_idx)) = closest_preceding_section(test_file, cursor_line_one_based)
    else {
        // Cursor is before the first step of any section — only setup
        // captures (of which there are none that "precede" the cursor)
        // would apply. Return the setup snapshot so hovers outside any
        // step still show setup captures as in-scope.
        return setup_snapshot;
    };

    match section {
        CursorSection::Setup => setup_snapshot
            .into_iter()
            .filter(|c| c.step_index < step_idx)
            .collect(),
        CursorSection::FlatSteps => {
            let flat_caps = collect_step_captures(&test_file.steps, CapturePhase::FlatSteps, None);
            let in_scope: Vec<_> = flat_caps
                .into_iter()
                .filter(|c| c.step_index < step_idx)
                .collect();
            let mut out = setup_snapshot;
            out.extend(in_scope);
            out
        }
        CursorSection::Test(test_name) => {
            let group = &test_file.tests[test_name];
            let test_caps =
                collect_step_captures(&group.steps, CapturePhase::Test, Some(test_name));
            let in_scope: Vec<_> = test_caps
                .into_iter()
                .filter(|c| c.step_index < step_idx)
                .collect();
            let mut out = setup_snapshot;
            out.extend(in_scope);
            out
        }
        CursorSection::Teardown => {
            // Teardown can see every prior capture (setup + tests +
            // flat steps), regardless of the cursor's step index.
            let mut out = setup_snapshot;
            for (test_name, group) in &test_file.tests {
                out.extend(collect_step_captures(
                    &group.steps,
                    CapturePhase::Test,
                    Some(test_name.as_str()),
                ));
            }
            out.extend(collect_step_captures(
                &test_file.steps,
                CapturePhase::FlatSteps,
                None,
            ));
            out
        }
    }
}

/// Scan every step vector in the test file and pick the step whose line
/// is the greatest value ≤ the cursor line. Returns the section that
/// step belongs to along with its 0-based index within that section.
fn closest_preceding_section<'a>(
    test_file: &'a TestFile,
    cursor_line_one_based: usize,
) -> Option<(CursorSection<'a>, usize)> {
    let mut best: Option<(usize, CursorSection<'a>, usize)> = None;

    let mut consider = |section: CursorSection<'a>, steps: &[Step]| {
        for (idx, step) in steps.iter().enumerate() {
            if let Some(loc) = &step.location {
                if loc.line <= cursor_line_one_based {
                    let line = loc.line;
                    match &best {
                        Some((best_line, _, _)) if *best_line >= line => {}
                        _ => {
                            best = Some((line, section.clone(), idx));
                        }
                    }
                }
            }
        }
    };

    consider(CursorSection::Setup, &test_file.setup);
    consider(CursorSection::FlatSteps, &test_file.steps);
    for (test_name, group) in &test_file.tests {
        consider(CursorSection::Test(test_name.as_str()), &group.steps);
    }
    consider(CursorSection::Teardown, &test_file.teardown);

    best.map(|(_, section, idx)| (section, idx))
}

/// Collect *every* capture declared anywhere in the test file, regardless
/// of cursor position. Used to distinguish "undefined" from "out of scope".
pub fn collect_all_captures(test_file: &TestFile) -> Vec<VisibleCapture> {
    let mut out = collect_step_captures(&test_file.setup, CapturePhase::Setup, None);
    out.extend(collect_step_captures(
        &test_file.steps,
        CapturePhase::FlatSteps,
        None,
    ));
    for (test_name, group) in &test_file.tests {
        out.extend(collect_step_captures(
            &group.steps,
            CapturePhase::Test,
            Some(test_name.as_str()),
        ));
    }
    out.extend(collect_step_captures(
        &test_file.teardown,
        CapturePhase::Teardown,
        None,
    ));
    out
}

fn collect_step_captures(
    steps: &[Step],
    phase: CapturePhase,
    test_name: Option<&str>,
) -> Vec<VisibleCapture> {
    let mut out = Vec::new();
    for (idx, step) in steps.iter().enumerate() {
        for (name, spec) in &step.capture {
            out.push(VisibleCapture {
                name: name.clone(),
                step_name: step.name.clone(),
                step_index: idx,
                phase,
                test_name: test_name.map(ToOwned::to_owned),
                source: describe_capture_source(spec),
            });
        }
    }
    out
}

fn describe_capture_source(spec: &tarn::model::CaptureSpec) -> String {
    match spec {
        tarn::model::CaptureSpec::JsonPath(path) => format!("JSONPath `{path}`"),
        tarn::model::CaptureSpec::Extended(ext) => {
            let mut parts = Vec::new();
            if let Some(h) = &ext.header {
                parts.push(format!("header `{h}`"));
            }
            if let Some(c) = &ext.cookie {
                parts.push(format!("cookie `{c}`"));
            }
            if let Some(jp) = &ext.jsonpath {
                parts.push(format!("JSONPath `{jp}`"));
            }
            if matches!(ext.body, Some(true)) {
                parts.push("whole body".to_owned());
            }
            if matches!(ext.status, Some(true)) {
                parts.push("status code".to_owned());
            }
            if matches!(ext.url, Some(true)) {
                parts.push("final URL".to_owned());
            }
            if let Some(rx) = &ext.regex {
                parts.push(format!("regex `{rx}`"));
            }
            if parts.is_empty() {
                "extended capture".to_owned()
            } else {
                parts.join(", ")
            }
        }
    }
}

/// Render a [`HoverTokenSpan`] into a full LSP [`Hover`] with Markdown body.
///
/// This is the single place that formats user-visible strings. Tests
/// assert the exact markdown so a rename anywhere upstream is caught
/// immediately.
pub fn hover_for_token(span: &HoverTokenSpan, ctx: &HoverContext) -> Hover {
    let body = match &span.token {
        HoverToken::Env(key) => render_env(key, ctx),
        HoverToken::Capture(name) => render_capture(name, ctx),
        HoverToken::Builtin(name) => render_builtin(name),
        HoverToken::SchemaKey(key) => render_schema_key(key, ctx),
        HoverToken::JsonPathLiteral(path) => {
            render_jsonpath_literal(path, span.step_context.as_ref(), ctx)
        }
    };
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: body,
        }),
        range: Some(span.range),
    }
}

/// Maximum characters of pretty-printed JSON the JSONPath hover will
/// inline before truncating with a `…` marker. Two kilobytes is a
/// soft ceiling picked to match the hover popup size that most LSP
/// clients render comfortably without scrollbars.
const JSONPATH_RESULT_MAX_LEN: usize = 2000;

/// Render a `JsonPathLiteral` hover, optionally appending the result of
/// evaluating the path against the step's last recorded response.
///
/// Always emits a heading so the hover never shows up as an empty
/// popup. When the sidecar lookup succeeds, the result is
/// pretty-printed as JSON and capped at [`JSONPATH_RESULT_MAX_LEN`];
/// longer payloads are truncated with an explicit marker so callers
/// know the full value was elided for display.
fn render_jsonpath_literal(
    path: &str,
    step_context: Option<&StepContext>,
    ctx: &HoverContext,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("**JSONPath literal:** `{path}`\n\n"));
    out.push_str(
        "Evaluated in place against the step's last recorded response (sidecar from `.last-run`).\n\n",
    );

    let Some(source) = ctx.response_source.as_ref() else {
        out.push_str("_No recorded response reader wired — evaluation unavailable._\n");
        return out;
    };
    let Some(step) = step_context else {
        out.push_str("_No enclosing step for this JSONPath literal — skipping evaluation._\n");
        return out;
    };
    if step.step.is_empty() {
        out.push_str(
            "_Enclosing step has no recoverable `name:` — cannot look up the sidecar response._\n",
        );
        return out;
    }
    let Some(response) = source.read(&ctx.file_path, &step.test, &step.step) else {
        out.push_str(
            "_No recorded response found for this step — run the step at least once to populate the sidecar._\n",
        );
        return out;
    };

    match evaluate_path(path, &response) {
        Ok(matches) if matches.is_empty() => {
            out.push_str("_Path evaluated successfully but matched no values._\n");
        }
        Ok(matches) => {
            // Single-match results unwrap so "one thing" looks like
            // that one thing; multi-match results emit an array so
            // the document structure is preserved.
            let value = if matches.len() == 1 {
                matches.into_iter().next().unwrap()
            } else {
                serde_json::Value::Array(matches)
            };
            let pretty = serde_json::to_string_pretty(&value)
                .unwrap_or_else(|e| format!("<serialize error: {e}>"));
            let (body, truncated) = truncate_for_hover(&pretty, JSONPATH_RESULT_MAX_LEN);
            out.push_str("```json\n");
            out.push_str(body);
            if !body.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n");
            if truncated {
                out.push_str(&format!(
                    "_Result truncated to {JSONPATH_RESULT_MAX_LEN} characters — full payload available via `tarn.evaluateJsonpath`._\n"
                ));
            }
        }
        Err(JsonPathError::Parse(msg)) => {
            out.push_str(&format!("_JSONPath parse error: {msg}_\n"));
        }
    }

    out
}

/// Truncate `s` to at most `max_len` characters (bytes, since the
/// pretty-printed JSON is ASCII-dominated and hover popups think in
/// display width rather than code points). Returns the truncated
/// slice plus a flag indicating whether truncation occurred.
fn truncate_for_hover(s: &str, max_len: usize) -> (&str, bool) {
    if s.len() <= max_len {
        return (s, false);
    }
    // Snap to the nearest preceding char boundary so the slice is
    // always valid UTF-8 — important when a user's recorded response
    // contains non-ASCII data.
    let mut boundary = max_len;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    (&s[..boundary], true)
}

fn render_env(key: &str, ctx: &HoverContext) -> String {
    if key.is_empty() {
        return env_quick_reference();
    }
    let mut out = String::new();
    out.push_str(&format!("**`env.{key}`**\n\n"));
    match ctx.env.get(key) {
        Some(entry) => {
            let redacted = ctx
                .redacted_env_keys
                .iter()
                .any(|k| k.eq_ignore_ascii_case(key));
            let display_value = if redacted {
                "***"
            } else {
                entry.value.as_str()
            };
            out.push_str(&format!("- Value: `{display_value}`\n"));
            out.push_str(&format!("- Source: {}\n", entry.source.label()));
            if let Some(path) = entry.source.source_file() {
                out.push_str(&format!("- File: `{path}`\n"));
            }
            if let EnvSource::NamedEnvFile { env_name, .. }
            | EnvSource::NamedProfileVars { env_name } = &entry.source
            {
                out.push_str(&format!("- Environment: `{env_name}`\n"));
            }
            out.push_str(&format!(
                "- Redacted: `{}`\n",
                if redacted { "yes" } else { "no" }
            ));
        }
        None => {
            out.push_str(
                "Not declared in any configured environment. Will resolve at runtime from `tarn.env.yaml`, a named env file, the shell, or an inline `env:` block — or fail with `unresolved_template` if none of those provide it.\n",
            );
        }
    }
    out
}

fn env_quick_reference() -> String {
    let mut s = String::new();
    s.push_str("**`{{ env.KEY }}`**\n\n");
    s.push_str("Resolves `KEY` from the env resolution chain:\n\n");
    s.push_str("1. `--var KEY=VALUE` on the CLI\n");
    s.push_str("2. `tarn.env.local.yaml`\n");
    s.push_str("3. `tarn.env.{active}.yaml`\n");
    s.push_str("4. shell environment `${VAR}` expansion\n");
    s.push_str("5. `tarn.env.yaml`\n");
    s.push_str("6. inline `env:` block in this test file\n");
    s
}

fn render_capture(name: &str, ctx: &HoverContext) -> String {
    if name.is_empty() {
        let mut s = String::new();
        s.push_str("**`{{ capture.NAME }}`**\n\n");
        s.push_str(
            "Resolves `NAME` from the captures accumulated by earlier steps in the same test (plus any setup captures).",
        );
        return s;
    }
    if ctx.parse_errored {
        return format!(
            "**`capture.{name}`**\n\n(capture `{name}` not resolvable — document has parse errors)"
        );
    }
    let mut out = format!("**`capture.{name}`**\n\n");
    let matches: Vec<&VisibleCapture> = ctx
        .visible_captures
        .iter()
        .filter(|c| c.name == name)
        .collect();
    if matches.is_empty() {
        // Check whether the name exists elsewhere in the file for a more
        // helpful "out of scope" vs "undefined" distinction.
        let elsewhere: Vec<&VisibleCapture> =
            ctx.all_captures.iter().filter(|c| c.name == name).collect();
        if elsewhere.is_empty() {
            out.push_str(&format!(
                "Not captured by any step visible from this position. Check that an earlier step declares `capture: {{ {name}: ... }}`.\n",
            ));
        } else {
            out.push_str(
                "Declared elsewhere in this file but not visible from here. Captures are only in scope within the same test (plus setup). Declared by:\n\n",
            );
            for cap in &elsewhere {
                out.push_str(&format_capture_line(cap));
            }
        }
        return out;
    }
    out.push_str("Captured by:\n\n");
    for cap in &matches {
        out.push_str(&format_capture_line(cap));
    }
    if matches.len() > 1 {
        out.push_str(
            "\n_Later declarations override earlier ones when the runner merges captures._\n",
        );
    }
    out
}

fn format_capture_line(cap: &VisibleCapture) -> String {
    let scope = match (&cap.phase, &cap.test_name) {
        (CapturePhase::Setup, _) => "setup".to_owned(),
        (CapturePhase::Teardown, _) => "teardown".to_owned(),
        (CapturePhase::FlatSteps, _) => "flat steps".to_owned(),
        (CapturePhase::Test, Some(name)) => format!("test `{name}`"),
        (CapturePhase::Test, None) => "this file".to_owned(),
    };
    format!(
        "- step `{}` (index {}, {}) — source: {}\n",
        cap.step_name, cap.step_index, scope, cap.source
    )
}

fn render_builtin(name: &str) -> String {
    if name.is_empty() {
        let mut s = String::new();
        s.push_str("**`{{ $builtin }}`**\n\n");
        s.push_str("Tarn built-in functions:\n\n");
        for fn_ in BUILTIN_DOCS {
            s.push_str(&format!("- `{}` — {}\n", fn_.signature, fn_.doc));
        }
        return s;
    }
    match BUILTIN_DOCS.iter().find(|b| b.name == name) {
        Some(fn_) => format!("**`{}`**\n\n{}", fn_.signature, fn_.doc),
        None => {
            let known: Vec<String> = BUILTIN_DOCS
                .iter()
                .map(|b| format!("`{}`", b.signature))
                .collect();
            format!(
                "**`${name}`**\n\nNot a recognized Tarn built-in. Known functions: {}.",
                known.join(", ")
            )
        }
    }
}

fn render_schema_key(key: &str, ctx: &HoverContext) -> String {
    let mut out = format!("**`{key}`**\n\n");
    match ctx.schema_keys.get(key) {
        Some(desc) => out.push_str(desc),
        None => out.push_str(&format!(
            "`{key}` is a Tarn test file field. See `schemas/v1/testfile.json` for its full definition."
        )),
    }
    out
}

/// Build a [`HoverContext`] for the given document + cursor position.
///
/// Every field is lazily computed: if the document fails to parse we set
/// `parse_errored = true`, leave capture lists empty, and return the rest
/// of the context so env / builtin / schema hovers still render.
pub fn build_hover_context(
    source: &str,
    uri: &Url,
    position: Position,
    schema_keys: &HashMap<String, String>,
    response_source: Option<Arc<dyn RecordedResponseSource>>,
) -> HoverContext {
    let path = uri_to_path(uri);
    let parse_result = parser::parse_str(source, &path);
    let (visible_captures, all_captures, inline_env, redacted_env_keys, parse_errored) =
        match &parse_result {
            Ok(test_file) => {
                let cursor_line_one_based = (position.line as usize) + 1;
                let visible = collect_visible_captures(test_file, cursor_line_one_based);
                let all = collect_all_captures(test_file);
                let inline = test_file.env.clone();
                let redacted = test_file
                    .redaction
                    .as_ref()
                    .map(|r| r.env_vars.iter().map(|k| k.to_ascii_lowercase()).collect())
                    .unwrap_or_default();
                (visible, all, inline, redacted, false)
            }
            Err(_) => (Vec::new(), Vec::new(), HashMap::new(), Vec::new(), true),
        };
    // Env resolution is cheap and works even when the inline env block
    // is empty, so we always run it. Errors are swallowed — the hover
    // simply shows "no entries" rather than crashing.
    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let env = env::resolve_env_with_sources(
        &inline_env,
        None,
        &[],
        &base_dir,
        "tarn.env.yaml",
        &HashMap::new(),
    )
    .unwrap_or_default();

    HoverContext {
        env,
        redacted_env_keys,
        visible_captures,
        schema_keys: schema_keys.clone(),
        parse_errored,
        all_captures,
        response_source,
        file_path: path,
    }
}

fn uri_to_path(uri: &Url) -> std::path::PathBuf {
    uri.to_file_path()
        .unwrap_or_else(|_| std::path::PathBuf::from(uri.path()))
}

/// The `textDocument/hover` request entry point.
///
/// Returns `None` (reported to the client as `Ok(None)`) when the cursor
/// does not resolve to a known token or when the URI is not currently
/// tracked in `store`. Does not error for missing documents — LSP clients
/// occasionally send hover for a stale buffer and expect a silent `null`.
pub fn text_document_hover(store: &DocumentStore, uri: &Url, position: Position) -> Option<Hover> {
    if !is_tarn_file_uri(uri) {
        return None;
    }
    let source = store.get(uri)?;
    let span = resolve_hover_token(source, position)?;
    let schema_keys = schema_key_descriptions();
    let response_source: Option<Arc<dyn RecordedResponseSource>> =
        Some(Arc::new(DiskResponseSource));
    let ctx = build_hover_context(source, uri, position, schema_keys, response_source);
    Some(hover_for_token(&span, &ctx))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // resolve_hover_token — pure classifier
    // ---------------------------------------------------------------------

    fn find_token(source: &str, line: u32, col: u32) -> Option<HoverToken> {
        resolve_hover_token(source, Position::new(line, col)).map(|s| s.token)
    }

    #[test]
    fn cursor_inside_env_interpolation_returns_env_token() {
        let src = "url: {{ env.base_url }}\n";
        let token = find_token(src, 0, 12).unwrap();
        assert_eq!(token, HoverToken::Env("base_url".to_owned()));
    }

    #[test]
    fn cursor_on_opening_braces_returns_env_token() {
        let src = "url: {{ env.base_url }}\n";
        // On the first `{` character.
        let token = find_token(src, 0, 5).unwrap();
        assert_eq!(token, HoverToken::Env("base_url".to_owned()));
    }

    #[test]
    fn cursor_on_closing_braces_returns_env_token() {
        let src = "url: {{ env.base_url }}\n";
        // On the last `}` character.
        let token = find_token(src, 0, 22).unwrap();
        assert_eq!(token, HoverToken::Env("base_url".to_owned()));
    }

    #[test]
    fn cursor_on_dot_separator_returns_env_token() {
        let src = "url: {{ env.base_url }}\n";
        // On the `.` between `env` and `base_url`.
        let token = find_token(src, 0, 11).unwrap();
        assert_eq!(token, HoverToken::Env("base_url".to_owned()));
    }

    #[test]
    fn cursor_in_capture_interpolation_returns_capture_token() {
        let src = "header: {{ capture.token }}\n";
        let token = find_token(src, 0, 20).unwrap();
        assert_eq!(token, HoverToken::Capture("token".to_owned()));
    }

    #[test]
    fn cursor_in_builtin_no_args_returns_builtin_token() {
        let src = "id: {{ $uuid }}\n";
        let token = find_token(src, 0, 9).unwrap();
        assert_eq!(token, HoverToken::Builtin("uuid".to_owned()));
    }

    #[test]
    fn cursor_in_builtin_with_args_returns_function_name_only() {
        let src = "n: {{ $random_hex(8) }}\n";
        let token = find_token(src, 0, 8).unwrap();
        assert_eq!(token, HoverToken::Builtin("random_hex".to_owned()));
    }

    #[test]
    fn cursor_outside_any_token_returns_none() {
        let src = "url: plain-text\n";
        assert!(find_token(src, 0, 8).is_none());
    }

    #[test]
    fn multiline_interpolation_with_closing_on_next_line_still_resolves() {
        // `{{` on line 0, `}}` on line 1 with the whole identifier on
        // line 0. The classifier's scan walks raw bytes, so it tolerates
        // a newline between the end of the identifier and the closing
        // `}}` — a shape that's rare but legal in YAML block scalars.
        let src = "url: {{ env.base_url\n  }}\nrest: yes\n";
        let token = find_token(src, 0, 12).unwrap();
        assert_eq!(token, HoverToken::Env("base_url".to_owned()));
    }

    #[test]
    fn unclosed_interpolation_returns_none() {
        let src = "url: {{ env.base_url\nrest: yes\n";
        assert!(find_token(src, 0, 12).is_none());
    }

    #[test]
    fn second_interpolation_on_same_line_resolves() {
        let src = "url: {{ env.a }}-{{ capture.b }}\n";
        assert_eq!(
            find_token(src, 0, 25),
            Some(HoverToken::Capture("b".to_owned()))
        );
    }

    #[test]
    fn cursor_in_whitespace_between_key_and_interpolation_returns_none() {
        // Column 4 is the space between the `url:` key and the `{{`.
        // Schema key classification only matches on the bare key (cols
        // 0–3 for `url`), and the interpolation classifier starts at
        // col 5 — so col 4 should fall through with no match.
        let src = "url: {{ env.x }}\n";
        assert!(find_token(src, 0, 4).is_none());
    }

    #[test]
    fn empty_interpolation_resolves_to_none() {
        let src = "x: {{}}\n";
        assert!(find_token(src, 0, 5).is_none());
    }

    #[test]
    fn schema_key_on_bare_status_line_classifies() {
        let src = "assert:\n  status: 200\n";
        let span = resolve_hover_token(src, Position::new(1, 4)).unwrap();
        assert_eq!(span.token, HoverToken::SchemaKey("status".to_owned()));
    }

    #[test]
    fn schema_key_unknown_key_returns_none() {
        let src = "not_a_schema_key: 1\n";
        assert!(resolve_hover_token(src, Position::new(0, 3)).is_none());
    }

    #[test]
    fn schema_key_inside_indented_top_level_capture_classifies() {
        let src = "steps:\n  - name: x\n    capture:\n      token: $.id\n";
        let span = resolve_hover_token(src, Position::new(2, 6)).unwrap();
        assert_eq!(span.token, HoverToken::SchemaKey("capture".to_owned()));
    }

    // ---------------------------------------------------------------------
    // hover_for_token — pure renderer
    // ---------------------------------------------------------------------

    fn dummy_span(token: HoverToken) -> HoverTokenSpan {
        HoverTokenSpan {
            token,
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            step_context: None,
        }
    }

    fn dummy_span_with_step(token: HoverToken, step: StepContext) -> HoverTokenSpan {
        HoverTokenSpan {
            token,
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            step_context: Some(step),
        }
    }

    fn body_of(hover: &Hover) -> &str {
        match &hover.contents {
            HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }) => value.as_str(),
            _ => panic!("expected markdown hover"),
        }
    }

    #[test]
    fn env_hover_renders_value_source_file_and_redaction_flag() {
        let mut env = BTreeMap::new();
        env.insert(
            "base_url".to_owned(),
            EnvEntry {
                value: "http://localhost:3000".to_owned(),
                source: EnvSource::DefaultEnvFile {
                    path: "/proj/tarn.env.yaml".to_owned(),
                },
                declaration_range: None,
            },
        );
        let ctx = HoverContext {
            env,
            redacted_env_keys: vec!["secret".to_owned()],
            ..HoverContext::default()
        };
        let hover = hover_for_token(&dummy_span(HoverToken::Env("base_url".to_owned())), &ctx);
        let body = body_of(&hover);
        assert!(body.contains("`env.base_url`"));
        assert!(body.contains("http://localhost:3000"));
        assert!(body.contains("/proj/tarn.env.yaml"));
        assert!(body.contains("Redacted: `no`"));
    }

    #[test]
    fn env_hover_for_redacted_key_hides_value_and_sets_flag() {
        let mut env = BTreeMap::new();
        env.insert(
            "api_key".to_owned(),
            EnvEntry {
                value: "super-secret".to_owned(),
                source: EnvSource::LocalEnvFile {
                    path: "/proj/tarn.env.local.yaml".to_owned(),
                },
                declaration_range: None,
            },
        );
        let ctx = HoverContext {
            env,
            redacted_env_keys: vec!["api_key".to_owned()],
            ..HoverContext::default()
        };
        let hover = hover_for_token(&dummy_span(HoverToken::Env("api_key".to_owned())), &ctx);
        let body = body_of(&hover);
        assert!(body.contains("Value: `***`"));
        assert!(!body.contains("super-secret"));
        assert!(body.contains("Redacted: `yes`"));
        assert!(body.contains("tarn.env.local.yaml"));
    }

    #[test]
    fn env_hover_for_undeclared_key_shows_unresolved_template_hint() {
        let ctx = HoverContext::default();
        let hover = hover_for_token(&dummy_span(HoverToken::Env("missing".to_owned())), &ctx);
        let body = body_of(&hover);
        assert!(body.contains("Not declared in any configured environment"));
        assert!(body.contains("unresolved_template"));
    }

    #[test]
    fn env_hover_with_empty_key_shows_quick_reference() {
        let ctx = HoverContext::default();
        let hover = hover_for_token(&dummy_span(HoverToken::Env(String::new())), &ctx);
        let body = body_of(&hover);
        assert!(body.contains("env resolution chain"));
        assert!(body.contains("tarn.env.local.yaml"));
    }

    #[test]
    fn capture_hover_renders_declaring_step_and_source() {
        let ctx = HoverContext {
            visible_captures: vec![VisibleCapture {
                name: "token".to_owned(),
                step_name: "login".to_owned(),
                step_index: 0,
                phase: CapturePhase::Test,
                test_name: Some("auth".to_owned()),
                source: "JSONPath `$.token`".to_owned(),
            }],
            ..HoverContext::default()
        };
        let hover = hover_for_token(&dummy_span(HoverToken::Capture("token".to_owned())), &ctx);
        let body = body_of(&hover);
        assert!(body.contains("`capture.token`"));
        assert!(body.contains("step `login`"));
        assert!(body.contains("test `auth`"));
        assert!(body.contains("JSONPath `$.token`"));
    }

    #[test]
    fn capture_hover_for_unknown_name_shows_hint() {
        let ctx = HoverContext::default();
        let hover = hover_for_token(&dummy_span(HoverToken::Capture("missing".to_owned())), &ctx);
        let body = body_of(&hover);
        assert!(body.contains("Not captured by any step"));
    }

    #[test]
    fn capture_hover_distinguishes_out_of_scope_from_undefined() {
        let ctx = HoverContext {
            visible_captures: vec![],
            all_captures: vec![VisibleCapture {
                name: "token".to_owned(),
                step_name: "login".to_owned(),
                step_index: 0,
                phase: CapturePhase::Test,
                test_name: Some("other_test".to_owned()),
                source: "JSONPath `$.token`".to_owned(),
            }],
            ..HoverContext::default()
        };
        let hover = hover_for_token(&dummy_span(HoverToken::Capture("token".to_owned())), &ctx);
        let body = body_of(&hover);
        assert!(body.contains("Declared elsewhere"));
        assert!(body.contains("other_test"));
    }

    #[test]
    fn capture_hover_degrades_gracefully_on_parse_error() {
        let ctx = HoverContext {
            parse_errored: true,
            ..HoverContext::default()
        };
        let hover = hover_for_token(&dummy_span(HoverToken::Capture("token".to_owned())), &ctx);
        let body = body_of(&hover);
        assert!(body.contains("parse errors"));
    }

    #[test]
    fn builtin_hover_renders_signature_and_doc_for_known_function() {
        let ctx = HoverContext::default();
        let hover = hover_for_token(&dummy_span(HoverToken::Builtin("uuid".to_owned())), &ctx);
        let body = body_of(&hover);
        assert!(body.contains("`$uuid`"));
        assert!(body.contains("UUID v4"));
    }

    #[test]
    fn builtin_hover_for_random_hex_has_signature_with_args() {
        let ctx = HoverContext::default();
        let hover = hover_for_token(
            &dummy_span(HoverToken::Builtin("random_hex".to_owned())),
            &ctx,
        );
        let body = body_of(&hover);
        assert!(body.contains("`$random_hex(n)`"));
        assert!(body.contains("hexadecimal"));
    }

    #[test]
    fn builtin_hover_for_unknown_function_lists_known_ones() {
        let ctx = HoverContext::default();
        let hover = hover_for_token(
            &dummy_span(HoverToken::Builtin("not_real".to_owned())),
            &ctx,
        );
        let body = body_of(&hover);
        assert!(body.contains("Not a recognized Tarn built-in"));
        assert!(body.contains("`$uuid`"));
        assert!(body.contains("`$random_hex(n)`"));
    }

    #[test]
    fn schema_key_hover_uses_description_from_schema_map() {
        let mut schema_keys = HashMap::new();
        schema_keys.insert("status".to_owned(), "Expected HTTP status code".to_owned());
        let ctx = HoverContext {
            schema_keys,
            ..HoverContext::default()
        };
        let hover = hover_for_token(
            &dummy_span(HoverToken::SchemaKey("status".to_owned())),
            &ctx,
        );
        let body = body_of(&hover);
        assert!(body.contains("`status`"));
        assert!(body.contains("Expected HTTP status code"));
    }

    #[test]
    fn hover_always_returns_markdown_kind() {
        let ctx = HoverContext::default();
        let hover = hover_for_token(&dummy_span(HoverToken::Builtin("uuid".to_owned())), &ctx);
        match hover.contents {
            HoverContents::Markup(m) => assert_eq!(m.kind, MarkupKind::Markdown),
            other => panic!("expected markup, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------------
    // schema_key_descriptions — loaded once at startup
    // ---------------------------------------------------------------------

    #[test]
    fn schema_descriptions_include_every_documented_top_level_key() {
        let map = schema_key_descriptions();
        for key in &[
            "name",
            "env",
            "defaults",
            "setup",
            "teardown",
            "tests",
            "steps",
            "cookies",
            "redaction",
        ] {
            assert!(map.contains_key(*key), "schema missing `{key}`");
        }
    }

    #[test]
    fn schema_descriptions_include_nested_assertion_keys() {
        let map = schema_key_descriptions();
        assert!(map.contains_key("status"));
        assert!(map.contains_key("body"));
        assert!(map.contains_key("headers"));
    }

    // ---------------------------------------------------------------------
    // collect_visible_captures — AST walker
    // ---------------------------------------------------------------------

    fn parse_fixture(source: &str) -> TestFile {
        parser::parse_str(source, Path::new("fixture.tarn.yaml")).expect("fixture should parse")
    }

    #[test]
    fn visible_captures_within_test_sees_earlier_steps_only() {
        let source = r#"name: cap-scope
tests:
  auth:
    steps:
      - name: login
        request:
          method: POST
          url: http://x/login
        capture:
          token: $.token
      - name: use-token
        request:
          method: GET
          url: http://x/me
        assert:
          status: 200
      - name: logout
        request:
          method: POST
          url: http://x/logout
        capture:
          bye: $.bye
"#;
        let tf = parse_fixture(source);
        // Cursor on the "use-token" step (2nd step inside auth, its name
        // line in the source). Its step.location line.
        let use_token_line = tf.tests["auth"].steps[1].location.as_ref().unwrap().line;
        let visible = collect_visible_captures(&tf, use_token_line + 1);
        let names: Vec<_> = visible.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["token"]);
    }

    #[test]
    fn visible_captures_setup_visible_everywhere() {
        let source = r#"name: cap-setup
setup:
  - name: bootstrap
    request:
      method: GET
      url: http://x/boot
    capture:
      boot_id: $.id
tests:
  first:
    steps:
      - name: use-setup
        request:
          method: GET
          url: http://x/use
        assert:
          status: 200
"#;
        let tf = parse_fixture(source);
        let use_line = tf.tests["first"].steps[0].location.as_ref().unwrap().line;
        let visible = collect_visible_captures(&tf, use_line + 1);
        assert!(visible.iter().any(|c| c.name == "boot_id"));
    }

    #[test]
    fn visible_captures_flat_steps_mode() {
        let source = r#"name: flat
steps:
  - name: one
    request:
      method: GET
      url: http://x
    capture:
      a: $.a
  - name: two
    request:
      method: GET
      url: http://x
    capture:
      b: $.b
  - name: three
    request:
      method: GET
      url: http://x
"#;
        let tf = parse_fixture(source);
        let third_line = tf.steps[2].location.as_ref().unwrap().line;
        let visible = collect_visible_captures(&tf, third_line + 1);
        let names: Vec<_> = visible.iter().map(|c| c.name.clone()).collect();
        assert!(names.contains(&"a".to_owned()));
        assert!(names.contains(&"b".to_owned()));
    }

    #[test]
    fn collect_all_captures_includes_cross_test_and_setup() {
        let source = r#"name: all-caps
setup:
  - name: s
    request:
      method: GET
      url: http://x
    capture:
      setup_cap: $.x
tests:
  first:
    steps:
      - name: f
        request:
          method: GET
          url: http://x
        capture:
          first_cap: $.f
  second:
    steps:
      - name: g
        request:
          method: GET
          url: http://x
        capture:
          second_cap: $.g
"#;
        let tf = parse_fixture(source);
        let all = collect_all_captures(&tf);
        let names: Vec<_> = all.iter().map(|c| c.name.clone()).collect();
        assert!(names.contains(&"setup_cap".to_owned()));
        assert!(names.contains(&"first_cap".to_owned()));
        assert!(names.contains(&"second_cap".to_owned()));
    }

    // ---------------------------------------------------------------------
    // resolve_jsonpath_at_position + render_jsonpath_literal — L3.6 hover
    // ---------------------------------------------------------------------

    /// Fixture: a named test whose step asserts on a JSONPath key
    /// inside `assert.body`. The cursor sits on the JSONPath key.
    const JSONPATH_ASSERT_FIXTURE: &str = "name: fixture
tests:
  main:
    steps:
      - name: list items
        request:
          method: GET
          url: http://example.com/items
        assert:
          body:
            \"$.data[0].id\":
              eq: 5
";

    /// Fixture: a capture block using a bare JSONPath shorthand.
    const JSONPATH_CAPTURE_FIXTURE: &str = "name: fixture
tests:
  main:
    steps:
      - name: cap
        request:
          method: GET
          url: http://example.com/thing
        capture:
          token: $.token
";

    #[test]
    fn resolve_jsonpath_classifies_cursor_on_assert_body_key() {
        let src = JSONPATH_ASSERT_FIXTURE;
        // Cursor on the `$.data[0].id` key line (0-based line 10,
        // column 14 lands inside the quoted key).
        let span =
            resolve_hover_token(src, Position::new(10, 14)).expect("should classify JSONPath");
        assert!(matches!(span.token, HoverToken::JsonPathLiteral(ref s) if s == "$.data[0].id"));
        let step = span.step_context.as_ref().expect("step context required");
        assert_eq!(step.test, "main");
        assert_eq!(step.step, "list items");
    }

    #[test]
    fn resolve_jsonpath_classifies_cursor_on_capture_value() {
        let src = JSONPATH_CAPTURE_FIXTURE;
        // `token: $.token` — cursor on the `$` char.
        let span = resolve_hover_token(src, Position::new(9, 17))
            .expect("should classify JSONPath from capture value");
        assert!(matches!(span.token, HoverToken::JsonPathLiteral(ref s) if s == "$.token"));
        let step = span.step_context.as_ref().expect("step context required");
        assert_eq!(step.test, "main");
        assert_eq!(step.step, "cap");
    }

    #[test]
    fn resolve_jsonpath_declines_non_jsonpath_scalar() {
        // A plain URL is not a JSONPath literal even though it is a
        // valid YAML scalar under a request field.
        let src = JSONPATH_ASSERT_FIXTURE;
        // Cursor on the `http://example.com/items` URL.
        let span = resolve_hover_token(src, Position::new(7, 20));
        // The URL line *might* classify to a schema-key or return None;
        // the only thing this test cares about is that it is not
        // mis-classified as a JSONPath literal.
        if let Some(span) = span {
            assert!(
                !matches!(span.token, HoverToken::JsonPathLiteral(_)),
                "URL must not be classified as JSONPath literal, got {:?}",
                span.token
            );
        }
    }

    #[test]
    fn render_jsonpath_literal_with_matching_response_inlines_result() {
        use crate::code_actions::response_source::InMemoryResponseSource;
        let response = serde_json::json!({"data": [{"id": 42}, {"id": 7}]});
        let ctx = HoverContext {
            response_source: Some(Arc::new(InMemoryResponseSource::new(response))),
            file_path: std::path::PathBuf::from("/tmp/fixture.tarn.yaml"),
            ..HoverContext::default()
        };
        let span = dummy_span_with_step(
            HoverToken::JsonPathLiteral("$.data[0].id".to_owned()),
            StepContext {
                test: "main".to_owned(),
                step: "list items".to_owned(),
            },
        );
        let hover = hover_for_token(&span, &ctx);
        let body = body_of(&hover);
        assert!(body.contains("JSONPath literal"));
        assert!(body.contains("`$.data[0].id`"));
        assert!(body.contains("```json"));
        // Single match unwraps to the raw integer, not wrapped in an array.
        assert!(
            body.contains("42"),
            "body missing evaluation result: {body}"
        );
    }

    #[test]
    fn render_jsonpath_literal_with_no_response_falls_back_gracefully() {
        use crate::code_actions::response_source::InMemoryResponseSource;
        let ctx = HoverContext {
            response_source: Some(Arc::new(InMemoryResponseSource::empty())),
            file_path: std::path::PathBuf::from("/tmp/fixture.tarn.yaml"),
            ..HoverContext::default()
        };
        let span = dummy_span_with_step(
            HoverToken::JsonPathLiteral("$.data[0].id".to_owned()),
            StepContext {
                test: "main".to_owned(),
                step: "list items".to_owned(),
            },
        );
        let hover = hover_for_token(&span, &ctx);
        let body = body_of(&hover);
        // Never crash: heading must still render.
        assert!(body.contains("JSONPath literal"));
        assert!(body.contains("`$.data[0].id`"));
        assert!(body.contains("No recorded response"));
        // Must not contain a code block since no evaluation ran.
        assert!(!body.contains("```json"));
    }

    #[test]
    fn render_jsonpath_literal_with_parse_error_reports_it() {
        use crate::code_actions::response_source::InMemoryResponseSource;
        let response = serde_json::json!({"x": 1});
        let ctx = HoverContext {
            response_source: Some(Arc::new(InMemoryResponseSource::new(response))),
            file_path: std::path::PathBuf::from("/tmp/fixture.tarn.yaml"),
            ..HoverContext::default()
        };
        let span = dummy_span_with_step(
            HoverToken::JsonPathLiteral("$.[not valid]".to_owned()),
            StepContext {
                test: "main".to_owned(),
                step: "list items".to_owned(),
            },
        );
        let hover = hover_for_token(&span, &ctx);
        let body = body_of(&hover);
        assert!(body.contains("JSONPath parse error"));
        assert!(!body.contains("```json"));
    }

    #[test]
    fn render_jsonpath_literal_truncates_very_long_result() {
        use crate::code_actions::response_source::InMemoryResponseSource;
        // Build a response whose pretty-printed form is well over the
        // 2000-char cap.
        let mut big = serde_json::Map::new();
        for i in 0..500 {
            big.insert(format!("key{i}"), serde_json::json!(format!("value{i}")));
        }
        let response = serde_json::json!({ "big": big });
        let ctx = HoverContext {
            response_source: Some(Arc::new(InMemoryResponseSource::new(response))),
            file_path: std::path::PathBuf::from("/tmp/fixture.tarn.yaml"),
            ..HoverContext::default()
        };
        let span = dummy_span_with_step(
            HoverToken::JsonPathLiteral("$.big".to_owned()),
            StepContext {
                test: "main".to_owned(),
                step: "list items".to_owned(),
            },
        );
        let hover = hover_for_token(&span, &ctx);
        let body = body_of(&hover);
        assert!(body.contains("truncated"));
        // Cap enforcement: the entire body should still be finite, and
        // the JSON block should be capped.
        assert!(body.len() < 3000, "body too long: {} chars", body.len());
    }

    #[test]
    fn render_jsonpath_literal_multi_match_serialises_as_array() {
        use crate::code_actions::response_source::InMemoryResponseSource;
        let response = serde_json::json!({"items": [{"id": 1}, {"id": 2}, {"id": 3}]});
        let ctx = HoverContext {
            response_source: Some(Arc::new(InMemoryResponseSource::new(response))),
            file_path: std::path::PathBuf::from("/tmp/fixture.tarn.yaml"),
            ..HoverContext::default()
        };
        let span = dummy_span_with_step(
            HoverToken::JsonPathLiteral("$.items[*].id".to_owned()),
            StepContext {
                test: "main".to_owned(),
                step: "list items".to_owned(),
            },
        );
        let hover = hover_for_token(&span, &ctx);
        let body = body_of(&hover);
        assert!(body.contains("```json"));
        // Three ids inline.
        assert!(body.contains("1"));
        assert!(body.contains("2"));
        assert!(body.contains("3"));
    }

    #[test]
    fn render_jsonpath_literal_without_response_source_shows_unavailable() {
        let ctx = HoverContext {
            response_source: None,
            file_path: std::path::PathBuf::from("/tmp/fixture.tarn.yaml"),
            ..HoverContext::default()
        };
        let span = dummy_span_with_step(
            HoverToken::JsonPathLiteral("$.foo".to_owned()),
            StepContext {
                test: "main".to_owned(),
                step: "step".to_owned(),
            },
        );
        let hover = hover_for_token(&span, &ctx);
        let body = body_of(&hover);
        assert!(body.contains("JSONPath literal"));
        assert!(body.contains("reader wired"));
        assert!(!body.contains("```json"));
    }

    #[test]
    fn existing_hover_classes_still_work_after_jsonpath_addition() {
        // Regression test: the four pre-existing classes (env, capture,
        // builtin, schema-key) must still classify correctly. Exercise
        // each one with the token classifier.
        let env_src = "url: {{ env.base_url }}\n";
        assert!(matches!(
            find_token(env_src, 0, 12),
            Some(HoverToken::Env(ref s)) if s == "base_url"
        ));
        let cap_src = "header: {{ capture.token }}\n";
        assert!(matches!(
            find_token(cap_src, 0, 20),
            Some(HoverToken::Capture(ref s)) if s == "token"
        ));
        let bi_src = "id: {{ $uuid }}\n";
        assert!(matches!(
            find_token(bi_src, 0, 9),
            Some(HoverToken::Builtin(ref s)) if s == "uuid"
        ));
        let sk_src = "assert:\n  status: 200\n";
        let span = resolve_hover_token(sk_src, Position::new(1, 4)).unwrap();
        assert!(matches!(span.token, HoverToken::SchemaKey(ref s) if s == "status"));
    }
}
