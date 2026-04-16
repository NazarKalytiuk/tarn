//! `textDocument/documentSymbol` handler and renderer.
//!
//! This module is the L1.5 (NAZ-294) counterpart to the L1.2/L1.3/L1.4
//! handlers: a pure renderer (`outline_to_document_symbols`) plus a thin
//! wrapper that reads the document out of the [`DocumentStore`] and hands
//! the renderer output back to the main loop for serialization.
//!
//! The outline itself is produced by [`tarn::outline::outline_document`],
//! a yaml-rust2 second-pass scanner added in the same ticket. Keeping the
//! outline extraction inside the `tarn` crate means diagnostics, symbols,
//! and every future outline consumer share one source of truth for the
//! ranges they report — so an error on line 42 is reported against the
//! exact same symbol the outline view highlights for that line.
//!
//! Range conventions:
//!
//!   * `tarn::outline` reports 1-based line/column numbers, the same
//!     convention [`tarn::model::Location`] uses.
//!   * This module is the **one** place where we convert those to 0-based
//!     LSP [`Position`]s. If more than one place did the conversion we'd
//!     eventually get off-by-one wrong in exactly one of them.
//!   * Symbol `range` covers the full YAML node (expands selection to the
//!     whole mapping when the user clicks the symbol), and
//!     `selection_range` covers just the `name:` value (drives the
//!     highlight when the cursor lands on it from elsewhere).
//!   * `kind`:
//!       - `Namespace` for the file root,
//!       - `Module` for every named test group,
//!       - `Function` for every step (setup, teardown, flat, nested).
//!
//! Everything here is sync and pure apart from the tiny wrapper at the
//! bottom that reaches into `DocumentStore`. That makes the renderer
//! trivially unit-testable — the integration tests still drive the full
//! LSP round-trip, they just don't need to carry every range assertion.

use std::path::Path;

use lsp_types::{DocumentSymbol, DocumentSymbolResponse, Position, Range, SymbolKind, Url};
use tarn::outline::{outline_document, Outline, OutlineSpan, StepOutline, TestOutline};

use crate::server::{is_tarn_file_uri, DocumentStore};

/// Entry point for the request dispatcher.
///
/// Reads the current buffer for `uri` out of the store, extracts an
/// [`Outline`], and renders it as a hierarchical
/// [`DocumentSymbolResponse`]. Always returns a response — an unknown URI
/// or an un-parseable buffer yields `Nested(vec![])`, which LSP clients
/// display as "no symbols" rather than showing an error.
///
/// Note the function name is in `snake_case` to match the existing handler
/// style (`text_document_hover`, `text_document_completion`). Clippy's
/// `non_snake_case` lint fires on the ticket-style `textDocument_*` name,
/// so we match the file's neighbours rather than its spec.
pub fn text_document_document_symbol(store: &DocumentStore, uri: &Url) -> DocumentSymbolResponse {
    if !is_tarn_file_uri(uri) {
        return DocumentSymbolResponse::Nested(Vec::new());
    }
    let symbols = build_symbols(store, uri);
    DocumentSymbolResponse::Nested(symbols)
}

/// Build the symbol tree for `uri`. Split out from
/// [`text_document_document_symbol`] so tests can assert directly against
/// the `Vec<DocumentSymbol>` without unwrapping the enum.
fn build_symbols(store: &DocumentStore, uri: &Url) -> Vec<DocumentSymbol> {
    let Some(source) = store.get(uri) else {
        return Vec::new();
    };
    let path = uri_to_path(uri);
    match outline_document(&path, source) {
        Some(outline) => outline_to_document_symbols(&outline, uri),
        None => Vec::new(),
    }
}

/// Pure renderer: map an [`Outline`] into the hierarchical
/// `Vec<DocumentSymbol>` that LSP clients render in their outline pane.
///
/// Emits at most one file-root symbol — the top-level `name:` becomes the
/// parent, with setup / tests / teardown / flat steps hanging off it.
/// When the file has no `name:` we fall back to the URI's basename so the
/// outline still has a single root; this matches what the VS Code
/// provider does for the same edge case.
///
/// Empty inputs (no tests, no steps, no name) still emit an empty vector
/// — callers should not rely on this function ever returning a dummy
/// "pending" symbol.
pub fn outline_to_document_symbols(outline: &Outline, uri: &Url) -> Vec<DocumentSymbol> {
    // Collect every child first — we decide afterwards whether to wrap
    // them in a file-root node or return them bare.
    let mut children = Vec::new();
    for step in &outline.setup {
        children.push(step_to_symbol(step, "setup"));
    }
    for test in &outline.tests {
        children.push(test_to_symbol(test));
    }
    for step in &outline.teardown {
        children.push(step_to_symbol(step, "teardown"));
    }
    for step in &outline.flat_steps {
        children.push(step_to_symbol(step, "step"));
    }

    // Decide the root name: prefer the declared `name:` value, otherwise
    // the URI basename (without the `.tarn.yaml` extension) to match VS
    // Code's behaviour.
    let root_name = outline
        .file_name
        .clone()
        .or_else(|| basename_from_uri(uri))
        .unwrap_or_else(|| "tarn".to_owned());

    // Root range: span every child when we have one, else the file-name
    // scalar when we have one, else a zero-width point at (0, 0). This
    // last branch only fires for the edge case of an empty document.
    let root_range = children_bounding_range(&children)
        .or_else(|| outline.file_name_range.as_ref().map(span_to_range))
        .unwrap_or_else(zero_range);
    let root_selection = outline
        .file_name_range
        .as_ref()
        .map(span_to_range)
        .unwrap_or(root_range);

    let root = DocumentSymbol {
        name: root_name,
        detail: None,
        kind: SymbolKind::NAMESPACE,
        tags: None,
        #[allow(deprecated)]
        deprecated: None,
        range: root_range,
        selection_range: root_selection,
        children: Some(children),
    };

    // An entirely empty document (no name, no steps, no tests, no hooks)
    // should return a bare empty vector rather than a root with no
    // children — editors will happily render an empty outline view, but
    // a ghost "tarn" node with nothing under it is noise.
    if outline.file_name.is_none()
        && outline.setup.is_empty()
        && outline.teardown.is_empty()
        && outline.tests.is_empty()
        && outline.flat_steps.is_empty()
    {
        return Vec::new();
    }

    vec![root]
}

/// Build the symbol for a named test group, including its nested steps.
///
/// Every test becomes `SymbolKind::MODULE` — the closest match in the
/// standard LSP kind set to "a grouping of related steps". The VS Code
/// provider uses `Class`, but the ticket pins us to `Module` so the LSP
/// surface is consistent regardless of which client renders it.
fn test_to_symbol(test: &TestOutline) -> DocumentSymbol {
    let mut children = Vec::with_capacity(test.steps.len());
    for step in &test.steps {
        children.push(step_to_symbol(step, "step"));
    }

    DocumentSymbol {
        name: test.name.clone(),
        detail: None,
        kind: SymbolKind::MODULE,
        tags: None,
        #[allow(deprecated)]
        deprecated: None,
        range: span_to_range(&test.range),
        selection_range: span_to_range(&test.selection_range),
        children: Some(children),
    }
}

/// Render a single step as a `DocumentSymbol`. The `section` hint is used
/// only for the `detail` label so the outline disambiguates between
/// `setup / step / teardown` visually.
fn step_to_symbol(step: &StepOutline, section: &str) -> DocumentSymbol {
    DocumentSymbol {
        name: step.name.clone(),
        detail: Some(section.to_owned()),
        kind: SymbolKind::FUNCTION,
        tags: None,
        #[allow(deprecated)]
        deprecated: None,
        range: span_to_range(&step.range),
        selection_range: span_to_range(&step.selection_range),
        children: Some(Vec::new()),
    }
}

/// Convert a 1-based [`OutlineSpan`] into an LSP [`Range`].
///
/// This is the **only** place in `tarn-lsp` that converts outline spans
/// into 0-based LSP positions. Keeping the conversion on one fence line
/// matches [`crate::diagnostics::location_to_range`] and ensures
/// symbol/diagnostic ranges agree on the same lines.
fn span_to_range(span: &OutlineSpan) -> Range {
    let start = Position::new(
        span.start_line.saturating_sub(1) as u32,
        span.start_column.saturating_sub(1) as u32,
    );
    let end = Position::new(
        span.end_line.saturating_sub(1) as u32,
        span.end_column.saturating_sub(1) as u32,
    );
    Range::new(start, end)
}

/// Fallback zero-width range at document start. Used only for the "empty
/// document with nothing to anchor on" path.
fn zero_range() -> Range {
    Range::new(Position::new(0, 0), Position::new(0, 0))
}

/// Compute the smallest range that encloses every child symbol. Returns
/// `None` when the child slice is empty so the caller can pick a
/// different fallback without losing the distinction.
fn children_bounding_range(children: &[DocumentSymbol]) -> Option<Range> {
    let first = children.first()?;
    let mut min = first.range.start;
    let mut max = first.range.end;
    for child in &children[1..] {
        if position_before(child.range.start, min) {
            min = child.range.start;
        }
        if position_before(max, child.range.end) {
            max = child.range.end;
        }
    }
    Some(Range::new(min, max))
}

/// `true` when `a` is strictly before `b` in document order.
fn position_before(a: Position, b: Position) -> bool {
    (a.line, a.character) < (b.line, b.character)
}

/// Extract the basename (without the `.tarn.yaml` extension) from a file
/// URI, used as the file-root symbol name when the document has no
/// `name:` key. Returns `None` for URIs without a path segment.
fn basename_from_uri(uri: &Url) -> Option<String> {
    let path = uri.path();
    let trimmed = path.trim_end_matches('/');
    let basename = trimmed.rsplit('/').next()?;
    if basename.is_empty() {
        return None;
    }
    // Strip both `.tarn.yaml` and `.tarn.yml` — the ordering matters so
    // we match the longer suffix first.
    if let Some(stripped) = basename.strip_suffix(".tarn.yaml") {
        return Some(stripped.to_owned());
    }
    if let Some(stripped) = basename.strip_suffix(".tarn.yml") {
        return Some(stripped.to_owned());
    }
    Some(basename.to_owned())
}

/// Convert an LSP `Url` to a `PathBuf` for the outline extractor. Mirrors
/// [`crate::diagnostics::uri_to_path`] so both features anchor on the same
/// display path.
fn uri_to_path(uri: &Url) -> std::path::PathBuf {
    uri.to_file_path()
        .unwrap_or_else(|_| Path::new(uri.path()).to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tarn::outline::{Outline, OutlineSpan, StepOutline, TestOutline};

    fn uri() -> Url {
        Url::parse("file:///tmp/fixture.tarn.yaml").unwrap()
    }

    fn span(start_line: usize, end_line: usize) -> OutlineSpan {
        OutlineSpan {
            start_line,
            start_column: 1,
            end_line,
            end_column: 10,
        }
    }

    fn sample_step(name: &str, start_line: usize, end_line: usize) -> StepOutline {
        StepOutline {
            name: name.to_owned(),
            range: span(start_line, end_line),
            selection_range: OutlineSpan::point(start_line, 9),
        }
    }

    // --- unit tests for the pure renderer ---

    #[test]
    fn empty_outline_yields_no_symbols() {
        let outline = Outline {
            file: "empty.tarn.yaml".to_owned(),
            ..Outline::default()
        };
        let symbols = outline_to_document_symbols(&outline, &uri());
        assert!(symbols.is_empty());
    }

    #[test]
    fn file_with_only_name_emits_root_namespace_symbol() {
        let outline = Outline {
            file: "only.tarn.yaml".to_owned(),
            file_name: Some("Only".to_owned()),
            file_name_range: Some(OutlineSpan::point(1, 7)),
            ..Outline::default()
        };
        let symbols = outline_to_document_symbols(&outline, &uri());
        assert_eq!(symbols.len(), 1);
        let root = &symbols[0];
        assert_eq!(root.name, "Only");
        assert_eq!(root.kind, SymbolKind::NAMESPACE);
        // Range and selection_range both anchor on the name scalar.
        assert_eq!(root.selection_range.start.line, 0);
        assert_eq!(root.selection_range.start.character, 6);
        assert!(root.children.as_ref().map(|c| c.is_empty()).unwrap_or(true));
    }

    #[test]
    fn tests_become_module_children_with_nested_function_steps() {
        let outline = Outline {
            file: "n.tarn.yaml".to_owned(),
            file_name: Some("Nested".to_owned()),
            file_name_range: Some(OutlineSpan::point(1, 7)),
            tests: vec![TestOutline {
                name: "main".to_owned(),
                range: span(3, 10),
                selection_range: OutlineSpan::point(3, 3),
                steps: vec![sample_step("alpha", 5, 6), sample_step("beta", 7, 8)],
            }],
            ..Outline::default()
        };

        let symbols = outline_to_document_symbols(&outline, &uri());
        assert_eq!(symbols.len(), 1);
        let root = &symbols[0];
        assert_eq!(root.kind, SymbolKind::NAMESPACE);
        let children = root.children.as_ref().unwrap();
        assert_eq!(children.len(), 1);
        let test_symbol = &children[0];
        assert_eq!(test_symbol.name, "main");
        assert_eq!(test_symbol.kind, SymbolKind::MODULE);
        let step_children = test_symbol.children.as_ref().unwrap();
        assert_eq!(step_children.len(), 2);
        assert_eq!(step_children[0].name, "alpha");
        assert_eq!(step_children[0].kind, SymbolKind::FUNCTION);
        assert_eq!(step_children[1].name, "beta");
        assert_eq!(step_children[1].kind, SymbolKind::FUNCTION);
    }

    #[test]
    fn setup_and_teardown_are_emitted_as_function_siblings() {
        let outline = Outline {
            file: "h.tarn.yaml".to_owned(),
            file_name: Some("Hooks".to_owned()),
            file_name_range: Some(OutlineSpan::point(1, 7)),
            setup: vec![sample_step("login", 3, 4)],
            teardown: vec![sample_step("cleanup", 10, 11)],
            flat_steps: vec![sample_step("main", 6, 7)],
            ..Outline::default()
        };
        let symbols = outline_to_document_symbols(&outline, &uri());
        assert_eq!(symbols.len(), 1);
        let children = symbols[0].children.as_ref().unwrap();
        // Order: setup, (no tests), teardown, flat_steps.
        assert_eq!(children.len(), 3);
        assert_eq!(children[0].name, "login");
        assert_eq!(children[0].detail.as_deref(), Some("setup"));
        assert_eq!(children[0].kind, SymbolKind::FUNCTION);
        assert_eq!(children[1].name, "cleanup");
        assert_eq!(children[1].detail.as_deref(), Some("teardown"));
        assert_eq!(children[2].name, "main");
        assert_eq!(children[2].detail.as_deref(), Some("step"));
    }

    #[test]
    fn missing_file_name_falls_back_to_basename() {
        let outline = Outline {
            file: "/tmp/unnamed.tarn.yaml".to_owned(),
            flat_steps: vec![sample_step("only", 2, 3)],
            ..Outline::default()
        };
        let symbols = outline_to_document_symbols(&outline, &uri());
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "fixture");
        // Root range is the bounding box of children when no file-name
        // scalar is available.
        assert_eq!(symbols[0].range.start.line, 1);
    }

    #[test]
    fn step_without_name_uses_synthetic_placeholder() {
        let outline = Outline {
            file: "p.tarn.yaml".to_owned(),
            file_name: Some("Placeholder".to_owned()),
            file_name_range: Some(OutlineSpan::point(1, 7)),
            flat_steps: vec![StepOutline {
                name: "<step 1>".to_owned(),
                range: span(3, 4),
                selection_range: OutlineSpan::point(3, 3),
            }],
            ..Outline::default()
        };
        let symbols = outline_to_document_symbols(&outline, &uri());
        let children = symbols[0].children.as_ref().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "<step 1>");
    }

    #[test]
    fn one_based_spans_convert_to_zero_based_lsp_positions() {
        let span = OutlineSpan {
            start_line: 3,
            start_column: 5,
            end_line: 3,
            end_column: 8,
        };
        let range = span_to_range(&span);
        assert_eq!(range.start, Position::new(2, 4));
        assert_eq!(range.end, Position::new(2, 7));
    }

    #[test]
    fn zero_line_or_column_clamps_without_underflow() {
        let span = OutlineSpan {
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
        };
        let range = span_to_range(&span);
        assert_eq!(range.start, Position::new(0, 0));
        assert_eq!(range.end, Position::new(0, 0));
    }

    #[test]
    fn children_bounding_range_spans_from_first_start_to_last_end() {
        let symbols = vec![
            DocumentSymbol {
                name: "a".into(),
                detail: None,
                kind: SymbolKind::FUNCTION,
                tags: None,
                #[allow(deprecated)]
                deprecated: None,
                range: Range::new(Position::new(2, 0), Position::new(3, 10)),
                selection_range: Range::new(Position::new(2, 0), Position::new(2, 3)),
                children: Some(Vec::new()),
            },
            DocumentSymbol {
                name: "b".into(),
                detail: None,
                kind: SymbolKind::FUNCTION,
                tags: None,
                #[allow(deprecated)]
                deprecated: None,
                range: Range::new(Position::new(5, 0), Position::new(7, 3)),
                selection_range: Range::new(Position::new(5, 0), Position::new(5, 1)),
                children: Some(Vec::new()),
            },
        ];
        let bounds = children_bounding_range(&symbols).unwrap();
        assert_eq!(bounds.start, Position::new(2, 0));
        assert_eq!(bounds.end, Position::new(7, 3));
    }

    #[test]
    fn basename_from_uri_strips_tarn_yaml_suffix() {
        let u = Url::parse("file:///projects/suite/login.tarn.yaml").unwrap();
        assert_eq!(basename_from_uri(&u).as_deref(), Some("login"));
        let u2 = Url::parse("file:///projects/suite/login.tarn.yml").unwrap();
        assert_eq!(basename_from_uri(&u2).as_deref(), Some("login"));
    }
}
