//! `textDocument/formatting` handler and pure edit-builder.
//!
//! This is the L3.1 (NAZ-302) editing-surface entry point: it wires
//! `tarn::format::format_document` into the LSP request loop so any
//! client that issues `textDocument/formatting` receives a single
//! whole-document `TextEdit` that replaces the buffer with the canonical
//! output of `tarn fmt`.
//!
//! Range formatting (`textDocument/rangeFormatting`) is explicitly **not**
//! supported — see `docs/TARN_LSP.md` for the rationale. The
//! corresponding `document_range_formatting_provider` flag is left unset
//! in `capabilities.rs`.
//!
//! # Module layout (mirrors `symbols.rs`, `diagnostics.rs`, etc.)
//!
//! * [`format_edits`] — pure function over `(old_source, new_source)`.
//!   Returns either an empty `Vec<TextEdit>` (when the two buffers are
//!   byte-equal) or a single whole-document replace edit covering every
//!   line of the old buffer. The LSP spec does not require a minimal
//!   diff here; clients merge whole-document edits as one undo step.
//! * [`text_document_formatting`] — thin wrapper that reads the document
//!   out of [`DocumentStore`], calls `tarn::format::format_document`,
//!   and feeds the result to [`format_edits`]. Any formatter error
//!   (schema-invalid YAML) and any broken-buffer path both degrade to
//!   "return no edits" so the editor is never asked to apply something
//!   dangerous.
//!
//! # Position math
//!
//! `tarn::format::format_document` never normalises line endings on its
//! own — it feeds `serde_yaml::to_string` output through the parser
//! re-render, which always emits `\n`. To compute the end `Position` of
//! the range we replace, we count `\n` occurrences in the **old** source
//! (the actual buffer on the client) and take the byte length of the
//! final line as the end character. That matches the LSP convention:
//! `Range.end` is exclusive, so the edit covers characters
//! `[start, end)` — an end past the final newline is well-defined and
//! means "replace to end of document".

use lsp_types::{Position, Range, TextEdit, Url};
use tarn::format;

use crate::server::{is_tarn_file_uri, DocumentStore};

/// Build the list of text edits that transform `source` into
/// `new_source`. Returns an empty `Vec` when the two buffers are
/// byte-equal so the client does not mark the document dirty.
///
/// Keep this function **pure**: it must not touch the document store,
/// the network, or the clock. Every consumer of L3.1 formatting (the
/// LSP handler, the unit tests, future MCP tools) should be able to
/// call this directly with two `&str`s and get a deterministic answer.
pub fn format_edits(source: &str, new_source: &str) -> Vec<TextEdit> {
    if source == new_source {
        return Vec::new();
    }

    // Compute the end `Position` of the old buffer. The LSP spec uses
    // zero-based line/character indices; `character` is measured in
    // UTF-16 code units. `.tarn.yaml` files are overwhelmingly ASCII
    // and the formatter output stays within UTF-8, so for any practical
    // Tarn test file `char_len == utf16_len`. Still, we count UTF-16
    // code units to stay spec-correct even if a user ever drops a BOM
    // or a non-BMP character into a comment.
    let end = end_position(source);
    let full_range = Range::new(Position::new(0, 0), end);
    vec![TextEdit {
        range: full_range,
        new_text: new_source.to_owned(),
    }]
}

/// `textDocument/formatting` request handler.
///
/// Behaviour:
///
/// * **Happy path** — the buffer parses and the formatter produces a
///   new canonical rendering. Return `[TextEdit]` with one whole-document
///   replace edit (or empty if the buffer is already canonical).
/// * **Unknown URI** — the client asked to format a document the server
///   has not seen (no `didOpen`). Return empty — formatting nothing is
///   not an error.
/// * **Un-parseable YAML** — `format_document` returns `Ok(identity)`
///   and logs via stderr. `format_edits(source, source)` then returns
///   empty. The user sees no edits and no error pop-up. This matches
///   the VS Code `TarnFormatProvider` behaviour (NAZ-170).
/// * **Schema-invalid YAML** — `format_document` returns `Err(FormatError)`.
///   We log the error prefixed with `tarn-lsp:` so it shows up in the
///   "Language Server" output pane, then return empty. Formatting must
///   never corrupt a partially-broken test file.
pub fn text_document_formatting(store: &DocumentStore, uri: &Url) -> Vec<TextEdit> {
    if !is_tarn_file_uri(uri) {
        return Vec::new();
    }
    let Some(source) = store.get(uri) else {
        return Vec::new();
    };
    match format::format_document(source) {
        Ok(new_source) => format_edits(source, &new_source),
        Err(err) => {
            eprintln!(
                "tarn-lsp: textDocument/formatting suppressing formatter error for {uri}: {err}"
            );
            Vec::new()
        }
    }
}

/// Compute the end `Position` of a source string: the line index of
/// the last line (0-based) and the UTF-16 length of that line as the
/// character index.
///
/// Split out of [`format_edits`] so the position math is easy to
/// unit-test in isolation.
fn end_position(source: &str) -> Position {
    let mut line: u32 = 0;
    let mut line_start_byte: usize = 0;
    for (idx, ch) in source.char_indices() {
        if ch == '\n' {
            line += 1;
            line_start_byte = idx + 1;
        }
    }
    // Character column is the UTF-16 length of the final line slice.
    let last_line = &source[line_start_byte..];
    let character: u32 = last_line.encode_utf16().count() as u32;
    Position::new(line, character)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Url;

    fn uri(name: &str) -> Url {
        Url::parse(&format!("file:///tmp/{name}.tarn.yaml")).unwrap()
    }

    #[test]
    fn format_edits_returns_empty_vec_when_buffers_are_identical() {
        let src =
            "name: already\nsteps:\n- name: ping\n  request:\n    method: GET\n    url: http://x\n";
        let edits = format_edits(src, src);
        assert!(
            edits.is_empty(),
            "identical buffers must produce zero edits, got {edits:?}"
        );
    }

    #[test]
    fn format_edits_returns_one_whole_document_edit_when_buffers_differ() {
        let before =
            "steps:\n- name: x\n  request:\n    url: http://x\n    method: GET\nname: reorder\n";
        let after =
            "name: reorder\nsteps:\n- name: x\n  request:\n    method: GET\n    url: http://x\n";
        let edits = format_edits(before, after);
        assert_eq!(
            edits.len(),
            1,
            "exactly one whole-document edit is expected, got {edits:?}"
        );
        let edit = &edits[0];
        assert_eq!(edit.new_text, after);
        // Range must start at (0, 0) so the edit replaces the whole
        // buffer no matter what indentation the old content had.
        assert_eq!(edit.range.start, Position::new(0, 0));
    }

    #[test]
    fn format_edits_end_range_reaches_past_last_newline() {
        // `before` ends with a trailing newline — the end Position must
        // point at (last_line_count, 0) so the replacement covers the
        // whole buffer.
        let before = "a\nb\nc\n";
        let after = "b\n";
        let edits = format_edits(before, after);
        assert_eq!(edits.len(), 1);
        let edit = &edits[0];
        // `before` has three '\n' bytes, so end.line must be 3 and
        // end.character 0 (empty final "line" after the trailing \n).
        assert_eq!(edit.range.end, Position::new(3, 0));
        assert_eq!(edit.new_text, after);
    }

    #[test]
    fn format_edits_end_range_handles_missing_trailing_newline() {
        // `before` has no trailing newline, so end.line = 0 and
        // end.character equals the buffer length.
        let before = "abc";
        let after = "def";
        let edits = format_edits(before, after);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.end, Position::new(0, 3));
    }

    #[test]
    fn format_edits_on_empty_to_empty_is_no_op() {
        let edits = format_edits("", "");
        assert!(edits.is_empty());
    }

    #[test]
    fn format_edits_on_whitespace_only_change_still_produces_single_edit() {
        // The formatter collapsing trailing whitespace is a legitimate
        // change — we still emit one whole-document replace edit.
        let before =
            "name: x\nsteps:\n- name: s  \n  request:\n    method: GET\n    url: http://x\n";
        let after = "name: x\nsteps:\n- name: s\n  request:\n    method: GET\n    url: http://x\n";
        let edits = format_edits(before, after);
        assert_eq!(edits.len(), 1);
        assert_ne!(before, after, "fixture must actually differ");
    }

    #[test]
    fn text_document_formatting_on_unknown_uri_returns_empty() {
        let store = DocumentStore::new();
        let edits = text_document_formatting(&store, &uri("never-opened"));
        assert!(
            edits.is_empty(),
            "unknown URI must yield no edits, got {edits:?}"
        );
    }

    #[test]
    fn text_document_formatting_on_already_canonical_document_returns_empty() {
        let mut store = DocumentStore::new();
        let u = uri("canonical");
        let src = "name: Canonical\nsteps:\n- name: ping\n  request:\n    method: GET\n    url: http://localhost:3000/ping\n  assert:\n    status: 200\n";
        store.open(u.clone(), src.to_owned());
        let edits = text_document_formatting(&store, &u);
        assert!(
            edits.is_empty(),
            "already-canonical document must yield zero edits, got {edits:?}"
        );
    }

    #[test]
    fn text_document_formatting_on_broken_document_returns_empty() {
        let mut store = DocumentStore::new();
        let u = uri("broken");
        let broken = "name: broken\nsteps: [\n  - name: oops\n";
        store.open(u.clone(), broken.to_owned());
        let edits = text_document_formatting(&store, &u);
        assert!(
            edits.is_empty(),
            "broken YAML must collapse to zero edits, got {edits:?}"
        );
    }

    #[test]
    fn text_document_formatting_on_schema_invalid_document_returns_empty() {
        let mut store = DocumentStore::new();
        let u = uri("orphan");
        // Parseable YAML, but the formatter's schema pass rejects it
        // because a required top-level `name:` is missing.
        let orphan = "steps:\n  - name: orphan\n    request:\n      method: GET\n      url: http://localhost:3000\n";
        store.open(u.clone(), orphan.to_owned());
        let edits = text_document_formatting(&store, &u);
        assert!(
            edits.is_empty(),
            "schema-invalid YAML must collapse to zero edits, got {edits:?}"
        );
    }

    #[test]
    fn text_document_formatting_on_non_canonical_document_returns_single_edit() {
        let mut store = DocumentStore::new();
        let u = uri("non-canonical");
        // Deliberately mis-ordered: steps before name, request url
        // before method.
        let src = "steps:\n- request:\n    url: http://x\n    method: GET\n  name: rename\nname: reorder me\n";
        store.open(u.clone(), src.to_owned());
        let edits = text_document_formatting(&store, &u);
        assert_eq!(edits.len(), 1, "expected one edit, got {edits:?}");
        let edit = &edits[0];
        assert!(
            edit.new_text.starts_with("name: reorder me\n"),
            "formatted output must start with the canonical top-level name, got: {}",
            edit.new_text
        );
    }

    #[test]
    fn end_position_counts_lines_and_characters() {
        assert_eq!(end_position(""), Position::new(0, 0));
        assert_eq!(end_position("abc"), Position::new(0, 3));
        assert_eq!(end_position("abc\n"), Position::new(1, 0));
        assert_eq!(end_position("abc\ndef"), Position::new(1, 3));
        assert_eq!(end_position("abc\ndef\n"), Position::new(2, 0));
        // Non-ASCII: "é" is one UTF-16 code unit, "🐟" is two.
        assert_eq!(end_position("é"), Position::new(0, 1));
        assert_eq!(end_position("🐟"), Position::new(0, 2));
    }
}
