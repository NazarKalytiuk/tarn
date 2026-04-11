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
}

impl ValidationCode {
    /// Stable string form used in LSP `Diagnostic.code` and similar surfaces.
    pub fn as_str(&self) -> &'static str {
        match self {
            ValidationCode::YamlSyntax => "yaml_syntax",
            ValidationCode::TarnParse => "tarn_parse",
            ValidationCode::TarnValidation => "tarn_validation",
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
        Ok(_) => Vec::new(),
        Err(err) => vec![tarn_error_to_message(path, err)],
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
    }
}
