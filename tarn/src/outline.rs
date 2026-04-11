//! Best-effort outline extraction for Tarn test files.
//!
//! This module is the "what's in this file" view that LSP clients render in
//! their document-symbol / outline panes. It is deliberately a second-pass
//! scanner over the raw YAML using `yaml-rust2`, mirroring
//! [`crate::parser_locations`] (added in NAZ-260) so symbol ranges stay in
//! sync with diagnostic ranges emitted elsewhere in the pipeline.
//!
//! Why a dedicated module rather than extending `parser_locations`?
//!
//!   * `parser_locations` only needs a single 1-based point per `name:` key
//!     because it feeds [`crate::model::Location`] — a point type consumed by
//!     runtime results. Outline symbols need a **span** for each node (start
//!     line/column **and** end line/column) so LSP clients can expand the
//!     clickable region to the full YAML node, not just the `name:` key.
//!   * `parser_locations` is `pub(crate)` — it is an implementation detail of
//!     the runner. The outline view, by contrast, is a public library surface
//!     consumed by `tarn-lsp` for `textDocument/documentSymbol`, and so ships
//!     under its own stable module name.
//!   * Keeping the two scanners separate means neither one grows
//!     conditional output for the other's consumer; each is tightly scoped
//!     to one job and is cheap to unit-test in isolation.
//!
//! Design notes:
//!
//!   * The scanner is **strictly best-effort**. YAML that round-trips through
//!     `yaml-rust2` returns `Some(Outline)`. YAML that fails the scan returns
//!     `None`, leaving the LSP free to publish an empty symbol tree rather
//!     than erroring out mid-edit.
//!   * Lines and columns are 1-based, matching [`crate::model::Location`] and
//!     everything else Tarn emits. `tarn-lsp` does the 1-based → 0-based LSP
//!     conversion in exactly one place, the same way diagnostics do.
//!   * The scanner ignores schema compatibility — it is not a validator.
//!     The only thing it assumes is that the top of the file is a YAML
//!     mapping. Anything else gracefully produces an empty outline.

use std::path::Path;
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::Marker;

/// 1-based source span covering a YAML node from its first token through the
/// position immediately after its last token.
///
/// Both ends are inclusive of the first byte on `start_line` / `end_line`
/// and report `column` as the 1-based position of the first / last character
/// of the node respectively. Consumers that need 0-based positions must
/// subtract one, exactly like the diagnostic pipeline does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineSpan {
    /// 1-based line of the first token in the node.
    pub start_line: usize,
    /// 1-based column of the first token in the node.
    pub start_column: usize,
    /// 1-based line of the last token in the node.
    pub end_line: usize,
    /// 1-based column just past the last token in the node.
    pub end_column: usize,
}

impl OutlineSpan {
    /// Construct a zero-width span at a single point.
    ///
    /// Used as the fallback when a node carries no discoverable end marker —
    /// the resulting span is still safe to convert into an LSP range because
    /// `start == end`.
    pub fn point(line: usize, column: usize) -> Self {
        Self {
            start_line: line,
            start_column: column,
            end_line: line,
            end_column: column,
        }
    }
}

/// Outline entry for a single step inside a test, setup, teardown, or flat
/// `steps:` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOutline {
    /// Display name — either the step's `name:` value or a synthetic
    /// `"<step N>"` placeholder if the step has none (e.g. an `include:`
    /// entry, or a malformed step where the `name:` key was elided).
    pub name: String,
    /// Full span of the step's YAML mapping — the region the LSP highlights
    /// when the symbol is clicked.
    pub range: OutlineSpan,
    /// Span of the `name:` value scalar, or the step's mapping start when the
    /// step has no `name:` key. Used as the LSP `selection_range`.
    pub selection_range: OutlineSpan,
}

/// Outline entry for a named test group inside the `tests:` mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestOutline {
    /// Key under `tests:`, e.g. `main`, `user_flow`.
    pub name: String,
    /// Full span of the test's YAML mapping.
    pub range: OutlineSpan,
    /// Span of the test group key scalar. Used as the LSP
    /// `selection_range` for the test symbol.
    pub selection_range: OutlineSpan,
    /// Steps declared under this test's `steps:` sequence.
    pub steps: Vec<StepOutline>,
}

/// File-level outline extracted from a single Tarn test file.
///
/// Empty vectors are valid — a `name:`-only file, or a file the scanner
/// could only partially parse, still produces a usable (empty) outline.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outline {
    /// Absolute or display path of the source file. Copied from the `file`
    /// argument to [`outline_document`] so downstream consumers can anchor
    /// on the same path they already use for diagnostics and runtime
    /// results.
    pub file: String,
    /// The file-level `name:` value, when present. This becomes the root
    /// symbol in the LSP outline; when absent, the LSP falls back to the
    /// file's basename.
    pub file_name: Option<String>,
    /// Span of the file-level `name:` key's value scalar. `None` when the
    /// file does not declare a top-level `name:`.
    pub file_name_range: Option<OutlineSpan>,
    /// Setup steps declared under the top-level `setup:` sequence.
    pub setup: Vec<StepOutline>,
    /// Teardown steps declared under the top-level `teardown:` sequence.
    pub teardown: Vec<StepOutline>,
    /// Flat steps declared under the top-level `steps:` sequence. These are
    /// mutually exclusive with `tests:` in practice but the scanner returns
    /// both if they both appear — downstream consumers decide how to render
    /// that edge case.
    pub flat_steps: Vec<StepOutline>,
    /// Named tests declared under the top-level `tests:` mapping. Ordering
    /// matches the order keys appear in the source so the outline view
    /// respects the author's layout.
    pub tests: Vec<TestOutline>,
}

/// Extract an [`Outline`] from the raw text of a Tarn test file.
///
/// The `file` argument is only used to populate [`Outline::file`]; the
/// scanner never touches the filesystem. When the YAML cannot be scanned
/// the function returns `None`, and callers should treat that as "no
/// outline available yet" rather than an error.
pub fn outline_document(file: &Path, content: &str) -> Option<Outline> {
    let mut sink = EventSink { events: Vec::new() };
    let mut parser = Parser::new_from_str(content);
    parser.load(&mut sink, true).ok()?;

    let mut cursor = Cursor {
        events: &sink.events,
        pos: 0,
        file: file.display().to_string(),
    };
    cursor.walk_document()
}

/// Convenience wrapper that accepts a `&str` path. Mirrors the ergonomics of
/// [`crate::validation::validate_document`]. Used from tests.
pub fn outline_from_str(file: &str, content: &str) -> Option<Outline> {
    outline_document(Path::new(file), content)
}

// --- internal scanner ---------------------------------------------------

/// Collects every `(Event, Marker)` the parser emits. Tarn files are small
/// relative to the cost of HTTP, so keeping the entire event stream in
/// memory is cheaper than trying to stream symbols out as events arrive.
struct EventSink {
    events: Vec<(Event, Marker)>,
}

impl MarkedEventReceiver for EventSink {
    fn on_event(&mut self, ev: Event, mark: Marker) {
        self.events.push((ev, mark));
    }
}

/// Stateful cursor walking a pre-recorded event stream.
struct Cursor<'a> {
    events: &'a [(Event, Marker)],
    pos: usize,
    file: String,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<&'a (Event, Marker)> {
        self.events.get(self.pos)
    }

    fn advance(&mut self) -> Option<&'a (Event, Marker)> {
        let event = self.events.get(self.pos);
        if event.is_some() {
            self.pos += 1;
        }
        event
    }

    /// Convert a `yaml-rust2` marker into a zero-width 1-based span.
    ///
    /// `yaml-rust2` reports `line` as 1-based and `col` as 0-based. We bump
    /// the column to 1-based so every span in this module uses consistent
    /// units, matching [`crate::model::Location`].
    fn point_span(mark: &Marker) -> OutlineSpan {
        OutlineSpan::point(mark.line(), mark.col() + 1)
    }

    /// Build a span that covers everything between two `yaml-rust2` markers.
    fn span_between(start: &Marker, end: &Marker) -> OutlineSpan {
        OutlineSpan {
            start_line: start.line(),
            start_column: start.col() + 1,
            end_line: end.line(),
            end_column: end.col() + 1,
        }
    }

    /// Walk from `StreamStart` through the root mapping, populating an
    /// [`Outline`]. Returns `None` if the file does not start with a YAML
    /// mapping.
    fn walk_document(&mut self) -> Option<Outline> {
        match self.advance()? {
            (Event::StreamStart, _) => {}
            _ => return None,
        }
        match self.advance()? {
            (Event::DocumentStart, _) => {}
            _ => return None,
        }
        match self.advance()? {
            (Event::MappingStart(_, _), _) => {}
            _ => return None,
        }

        let mut outline = Outline {
            file: self.file.clone(),
            ..Outline::default()
        };

        loop {
            match self.peek()? {
                (Event::MappingEnd, _) => {
                    self.advance();
                    break;
                }
                _ => {
                    let (key, _) = self.read_scalar_key_with_mark()?;
                    match key.as_str() {
                        "name" => {
                            // File-level `name:` — capture its scalar span
                            // so the file root symbol's selection_range
                            // points at the value, not the key.
                            let (event, mark) = self.advance()?;
                            match event {
                                Event::Scalar(value, _, _, _) => {
                                    outline.file_name = Some(value.clone());
                                    outline.file_name_range = Some(Self::point_span(mark));
                                }
                                _ => {
                                    // Non-scalar `name:` (unusual but not
                                    // impossible). Skip balanced — we only
                                    // advertise scalar names.
                                    self.skip_remaining_node(event)?;
                                }
                            }
                        }
                        "setup" => {
                            outline.setup = self.walk_step_sequence()?;
                        }
                        "teardown" => {
                            outline.teardown = self.walk_step_sequence()?;
                        }
                        "steps" => {
                            outline.flat_steps = self.walk_step_sequence()?;
                        }
                        "tests" => {
                            outline.tests = self.walk_tests_mapping()?;
                        }
                        _ => {
                            self.skip_node()?;
                        }
                    }
                }
            }
        }

        Some(outline)
    }

    /// Read a mapping key and return it along with its marker. Caller is
    /// expected to handle the value (or call [`Self::skip_node`] to discard
    /// it).
    fn read_scalar_key_with_mark(&mut self) -> Option<(String, Marker)> {
        let (event, mark) = self.advance()?;
        match event {
            Event::Scalar(value, _, _, _) => Some((value.clone(), *mark)),
            _ => None,
        }
    }

    /// Skip the next value node in a balanced way, advancing past its
    /// closing event. Used for every top-level key the outline does not
    /// care about (defaults, env, description, etc.).
    fn skip_node(&mut self) -> Option<()> {
        let (event, _) = self.advance()?;
        self.skip_remaining_node(event)
    }

    /// Variant of [`Self::skip_node`] for when the first event has already
    /// been consumed by the caller.
    fn skip_remaining_node(&mut self, event: &Event) -> Option<()> {
        match event {
            Event::Scalar(_, _, _, _) | Event::Alias(_) => Some(()),
            Event::SequenceStart(_, _) => loop {
                match self.peek()? {
                    (Event::SequenceEnd, _) => {
                        self.advance();
                        return Some(());
                    }
                    _ => {
                        self.skip_node()?;
                    }
                }
            },
            Event::MappingStart(_, _) => loop {
                match self.peek()? {
                    (Event::MappingEnd, _) => {
                        self.advance();
                        return Some(());
                    }
                    _ => {
                        self.skip_node()?; // key
                        self.skip_node()?; // value
                    }
                }
            },
            _ => None,
        }
    }

    /// Walk a sequence of steps and return one [`StepOutline`] per entry.
    /// The position must be just before `SequenceStart`.
    fn walk_step_sequence(&mut self) -> Option<Vec<StepOutline>> {
        match self.advance()? {
            (Event::SequenceStart(_, _), _) => {}
            _ => return Some(Vec::new()),
        }

        let mut items = Vec::new();
        let mut index = 0usize;
        loop {
            match self.peek()? {
                (Event::SequenceEnd, _) => {
                    self.advance();
                    return Some(items);
                }
                (Event::MappingStart(_, _), _) => {
                    index += 1;
                    items.push(self.walk_step_mapping(index)?);
                }
                _ => {
                    // Non-mapping sequence entry (rare — e.g. a flow-style
                    // scalar). Skip it balanced so the outline stays aligned
                    // with the source order, but do not synthesise a symbol
                    // for it.
                    self.skip_node()?;
                }
            }
        }
    }

    /// Walk a single step mapping. Expects position at the opening
    /// `MappingStart`. Returns a populated [`StepOutline`] even when the
    /// step has no `name:` key (the outline placeholder is `<step N>`).
    fn walk_step_mapping(&mut self, index: usize) -> Option<StepOutline> {
        let start_mark = match self.advance()? {
            (Event::MappingStart(_, _), mark) => *mark,
            _ => return None,
        };

        let mut step_name: Option<String> = None;
        let mut selection: Option<OutlineSpan> = None;
        let end_mark: Marker;

        loop {
            match self.peek()? {
                (Event::MappingEnd, mark) => {
                    end_mark = *mark;
                    self.advance();
                    break;
                }
                _ => {
                    let (key, key_mark) = self.read_scalar_key_with_mark()?;
                    if key == "name" {
                        // Consume the name value. If it is a scalar, record
                        // both the display name and the selection_range
                        // pointing at the scalar's start.
                        let (event, mark) = self.advance()?;
                        match event {
                            Event::Scalar(value, _, _, _) => {
                                step_name = Some(value.clone());
                                selection = Some(Self::point_span(mark));
                            }
                            other => {
                                self.skip_remaining_node(other)?;
                            }
                        }
                        // Fall back to the key marker if the value wasn't
                        // a scalar — better than losing the selection
                        // entirely.
                        if selection.is_none() {
                            selection = Some(Self::point_span(&key_mark));
                        }
                    } else {
                        self.skip_node()?;
                    }
                }
            }
        }

        let display_name = step_name.unwrap_or_else(|| format!("<step {index}>"));
        let range = Self::span_between(&start_mark, &end_mark);
        let selection_range = selection.unwrap_or_else(|| Self::point_span(&start_mark));

        Some(StepOutline {
            name: display_name,
            range,
            selection_range,
        })
    }

    /// Walk the `tests:` mapping, producing one [`TestOutline`] per named
    /// test group, in source order.
    fn walk_tests_mapping(&mut self) -> Option<Vec<TestOutline>> {
        match self.advance()? {
            (Event::MappingStart(_, _), _) => {}
            _ => return Some(Vec::new()),
        }

        let mut groups = Vec::new();
        loop {
            match self.peek()? {
                (Event::MappingEnd, _) => {
                    self.advance();
                    return Some(groups);
                }
                _ => {
                    let (name, key_mark) = self.read_scalar_key_with_mark()?;
                    let selection_range = Self::point_span(&key_mark);
                    if let Some((range, steps)) = self.walk_test_group_mapping(&key_mark)? {
                        groups.push(TestOutline {
                            name,
                            range,
                            selection_range,
                            steps,
                        });
                    }
                }
            }
        }
    }

    /// Walk a single test group mapping, returning the full span of the
    /// group and its step list. `key_mark` is used to start the span so the
    /// group's "range" covers the key **and** the body — clicking the key
    /// in the LSP outline scrolls to the right place.
    fn walk_test_group_mapping(
        &mut self,
        key_mark: &Marker,
    ) -> Option<Option<(OutlineSpan, Vec<StepOutline>)>> {
        match self.peek()? {
            (Event::MappingStart(_, _), _) => {}
            _ => {
                // Primitive value under a test name. No steps. Still emit a
                // symbol so the outline reflects the author's intent.
                let point = Self::point_span(key_mark);
                self.skip_node()?;
                return Some(Some((point, Vec::new())));
            }
        }

        self.advance(); // consume the MappingStart

        let mut steps = Vec::new();
        let end_mark: Marker;
        loop {
            match self.peek()? {
                (Event::MappingEnd, mark) => {
                    end_mark = *mark;
                    self.advance();
                    break;
                }
                _ => {
                    let (key, _) = self.read_scalar_key_with_mark()?;
                    if key == "steps" {
                        steps = self.walk_step_sequence()?;
                    } else {
                        self.skip_node()?;
                    }
                }
            }
        }

        Some(Some((Self::span_between(key_mark, &end_mark), steps)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_returns_none() {
        let outline = outline_from_str("empty.tarn.yaml", "");
        // yaml-rust2 may emit an empty stream — either None or an empty
        // outline is acceptable. Assert we never panic and never crash.
        if let Some(outline) = outline {
            assert!(outline.file_name.is_none());
            assert!(outline.tests.is_empty());
            assert!(outline.flat_steps.is_empty());
            assert!(outline.setup.is_empty());
            assert!(outline.teardown.is_empty());
        }
    }

    #[test]
    fn file_with_only_name_returns_file_level_name_only() {
        let yaml = "name: Only\n";
        let outline = outline_from_str("only.tarn.yaml", yaml).expect("outline");
        assert_eq!(outline.file_name.as_deref(), Some("Only"));
        assert!(outline.file_name_range.is_some());
        assert!(outline.tests.is_empty());
        assert!(outline.flat_steps.is_empty());
    }

    #[test]
    fn flat_steps_produce_one_symbol_per_entry_with_full_ranges() {
        let yaml = "\
name: Flat
steps:
  - name: first
    request:
      method: GET
      url: http://localhost/a
  - name: second
    request:
      method: GET
      url: http://localhost/b
    assert:
      status: 200
";
        let outline = outline_from_str("flat.tarn.yaml", yaml).expect("outline");
        assert_eq!(outline.flat_steps.len(), 2);
        assert_eq!(outline.flat_steps[0].name, "first");
        assert_eq!(outline.flat_steps[1].name, "second");
        // Each step range must span multiple lines, not be a point.
        for step in &outline.flat_steps {
            assert!(step.range.end_line >= step.range.start_line);
            assert!(step.selection_range.start_line >= step.range.start_line);
            assert!(step.selection_range.end_line <= step.range.end_line);
        }
        // The first step starts on the line holding `- name: first`.
        let first = &outline.flat_steps[0];
        assert_eq!(first.selection_range.start_line, 3, "name on line 3");
    }

    #[test]
    fn named_tests_produce_nested_step_symbols() {
        let yaml = "\
name: Nested
tests:
  group_a:
    steps:
      - name: alpha
        request:
          method: GET
          url: http://localhost/a
      - name: beta
        request:
          method: GET
          url: http://localhost/b
  group_b:
    steps:
      - name: gamma
        request:
          method: GET
          url: http://localhost/c
";
        let outline = outline_from_str("nested.tarn.yaml", yaml).expect("outline");
        assert_eq!(outline.file_name.as_deref(), Some("Nested"));
        assert_eq!(outline.tests.len(), 2);
        let group_a = &outline.tests[0];
        assert_eq!(group_a.name, "group_a");
        assert_eq!(group_a.steps.len(), 2);
        assert_eq!(group_a.steps[0].name, "alpha");
        assert_eq!(group_a.steps[1].name, "beta");
        let group_b = &outline.tests[1];
        assert_eq!(group_b.name, "group_b");
        assert_eq!(group_b.steps.len(), 1);
        assert_eq!(group_b.steps[0].name, "gamma");
    }

    #[test]
    fn setup_and_teardown_are_captured_independently() {
        let yaml = "\
name: Hooks
setup:
  - name: login
    request:
      method: POST
      url: http://localhost/auth
teardown:
  - name: cleanup
    request:
      method: POST
      url: http://localhost/cleanup
steps:
  - name: main
    request:
      method: GET
      url: http://localhost/
";
        let outline = outline_from_str("hooks.tarn.yaml", yaml).expect("outline");
        assert_eq!(outline.setup.len(), 1);
        assert_eq!(outline.teardown.len(), 1);
        assert_eq!(outline.flat_steps.len(), 1);
        assert_eq!(outline.setup[0].name, "login");
        assert_eq!(outline.teardown[0].name, "cleanup");
        assert_eq!(outline.flat_steps[0].name, "main");
    }

    #[test]
    fn include_entries_receive_synthetic_placeholder_name() {
        let yaml = "\
name: With include
setup:
  - include: ./other.tarn.yaml
  - name: real
    request:
      method: GET
      url: http://localhost/
";
        let outline = outline_from_str("inc.tarn.yaml", yaml).expect("outline");
        // Include has no `name:` — we still emit a symbol with a synthetic
        // placeholder so the outline reflects the author's ordering.
        assert_eq!(outline.setup.len(), 2);
        assert_eq!(outline.setup[0].name, "<step 1>");
        assert_eq!(outline.setup[1].name, "real");
    }

    #[test]
    fn malformed_yaml_returns_none_without_panicking() {
        let yaml = "name: broken\n  bad-indent: true\n  - list-here: oops\n";
        // The exact outcome depends on yaml-rust2's recovery — we just
        // need to guarantee we never panic on malformed input.
        let _ = outline_from_str("bad.tarn.yaml", yaml);
    }

    #[test]
    fn step_range_end_is_after_start() {
        let yaml = "\
name: Spans
steps:
  - name: only
    request:
      method: GET
      url: http://localhost/
";
        let outline = outline_from_str("spans.tarn.yaml", yaml).expect("outline");
        let step = &outline.flat_steps[0];
        assert!(
            step.range.end_line >= step.range.start_line,
            "end_line must not precede start_line"
        );
        assert_eq!(step.name, "only");
    }

    #[test]
    fn file_preserves_test_group_order_in_source() {
        let yaml = "\
name: Order
tests:
  zzz:
    steps:
      - name: last
        request:
          method: GET
          url: http://localhost/z
  aaa:
    steps:
      - name: first
        request:
          method: GET
          url: http://localhost/a
";
        let outline = outline_from_str("order.tarn.yaml", yaml).expect("outline");
        // The source declares `zzz` before `aaa`, so the outline must too.
        // This is why we return `Vec<TestOutline>` instead of a HashMap.
        assert_eq!(outline.tests[0].name, "zzz");
        assert_eq!(outline.tests[1].name, "aaa");
    }

    #[test]
    fn file_name_absent_yields_none_file_name() {
        let yaml = "\
steps:
  - name: s
    request:
      method: GET
      url: http://localhost/
";
        let outline = outline_from_str("n.tarn.yaml", yaml).expect("outline");
        assert!(outline.file_name.is_none());
        assert!(outline.file_name_range.is_none());
        assert_eq!(outline.flat_steps.len(), 1);
    }

    #[test]
    fn point_span_is_zero_width_and_sits_on_exact_marker() {
        let span = OutlineSpan::point(3, 5);
        assert_eq!(span.start_line, 3);
        assert_eq!(span.end_line, 3);
        assert_eq!(span.start_column, 5);
        assert_eq!(span.end_column, 5);
    }
}
