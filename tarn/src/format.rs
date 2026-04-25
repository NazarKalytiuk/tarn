//! In-process whole-document formatter for `.tarn.yaml` buffers.
//!
//! This module owns the **library surface** of `tarn fmt`: the thin wrapper
//! [`format_document`] that any in-process caller (the `tarn` CLI, the
//! `tarn-lsp` formatting handler, future MCP consumers) calls to reformat a
//! single buffer as a `String`.
//!
//! The heavy lifting (alias normalisation, field reordering, schema
//! directive preservation) already lives in [`crate::parser::format_str`] —
//! this module exists so callers no longer have to know about that
//! function's `Path` argument or its [`crate::error::TarnError`] return
//! shape. That keeps the CLI and the LSP on **one** implementation with
//! **two** call sites, exactly as NAZ-302 requires.
//!
//! # Error contract
//!
//! * Parse-able-but-schema-invalid input → [`FormatError`] with a human
//!   message (and, when available, a 1-based [`Location`] pointing at the
//!   offending node). Callers that care about exit codes (CLI) surface the
//!   error; callers that prefer to degrade gracefully (LSP) collapse the
//!   error into a no-op.
//! * **Unparseable** YAML → `Ok(source.to_string())`. The library logs a
//!   `tarn::format:` prefixed warning on stderr so the CLI output channel
//!   still tells the user "something was broken". Formatting a broken
//!   buffer is never an error — it is a no-op. The LSP handler relies on
//!   this so an editor that asks for `textDocument/formatting` mid-edit
//!   never sees a server-side error pop-up.
//! * Empty input → `Ok(String::new())` without running the parser.
//!
//! # Why a shim instead of moving `format_str` here
//!
//! [`crate::parser::format_str`] shares a lot of helper functions with the
//! parser-side validation pass (`validate_formattable_test_file`,
//! `normalize_*_value`, etc.). Lifting those helpers out of `parser.rs`
//! would make this PR enormous and risky. Keeping the implementation in
//! `parser.rs` and exposing a clean façade here gives us the API the LSP
//! needs today without churning the rest of the parser module.

use std::path::Path;

use crate::error::TarnError;
use crate::model::Location;
use crate::parser;

/// Synthetic path used when the caller only has an in-memory buffer.
///
/// The parser uses this path for two things: include resolution (a broken
/// relative include would resolve against the buffer's directory) and
/// error messages. An in-memory LSP buffer has neither a directory nor a
/// meaningful filename, so we synthesize `<buffer>` — error messages
/// render it verbatim, which matches what stdin-style tools do in other
/// ecosystems (`prettier --stdin-filepath`, `rustfmt --emit=stdout`).
const BUFFER_PATH: &str = "<buffer>";

/// Structured error returned by [`format_document`] when a document is
/// parse-able but fails the schema validation the formatter runs before
/// normalising field order.
///
/// The `location` field is optional because not every error kind the
/// underlying parser produces carries a span — validator errors often do,
/// but shape errors (e.g. "root must be a mapping") may not. Callers that
/// want a fall-back range should treat `None` as `(1, 1)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError {
    /// Human-readable summary of the failure. Safe to surface verbatim in
    /// editor notifications — it never contains secrets (Tarn's validator
    /// redacts them upstream).
    pub message: String,
    /// Optional 1-based point inside `source` the failure is anchored to.
    /// Reuses [`crate::model::Location`] so every downstream consumer
    /// (diagnostics, hover, LSP range conversion) shares one type.
    pub location: Option<Location>,
}

impl FormatError {
    /// Build a `FormatError` from a bare message (no location). Used when
    /// the underlying parser surfaces a message without a span.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            location: None,
        }
    }
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for FormatError {}

/// Reformat a single `.tarn.yaml` document buffer.
///
/// See the module docs for the full error contract. Summarised:
///
/// | input                    | result                                |
/// |--------------------------|---------------------------------------|
/// | already canonical        | `Ok(source.to_string())`              |
/// | non-canonical & valid    | `Ok(formatted)`                       |
/// | empty string             | `Ok(String::new())`                   |
/// | **unparseable YAML**     | `Ok(source.to_string())` + warn log   |
/// | schema-invalid YAML      | `Err(FormatError)`                    |
///
/// The function is pure with respect to the filesystem — it reads no
/// files, writes no files, and never looks at environment variables. The
/// only side effect is the `eprintln!` warning on the unparseable branch.
pub fn format_document(source: &str) -> Result<String, FormatError> {
    // Empty input: skip the parser entirely. `serde_yaml::from_str("")`
    // returns `Value::Null`, which the normaliser then round-trips to
    // `"null\n"` — not what the user wants when they format an empty
    // buffer. Treat empty as the identity operation.
    if source.is_empty() {
        return Ok(String::new());
    }

    // Whitespace-only input is the same story: the parser would either
    // error or collapse it to `null`. Preserve the original bytes so the
    // LSP handler's "identical → no edits" branch fires.
    if source.chars().all(|c| c.is_whitespace()) {
        return Ok(source.to_string());
    }

    // Fast-path unparseable-YAML detection. We run `serde_yaml::from_str`
    // once up-front so we can distinguish "broken YAML" (→ identity
    // no-op) from "parseable but schema-invalid" (→ structured error).
    // `parser::format_str` conflates both into `TarnError::Parse`, so
    // without this check we'd have to pattern-match on the error message
    // to tell them apart.
    if let Err(parse_err) = serde_yaml::from_str::<serde_yaml::Value>(source) {
        eprintln!("tarn::format: skipping format_document on unparseable buffer: {parse_err}");
        return Ok(source.to_string());
    }

    match parser::format_str(source, Path::new(BUFFER_PATH)) {
        Ok(formatted) => Ok(formatted),
        Err(err) => Err(tarn_error_to_format_error(err)),
    }
}

/// Lift a [`TarnError`] returned by [`parser::format_str`] into a
/// [`FormatError`]. The parser rarely attaches explicit spans today, so
/// `location` is almost always `None` — we keep the field on `FormatError`
/// so future parser work can populate it without breaking this surface.
fn tarn_error_to_format_error(err: TarnError) -> FormatError {
    FormatError {
        message: err.to_string(),
        location: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canonical, already-formatted test file. Field order matches the
    /// `TOP_LEVEL_FIELDS` / `STEP_FIELDS` arrays in `parser.rs`, so
    /// `format_document` on this input must be the identity.
    const CANONICAL: &str = "name: Canonical\nsteps:\n- name: ping\n  request:\n    method: GET\n    url: http://localhost:3000/ping\n  assert:\n    status: 200\n";

    #[test]
    fn format_document_on_already_canonical_input_is_identity() {
        let formatted = format_document(CANONICAL).expect("canonical input must format");
        assert_eq!(
            formatted, CANONICAL,
            "canonical input should round-trip unchanged"
        );
    }

    #[test]
    fn format_document_normalizes_field_order_and_whitespace() {
        // `request` appears before `name`, `assert` operators out of
        // canonical order — the formatter must rewrite all of that.
        let input = r#"
steps:
  - request:
      url: http://localhost:3000
      method: GET
    name: Example
    assert:
      body:
        "$.email":
          not_empty: true
          type: string
          contains: "@"
      status: 200
name: Format me
"#;
        let formatted = format_document(input).expect("parseable input must format");

        // `name:` is now the first top-level key.
        assert!(
            formatted.starts_with("name: Format me\n"),
            "name: should come first, got: {formatted}"
        );
        // Within the step, `name` precedes `request`, and `method`
        // precedes `url` inside `request`.
        let name_idx = formatted
            .find("name: Example")
            .expect("name: Example present");
        let method_idx = formatted.find("method: GET").expect("method: GET present");
        let url_idx = formatted
            .find("http://localhost:3000")
            .expect("url present");
        assert!(name_idx < method_idx, "name should precede method");
        assert!(method_idx < url_idx, "method should precede url");
        // Canonical assertion operator order: type → contains → not_empty.
        let type_idx = formatted.find("type: string").expect("type operator");
        let contains_idx = formatted.find("contains: '@'").expect("contains operator");
        let not_empty_idx = formatted
            .find("not_empty: true")
            .expect("not_empty operator");
        assert!(type_idx < contains_idx);
        assert!(contains_idx < not_empty_idx);
    }

    #[test]
    fn format_document_preserves_schema_directive_comment() {
        // Comments are lost by serde_yaml → re-render, with one exception:
        // the `# yaml-language-server:` directive, which the formatter
        // re-attaches to the top of the document. This test locks that
        // exception in.
        let input = r#"# yaml-language-server: $schema=https://raw.githubusercontent.com/NazarKalytiuk/tarn/main/schemas/v1/testfile.json
name: With schema
steps:
  - name: ping
    request:
      method: GET
      url: http://localhost:3000/ping
"#;
        let formatted = format_document(input).expect("directive input must format");
        assert!(
            formatted.starts_with(
                "# yaml-language-server: $schema=https://raw.githubusercontent.com/NazarKalytiuk/tarn/main/schemas/v1/testfile.json\n"
            ),
            "schema directive must stay at the top of the formatted buffer, got: {formatted}"
        );
        assert!(formatted.contains("name: With schema\n"));
    }

    #[test]
    fn format_document_on_unparseable_yaml_returns_identity_no_error() {
        // `[` with no closing bracket: serde_yaml rejects outright. The
        // contract says we return the input unchanged and log — we do NOT
        // error — so an LSP client that asks for formatting mid-edit
        // never sees a server-side pop-up.
        let broken = "name: broken\nsteps: [\n  - name: oops\n";
        let formatted = format_document(broken).expect("broken input must NOT error");
        assert_eq!(formatted, broken, "broken YAML should be returned verbatim");
    }

    #[test]
    fn format_document_on_empty_input_returns_empty_string() {
        let formatted = format_document("").expect("empty input must not error");
        assert_eq!(formatted, "", "empty input should produce empty output");
    }

    #[test]
    fn format_document_on_whitespace_only_input_returns_input_verbatim() {
        // Whitespace-only is technically parseable (→ `null`), but
        // running it through the parser would collapse it to `"null\n"`
        // and break the LSP's identity check. Treat it as identity.
        let ws = "   \n\t\n  ";
        let formatted = format_document(ws).expect("whitespace input must not error");
        assert_eq!(formatted, ws, "whitespace-only should be preserved");
    }

    #[test]
    fn format_document_on_schema_invalid_input_returns_format_error() {
        // Parseable YAML, but fails the schema validator the formatter
        // runs before normalising (missing `name:` at root). Callers get
        // a structured error, not identity.
        let input = "steps:\n  - name: orphan\n    request:\n      method: GET\n      url: http://localhost:3000\n";
        let err = format_document(input).expect_err("schema-invalid input must return FormatError");
        assert!(
            !err.message.is_empty(),
            "FormatError must carry a non-empty message, got: {err:?}"
        );
    }

    #[test]
    fn format_document_is_idempotent() {
        // Format twice — the second pass must be a no-op. This is the
        // single strongest property test we have: if any normaliser
        // introduces non-determinism, the second call will surface it.
        let input = r#"
name: Idempotent
steps:
  - request:
      url: http://localhost:3000
      method: GET
    name: step
"#;
        let once = format_document(input).expect("first format must succeed");
        let twice = format_document(&once).expect("second format must succeed");
        assert_eq!(
            once, twice,
            "format_document must be idempotent: re-formatting a formatted buffer is a no-op"
        );
    }

    #[test]
    fn format_error_display_renders_message_verbatim() {
        let err = FormatError::new("boom");
        assert_eq!(err.to_string(), "boom");
        assert!(err.location.is_none());
    }
}
