//! Library-friendly façade over `tarn`'s validation pipeline.
//!
//! This module is the entry point language servers, editors, and other
//! in-process consumers should call when they want to check a `.tarn.yaml`
//! document without spawning `tarn validate` as a subprocess. It reuses the
//! exact same code paths `tarn validate` uses in `main.rs`:
//!
//!   1. YAML syntactic check via `serde_yaml::from_str::<serde_yaml::Value>`.
//!      If the raw YAML cannot even parse, we surface the error with the
//!      line/column reported by `serde_yaml::Error::location`.
//!   2. Semantic parse via [`crate::parser::parse_str`], which runs the full
//!      shape + schema + cross-field validation pipeline and, on success,
//!      attaches NAZ-260 `Location` metadata to every step and assertion.
//!
//! Today Tarn's parser is single-error — once a check trips, it returns early.
//! The returned `Vec<ValidationMessage>` therefore contains at most one
//! message in the current implementation, but the shape is future-proofed so
//! a later pass can accumulate multiple diagnostics without breaking callers.
//!
//! The message shape (severity, code, human message, optional
//! [`Location`](crate::model::Location)) is deliberately decoupled from
//! [`crate::error::TarnError`] so LSP / editor consumers never have to match
//! on a Rust enum.

use std::path::{Path, PathBuf};

use crate::error::TarnError;
use crate::model::Location;
use crate::parser;

/// Severity tag carried on every [`ValidationMessage`].
///
/// Today every message emitted by [`validate_document`] is an `Error` — Tarn's
/// parser does not currently produce soft warnings. The variant exists so the
/// LSP diagnostics pipeline can already map severities correctly the moment a
/// follow-up ticket (or a future validator) introduces warning-level checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// Stable machine-readable code for a [`ValidationMessage`].
///
/// These strings are chosen to be a stable public contract for downstream
/// consumers (LSP clients, editors, CI pipelines) that want to filter or
/// surface messages programmatically. Renames here are breaking changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationCode {
    /// Raw YAML could not be parsed by `serde_yaml`.
    YamlSyntax,
    /// The document parsed as YAML but failed `tarn::parser::parse_str`'s
    /// structural / schema checks (unknown fields, wrong types, etc).
    TarnParse,
    /// The document parsed as YAML and matched the schema, but failed one of
    /// Tarn's cross-field semantic validations (e.g. step with both
    /// `body:` and `graphql:`).
    TarnValidation,
    /// The document is structurally valid, but uses a pattern that is
    /// known to be brittle against shared or persistent environments
    /// (exact array lengths on list endpoints, `$[0]` captures, static
    /// unique-looking identifiers, etc.). Emitted as a warning so
    /// `tarn validate` still succeeds but editors can surface a squiggle.
    BrittlePattern,
}

impl ValidationCode {
    /// Stable string form used in LSP `Diagnostic.code` and similar surfaces.
    pub fn as_str(&self) -> &'static str {
        match self {
            ValidationCode::YamlSyntax => "yaml_syntax",
            ValidationCode::TarnParse => "tarn_parse",
            ValidationCode::TarnValidation => "tarn_validation",
            ValidationCode::BrittlePattern => "brittle_pattern",
        }
    }
}

/// A single diagnostic produced by [`validate_document`].
///
/// All fields except `location` are guaranteed to be populated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationMessage {
    pub severity: Severity,
    pub code: ValidationCode,
    pub message: String,
    /// Optional 1-based source position. `None` when the underlying validator
    /// could not pinpoint the offending node (e.g. a whole-file "missing
    /// steps or tests" error).
    pub location: Option<Location>,
}

/// Validate an in-memory `.tarn.yaml` document and return every diagnostic
/// the core parser emits.
///
/// The `path` parameter is used solely to tag message locations — the file
/// at that path is **not** read from disk. Callers such as `tarn-lsp` that
/// operate on unsaved buffers pass the canonical LSP URI → `PathBuf` and the
/// buffer text the editor gave them.
///
/// The returned vector is empty when the document is valid. In the current
/// implementation it contains at most one message, but callers must not rely
/// on that invariant — treat it as `Vec<_>` for forward compatibility.
pub fn validate_document(path: &Path, source: &str) -> Vec<ValidationMessage> {
    // Step 1: raw YAML syntax. We intentionally run this even though
    // `parser::parse_str` will run it again internally — the serde_yaml
    // error object is the only place we can get a structured line/column
    // for malformed YAML (parser embeds it in the message string for a
    // single variant but drops it for `invalid type` errors).
    if let Err(yaml_err) = serde_yaml::from_str::<serde_yaml::Value>(source) {
        let location = yaml_err.location().map(|loc| Location {
            file: path.display().to_string(),
            line: loc.line(),
            column: loc.column(),
        });
        return vec![ValidationMessage {
            severity: Severity::Error,
            code: ValidationCode::YamlSyntax,
            message: yaml_err.to_string(),
            location,
        }];
    }

    // Step 2: full parser + semantic validation. This is the same code path
    // `tarn validate` uses, just lifted to operate on a string instead of
    // a file on disk.
    match parser::parse_str(source, path) {
        Ok(test_file) => lint_brittle_patterns(&test_file),
        Err(err) => vec![tarn_error_to_message(path, err)],
    }
}

/// Scan a parsed [`TestFile`] for patterns that make integration tests
/// brittle against shared or persistent environments. These are
/// warnings, not errors — the run will still happen, but editors can
/// surface a squiggle and CI can fail on warnings if the team wants.
///
/// Rules (NAZ-342):
///
/// 1. **Exact array length on a list-shaped JSONPath** —
///    `body: { "$.users": { length: N } }` is brittle whenever `/users`
///    might return shared data. Suggest `length_gte` / `exists_where`.
/// 2. **`$[N]` capture from a list endpoint** — `capture.jsonpath = "$[0].id"`
///    on a `GET /users`-style step binds the test to an array position
///    that shared state can change. Suggest `where:` filtering.
/// 3. **Static UUID / opaque identifier** baked into a request body or
///    URL inside a mutating test. Usually a leftover from copy-pasting
///    an id during debugging and a guaranteed source of cross-run
///    collisions.
fn lint_brittle_patterns(test_file: &crate::model::TestFile) -> Vec<ValidationMessage> {
    let mut messages: Vec<ValidationMessage> = Vec::new();

    let mut visit = |steps: &[crate::model::Step]| {
        for step in steps {
            lint_step(step, &mut messages);
        }
    };
    visit(&test_file.setup);
    visit(&test_file.steps);
    for test in test_file.tests.values() {
        visit(&test.steps);
    }
    visit(&test_file.teardown);

    messages
}

fn lint_step(step: &crate::model::Step, messages: &mut Vec<ValidationMessage>) {
    // Rule 1: `length: N` on a JSONPath that looks like it addresses a
    // whole collection (root `$`, a top-level array field like `$.users`,
    // or a `$[*]` fan-out). Narrower paths (`$.user.friends[0]`) are not
    // flagged because they often address a specific record.
    if let Some(ref assertion) = step.assertions {
        if let Some(ref body) = assertion.body {
            for (jsonpath, value) in body {
                if !looks_like_list_path(jsonpath) {
                    continue;
                }
                if let serde_yaml::Value::Mapping(map) = value {
                    for (k, _) in map {
                        if k.as_str() == Some("length") {
                            messages.push(ValidationMessage {
                                severity: Severity::Warning,
                                code: ValidationCode::BrittlePattern,
                                message: format!(
                                    "Exact array length assertion on `{}` is brittle on shared endpoints. \
                                     Consider `length_gte: N`, `exists_where: {{ ... }}`, or `contains_object: {{ ... }}` to assert by identity instead of count.",
                                    jsonpath
                                ),
                                location: step.location.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    // Rule 2: `$[0]` / `$[N]` captures from list endpoints. Safe to
    // flag even without knowing the URL — positional captures are
    // brittle regardless of the endpoint.
    for (name, spec) in &step.capture {
        let path_str = match spec {
            crate::model::CaptureSpec::JsonPath(s) => Some(s.as_str()),
            crate::model::CaptureSpec::Extended(ext) => ext.jsonpath.as_deref(),
        };
        if let Some(path_str) = path_str {
            if is_positional_array_path(path_str) {
                messages.push(ValidationMessage {
                    severity: Severity::Warning,
                    code: ValidationCode::BrittlePattern,
                    message: format!(
                        "Capture `{}` uses positional index `{}` — shared list endpoints can reorder or grow. \
                         Capture by identity instead, e.g. `jsonpath: \"$.items\"` plus `where: {{ id: \"...\" }}`.",
                        name, path_str
                    ),
                    location: step.location.clone(),
                });
            }
        }
    }

    // Rule 3: static opaque identifiers (UUIDs / long hex) baked into a
    // mutating request's URL or body. Heuristic-only, limited to POST /
    // PATCH / PUT / DELETE so we don't over-warn on `/health/<version>`
    // style read paths where the value is a deliberate constant.
    let method = step.request.method.to_ascii_uppercase();
    let is_mutating = matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE");
    if is_mutating {
        if let Some(ident) = find_static_identifier(&step.request.url) {
            messages.push(ValidationMessage {
                severity: Severity::Warning,
                code: ValidationCode::BrittlePattern,
                message: format!(
                    "Static opaque identifier `{}` embedded in a {method} URL. \
                     Integration runs that re-execute this test will collide — prefer capturing or generating the id with `$uuid`.",
                    ident
                ),
                location: step.location.clone(),
            });
        }
        if let Some(ref body) = step.request.body {
            if let Some(ident) = find_static_identifier_in_json(body) {
                messages.push(ValidationMessage {
                    severity: Severity::Warning,
                    code: ValidationCode::BrittlePattern,
                    message: format!(
                        "Static opaque identifier `{}` embedded in a {method} request body. \
                         Consider `{{{{ $uuid }}}}` or a captured id so parallel/repeated runs don't collide.",
                        ident
                    ),
                    location: step.location.clone(),
                });
            }
        }
    }
}

/// A JSONPath is treated as "addresses a whole list" when it's the root
/// `$`, a bare top-level field (`$.users`, `$.items`), or ends in a
/// wildcard fan-out (`$.users[*]`). These are the paths where a shared
/// endpoint's count realistically varies between runs.
fn looks_like_list_path(path: &str) -> bool {
    let trimmed = path.trim();
    if trimmed == "$" {
        return true;
    }
    if trimmed.ends_with("[*]") {
        return true;
    }
    // `$.foo` or `$.foo.bar` (no bracket indexing, at most two `.` levels)
    // — treat as list-shaped candidates. Paths that already filter to a
    // specific record (`$.items[?(@.id == 'x')]`) are identity-based,
    // not list-based, so they are not flagged.
    if let Some(rest) = trimmed.strip_prefix("$.") {
        if !rest.contains('[') && !rest.contains('?') && !rest.contains('*') {
            return true;
        }
    }
    false
}

fn is_positional_array_path(path: &str) -> bool {
    // Matches the `$[0]`, `$[12]`, `$.items[0].id` style. We deliberately
    // do not flag `$[*]` (wildcard) or `$[?(...)]` (filter predicate),
    // since those are identity-based selections.
    let trimmed = path.trim();
    let mut chars = trimmed.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '[' {
            let mut inner = String::new();
            for &next in chars.clone().collect::<Vec<_>>().iter() {
                if next == ']' {
                    break;
                }
                inner.push(next);
                chars.next();
            }
            if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()) {
                return true;
            }
        }
    }
    false
}

fn find_static_identifier(s: &str) -> Option<String> {
    // Avoid double-flagging templated values — `{{ capture.x }}` would
    // contain long strings but they're dynamic.
    if s.contains("{{") {
        return None;
    }
    // UUID-ish: 8-4-4-4-12 hex groups.
    static UUID_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let uuid = UUID_RE.get_or_init(|| {
        regex::Regex::new(
            r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
        )
        .expect("valid UUID regex")
    });
    if let Some(m) = uuid.find(s) {
        return Some(m.as_str().to_string());
    }
    // Long opaque hex blob (≥ 24 chars) — stripe/sendgrid-style tokens.
    static HEX_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let hex =
        HEX_RE.get_or_init(|| regex::Regex::new(r"\b[0-9a-fA-F]{24,}\b").expect("valid hex regex"));
    if let Some(m) = hex.find(s) {
        return Some(m.as_str().to_string());
    }
    None
}

fn find_static_identifier_in_json(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => find_static_identifier(s),
        serde_json::Value::Array(arr) => arr.iter().find_map(find_static_identifier_in_json),
        serde_json::Value::Object(obj) => obj.values().find_map(find_static_identifier_in_json),
        _ => None,
    }
}

/// Convert a [`TarnError`] produced by [`parser::parse_str`] into a
/// [`ValidationMessage`]. Exposed at crate level so the handful of other
/// call-sites that already hold a `TarnError` (for example the `tarn validate`
/// CLI command) can share the same mapping.
pub(crate) fn tarn_error_to_message(path: &Path, err: TarnError) -> ValidationMessage {
    let code = match &err {
        TarnError::Parse(_) => ValidationCode::TarnParse,
        TarnError::Validation(_) => ValidationCode::TarnValidation,
        // `parser::parse_str` does not itself produce these variants today,
        // but guard against accidental breakage in the future by falling
        // back to `tarn_parse` rather than panicking.
        _ => ValidationCode::TarnParse,
    };
    let raw = err.to_string();
    // Strip `thiserror`'s "Parse error: " / "Validation error: " prefix so
    // the user-visible message starts with the actual content.
    let stripped = strip_thiserror_prefix(&raw);
    let (message, location) = extract_location_prefix(stripped, path);
    ValidationMessage {
        severity: Severity::Error,
        code,
        message,
        location,
    }
}

/// Remove the `"<variant>: "` prefix that `thiserror` prepends on every
/// `TarnError::Parse` / `TarnError::Validation` display. Returning the raw
/// string unchanged when the prefix is absent keeps this safe for any
/// future variants.
fn strip_thiserror_prefix(raw: &str) -> &str {
    const PREFIXES: &[&str] = &["Parse error: ", "Validation error: "];
    for prefix in PREFIXES {
        if let Some(rest) = raw.strip_prefix(prefix) {
            return rest;
        }
    }
    raw
}

/// Extract the `"<display-path>:<line>:<column>: <rest>"` prefix that
/// [`parser::parse_str`]'s `enhance_parse_error` embeds on YAML-originated
/// errors. Semantic errors from `validate_test_file` do not carry a
/// location, in which case the original message is preserved and the
/// returned `Location` is `None`.
fn extract_location_prefix(message: &str, path: &Path) -> (String, Option<Location>) {
    let prefix = format!("{}:", path.display());
    let Some(rest) = message.strip_prefix(&prefix) else {
        // Semantic validation errors from `validate_test_file` use the same
        // `"<path>: <msg>"` pattern but with a single colon and no line/col.
        // Strip that prefix too so LSP diagnostics don't double-label the
        // URI, and return with no location.
        let bare = format!("{}: ", path.display());
        let cleaned = message.strip_prefix(&bare).unwrap_or(message).to_string();
        return (cleaned, None);
    };
    // We now have "<line>:<column>: <rest>" or "<line>:<column>: <rest>\n  hint: ...".
    let mut parts = rest.splitn(3, ':');
    let line_part = parts.next();
    let col_part = parts.next();
    let tail = parts.next();
    let (Some(line_str), Some(col_str), Some(tail)) = (line_part, col_part, tail) else {
        // Prefix looked like a path but wasn't fully location-tagged; fall
        // back to stripping just the path.
        let stripped = message
            .strip_prefix(&format!("{}: ", path.display()))
            .unwrap_or(message)
            .to_string();
        return (stripped, None);
    };
    let (Ok(line), Ok(column)) = (
        line_str.trim().parse::<usize>(),
        col_str.trim().parse::<usize>(),
    ) else {
        return (message.to_string(), None);
    };
    let location = Location {
        file: PathBuf::from(path).display().to_string(),
        line,
        column,
    };
    (tail.trim_start().to_string(), Some(location))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PATH: &str = "test.tarn.yaml";

    #[test]
    fn empty_source_yields_semantic_error() {
        // An entirely empty document is valid YAML (`null`) but fails
        // `validate_test_file`'s "must have steps or tests" check.
        let msgs = validate_document(Path::new(TEST_PATH), "");
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        assert_eq!(msg.severity, Severity::Error);
        // Empty input hits parse (invalid type: null) before semantic checks.
        assert!(matches!(
            msg.code,
            ValidationCode::TarnParse | ValidationCode::TarnValidation
        ));
        assert!(!msg.message.is_empty());
    }

    #[test]
    fn valid_minimal_document_produces_no_messages() {
        let source = "name: smoke\nsteps:\n  - name: ping\n    request:\n      method: GET\n      url: http://example.com\n    assert:\n      status: 200\n";
        let msgs = validate_document(Path::new(TEST_PATH), source);
        assert!(msgs.is_empty(), "expected no diagnostics, got {:?}", msgs);
    }

    #[test]
    fn yaml_syntax_error_carries_location() {
        // Unclosed bracket — serde_yaml should return a location.
        let source = "name: broken\nsteps: [\n";
        let msgs = validate_document(Path::new(TEST_PATH), source);
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        assert_eq!(msg.severity, Severity::Error);
        assert_eq!(msg.code, ValidationCode::YamlSyntax);
        assert!(
            msg.location.is_some(),
            "expected serde_yaml to report a location"
        );
    }

    #[test]
    fn tarn_shape_error_on_unknown_top_level_field() {
        // `step` instead of `steps` — caught by `validate_yaml_shape` in
        // the parser, which currently emits `TarnError::Validation` rather
        // than `TarnError::Parse`. This test locks in the mapping: unknown
        // fields surface as the `tarn_validation` code.
        let source = "name: typo\nstep: []\n";
        let msgs = validate_document(Path::new(TEST_PATH), source);
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        assert_eq!(msg.severity, Severity::Error);
        assert_eq!(msg.code, ValidationCode::TarnValidation);
        assert!(
            !msg.message.starts_with("test.tarn.yaml"),
            "path prefix should be stripped from message: {}",
            msg.message
        );
    }

    #[test]
    fn tarn_validation_error_on_wrong_type() {
        // `steps` must be a list, not a string. Today this is caught by
        // `validate_yaml_shape` and surfaces as `TarnError::Validation`.
        // Locks in the current behavior so future refactors can't silently
        // change the code under the LSP's feet.
        let source = "name: typo\nsteps: not-a-list\n";
        let msgs = validate_document(Path::new(TEST_PATH), source);
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        assert_eq!(msg.severity, Severity::Error);
        assert_eq!(msg.code, ValidationCode::TarnValidation);
    }

    #[test]
    fn tarn_validation_error_for_empty_steps_and_tests() {
        // Valid YAML, shape-OK, but violates the "must have steps or tests"
        // semantic check.
        let source = "name: nothing\n";
        let msgs = validate_document(Path::new(TEST_PATH), source);
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        assert_eq!(msg.severity, Severity::Error);
        // This originates in `validate_test_file` which returns TarnError::Parse,
        // so it maps to TarnParse. That's the current code path — the test
        // locks it in so a future refactor doesn't silently change it.
        assert_eq!(msg.code, ValidationCode::TarnParse);
        assert!(msg.message.contains("steps") || msg.message.contains("tests"));
    }

    #[test]
    fn strip_thiserror_prefix_removes_parse_and_validation() {
        assert_eq!(strip_thiserror_prefix("Parse error: hello"), "hello");
        assert_eq!(strip_thiserror_prefix("Validation error: hi"), "hi");
        assert_eq!(strip_thiserror_prefix("Something else"), "Something else");
    }

    #[test]
    fn extract_location_prefix_parses_line_and_column() {
        let (msg, loc) =
            extract_location_prefix("test.tarn.yaml:3:5: something broke", Path::new(TEST_PATH));
        let loc = loc.expect("expected a location");
        assert_eq!(loc.line, 3);
        assert_eq!(loc.column, 5);
        assert_eq!(msg, "something broke");
    }

    #[test]
    fn extract_location_prefix_handles_bare_path_prefix() {
        let (msg, loc) = extract_location_prefix(
            "test.tarn.yaml: Step 'x' has empty URL",
            Path::new(TEST_PATH),
        );
        assert!(loc.is_none());
        assert_eq!(msg, "Step 'x' has empty URL");
    }

    #[test]
    fn severity_and_code_enums_are_distinct() {
        assert_ne!(Severity::Error, Severity::Warning);
        assert_eq!(ValidationCode::YamlSyntax.as_str(), "yaml_syntax");
        assert_eq!(ValidationCode::TarnParse.as_str(), "tarn_parse");
        assert_eq!(ValidationCode::TarnValidation.as_str(), "tarn_validation");
        assert_eq!(ValidationCode::BrittlePattern.as_str(), "brittle_pattern");
    }

    #[test]
    fn lint_flags_exact_length_on_list_endpoint() {
        let source = r#"
name: brittle length
steps:
  - name: list users
    request:
      method: GET
      url: http://example.com/users
    assert:
      status: 200
      body:
        "$.users":
          length: 3
"#;
        let msgs = validate_document(Path::new(TEST_PATH), source);
        assert_eq!(msgs.len(), 1, "{:#?}", msgs);
        assert_eq!(msgs[0].severity, Severity::Warning);
        assert_eq!(msgs[0].code, ValidationCode::BrittlePattern);
        assert!(msgs[0].message.contains("length"));
        assert!(msgs[0].message.contains("$.users"));
    }

    #[test]
    fn lint_allows_length_on_specific_record() {
        // `$.user.tags` is identity-scoped (single user's tags), not
        // list-shaped. Exact length on it is fine and must not warn.
        let source = r#"
name: scoped length
steps:
  - name: get user tags
    request:
      method: GET
      url: http://example.com/users/me/tags
    assert:
      status: 200
      body:
        "$[0].tags":
          length: 2
"#;
        let msgs = validate_document(Path::new(TEST_PATH), source);
        assert!(
            msgs.iter()
                .all(|m| m.code != ValidationCode::BrittlePattern),
            "unexpected lint warning: {:#?}",
            msgs
        );
    }

    #[test]
    fn lint_flags_positional_capture() {
        let source = r#"
name: positional
steps:
  - name: list
    request:
      method: GET
      url: http://example.com/items
    capture:
      first_id: "$[0].id"
    assert:
      status: 200
"#;
        let msgs = validate_document(Path::new(TEST_PATH), source);
        assert!(
            msgs.iter().any(|m| m.code == ValidationCode::BrittlePattern
                && m.message.contains("first_id")),
            "expected positional-capture warning, got {:#?}",
            msgs
        );
    }

    #[test]
    fn lint_flags_static_uuid_in_mutating_url() {
        let source = r#"
name: static id
steps:
  - name: update
    request:
      method: PATCH
      url: http://example.com/users/550e8400-e29b-41d4-a716-446655440000
      body: { name: updated }
    assert:
      status: 200
"#;
        let msgs = validate_document(Path::new(TEST_PATH), source);
        assert!(
            msgs.iter().any(|m| m.code == ValidationCode::BrittlePattern
                && m.message.contains("550e8400")),
            "expected static-UUID warning, got {:#?}",
            msgs
        );
    }

    #[test]
    fn lint_ignores_interpolated_ids() {
        // A `{{ capture.x }}` in a PATCH URL is dynamic — the heuristic
        // must skip it so real dynamic paths don't spam warnings.
        let source = r#"
name: dynamic
steps:
  - name: update
    request:
      method: PATCH
      url: "http://example.com/users/{{ capture.user_id }}"
      body: { name: updated }
    assert:
      status: 200
"#;
        let msgs = validate_document(Path::new(TEST_PATH), source);
        assert!(
            msgs.iter()
                .all(|m| m.code != ValidationCode::BrittlePattern),
            "interpolated URL must not be flagged: {:#?}",
            msgs
        );
    }
}
