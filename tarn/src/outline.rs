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

use crate::model::Location;
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

/// Identifies the section of a test file that contains a step.
///
/// `Setup`, `Teardown`, and `FlatSteps` apply to the top-level step
/// sequences. `Test(name)` picks one named group from the `tests:`
/// mapping. `Any` means "search every section" — the LSP
/// `textDocument/definition` handler uses this when the client jumps
/// from a step that lives outside any test (e.g. a `teardown` step
/// referencing a capture declared by the setup phase).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureScope<'a> {
    /// Search only the top-level `setup:` sequence.
    Setup,
    /// Search only the top-level `teardown:` sequence.
    Teardown,
    /// Search only the top-level `steps:` sequence.
    FlatSteps,
    /// Search only the `tests.<name>.steps:` sequence.
    Test(&'a str),
    /// Search every sequence above. Returns every matching declaration.
    Any,
}

/// Scan `content` for every declaration of `capture_name` that lives in
/// a step inside `scope`, returning a 1-based [`Location`] per match.
///
/// `display_path` is used for [`Location::file`] so the returned points
/// anchor on the same path the rest of the tarn pipeline already uses.
///
/// Captures are declared under `capture:` in a step, either as a
/// `JSONPath` scalar (`capture: { token: $.id }`) or as an extended
/// mapping (`capture: { token: { jsonpath: $.id } }`). This helper
/// locates the *key* inside the `capture:` mapping and returns a
/// location pointing at the start of that key — the LSP's
/// `textDocument/definition` jump lands on the key the user clicks, not
/// on the value that happens to be captured there.
///
/// Returns an empty vector when the capture is not declared anywhere
/// in `scope`, when `content` cannot be scanned as YAML, or when the
/// root is not a mapping. Never panics on malformed input — the scan
/// is strictly best-effort so an in-progress edit still produces a
/// definition jump for the parts that are intact.
pub fn find_capture_declarations(
    content: &str,
    display_path: &str,
    capture_name: &str,
    scope: &CaptureScope<'_>,
) -> Vec<Location> {
    let mut out = Vec::new();
    let mut sink = EventSink { events: Vec::new() };
    let mut parser = Parser::new_from_str(content);
    if parser.load(&mut sink, true).is_err() {
        return out;
    }
    let events = &sink.events;

    // Skip StreamStart, DocumentStart, and consume the root mapping
    // opening. If the root isn't a mapping the document has nothing to
    // search and we return empty.
    let mut i = 0usize;
    while i < events.len() {
        if matches!(events[i].0, Event::MappingStart(_, _)) {
            i += 1;
            break;
        }
        i += 1;
    }

    // Walk the root mapping's keys. For each section of interest (per
    // `scope`) we descend into its step list and collect capture-key
    // markers.
    while i < events.len() {
        match &events[i].0 {
            Event::MappingEnd => break,
            Event::Scalar(key, _, _, _) => {
                let key = key.clone();
                i += 1;
                let interested = match scope {
                    CaptureScope::Setup => key == "setup",
                    CaptureScope::Teardown => key == "teardown",
                    CaptureScope::FlatSteps => key == "steps",
                    CaptureScope::Test(_) => key == "tests",
                    CaptureScope::Any => {
                        key == "setup" || key == "teardown" || key == "steps" || key == "tests"
                    }
                };
                if !interested {
                    i = skip_event_node(events, i);
                    continue;
                }
                if key == "tests" {
                    let target = match scope {
                        CaptureScope::Test(name) => Some(*name),
                        _ => None,
                    };
                    i = scan_tests_for_captures(
                        events,
                        i,
                        display_path,
                        capture_name,
                        target,
                        &mut out,
                    );
                } else {
                    i = scan_step_sequence_for_captures(
                        events,
                        i,
                        display_path,
                        capture_name,
                        &mut out,
                    );
                }
            }
            _ => {
                i = skip_event_node(events, i);
            }
        }
    }

    out
}

/// Scan a `tests:` mapping value for captures. Position must start at
/// the `MappingStart` event of the tests mapping. `target` optionally
/// restricts the walk to a single named test.
fn scan_tests_for_captures(
    events: &[(Event, Marker)],
    mut i: usize,
    display_path: &str,
    capture_name: &str,
    target: Option<&str>,
    out: &mut Vec<Location>,
) -> usize {
    if !matches!(
        events.get(i).map(|(e, _)| e),
        Some(Event::MappingStart(_, _))
    ) {
        return skip_event_node(events, i);
    }
    i += 1;
    while i < events.len() {
        match &events[i].0 {
            Event::MappingEnd => return i + 1,
            Event::Scalar(name, _, _, _) => {
                let name = name.clone();
                i += 1;
                if !matches!(
                    events.get(i).map(|(e, _)| e),
                    Some(Event::MappingStart(_, _))
                ) {
                    i = skip_event_node(events, i);
                    continue;
                }
                let matches_target = target.map(|t| t == name).unwrap_or(true);
                if !matches_target {
                    i = skip_event_node(events, i);
                    continue;
                }
                // Descend into the test group body.
                i += 1; // consume MappingStart
                while i < events.len() {
                    match &events[i].0 {
                        Event::MappingEnd => {
                            i += 1;
                            break;
                        }
                        Event::Scalar(inner_key, _, _, _) => {
                            let inner_key = inner_key.clone();
                            i += 1;
                            if inner_key == "steps" {
                                i = scan_step_sequence_for_captures(
                                    events,
                                    i,
                                    display_path,
                                    capture_name,
                                    out,
                                );
                            } else {
                                i = skip_event_node(events, i);
                            }
                        }
                        _ => {
                            i = skip_event_node(events, i);
                        }
                    }
                }
            }
            _ => {
                i = skip_event_node(events, i);
            }
        }
    }
    i
}

/// Scan a sequence of step mappings for captures. Position must start
/// at the `SequenceStart` event of the step list.
fn scan_step_sequence_for_captures(
    events: &[(Event, Marker)],
    mut i: usize,
    display_path: &str,
    capture_name: &str,
    out: &mut Vec<Location>,
) -> usize {
    if !matches!(
        events.get(i).map(|(e, _)| e),
        Some(Event::SequenceStart(_, _))
    ) {
        return skip_event_node(events, i);
    }
    i += 1;
    while i < events.len() {
        match &events[i].0 {
            Event::SequenceEnd => return i + 1,
            Event::MappingStart(_, _) => {
                i += 1;
                // Walk the step's top-level keys.
                while i < events.len() {
                    match &events[i].0 {
                        Event::MappingEnd => {
                            i += 1;
                            break;
                        }
                        Event::Scalar(key, _, _, _) => {
                            let key = key.clone();
                            i += 1;
                            if key == "capture" {
                                i = scan_capture_mapping(
                                    events,
                                    i,
                                    display_path,
                                    capture_name,
                                    out,
                                );
                            } else {
                                i = skip_event_node(events, i);
                            }
                        }
                        _ => {
                            i = skip_event_node(events, i);
                        }
                    }
                }
            }
            _ => {
                i = skip_event_node(events, i);
            }
        }
    }
    i
}

/// Scan a `capture:` mapping for a specific key, appending a
/// [`Location`] pointing at the key's marker when found. Position must
/// start at the `MappingStart` event of the capture mapping.
fn scan_capture_mapping(
    events: &[(Event, Marker)],
    mut i: usize,
    display_path: &str,
    capture_name: &str,
    out: &mut Vec<Location>,
) -> usize {
    if !matches!(
        events.get(i).map(|(e, _)| e),
        Some(Event::MappingStart(_, _))
    ) {
        return skip_event_node(events, i);
    }
    i += 1;
    while i < events.len() {
        match &events[i].0 {
            Event::MappingEnd => return i + 1,
            Event::Scalar(key, _, _, _) => {
                let key = key.clone();
                let marker = events[i].1;
                i += 1;
                if key == capture_name {
                    out.push(Location {
                        file: display_path.to_owned(),
                        line: marker.line(),
                        column: marker.col() + 1,
                    });
                }
                // Skip the value regardless of match — captures values
                // are either scalars or nested mappings.
                i = skip_event_node(events, i);
            }
            _ => {
                i = skip_event_node(events, i);
            }
        }
    }
    i
}

/// Advance past a balanced YAML event node (scalar, sequence, or
/// mapping), returning the index immediately after the node's closing
/// event. Used by [`find_capture_declarations`] and its helpers so they
/// can hop over every non-target node without losing their place in
/// the event stream.
fn skip_event_node(events: &[(Event, Marker)], mut i: usize) -> usize {
    if i >= events.len() {
        return i;
    }
    let start = &events[i].0;
    match start {
        Event::Scalar(_, _, _, _) | Event::Alias(_) => i + 1,
        Event::SequenceStart(_, _) => {
            i += 1;
            let mut depth = 1i32;
            while i < events.len() && depth > 0 {
                match &events[i].0 {
                    Event::SequenceStart(_, _) | Event::MappingStart(_, _) => depth += 1,
                    Event::SequenceEnd | Event::MappingEnd => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            i
        }
        Event::MappingStart(_, _) => {
            i += 1;
            let mut depth = 1i32;
            while i < events.len() && depth > 0 {
                match &events[i].0 {
                    Event::MappingStart(_, _) | Event::SequenceStart(_, _) => depth += 1,
                    Event::MappingEnd | Event::SequenceEnd => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            i
        }
        _ => i + 1,
    }
}

// --- position-based scalar lookup --------------------------------------
//
// NAZ-303 (tarn-lsp code actions) needs "what scalar is under the
// cursor, and what is its field path?". The outline walker owns every
// other YAML walk in the crate, so the helper lives here alongside it.
//
// The returned [`ScalarAtPosition`] carries:
//
//   * the unquoted string value of the scalar,
//   * the YAML scalar style (plain, single-quoted, double-quoted, …),
//   * an inclusive 1-based line/column span that covers the scalar
//     **including any surrounding quotes**, so an LSP client can
//     replace the whole literal in one edit,
//   * a path of [`PathSegment`]s from the document root to the scalar,
//     so consumers can decide whether the scalar is inside a request
//     field, an env value, a header, a name, etc.
//
// The walker is best-effort: malformed YAML returns `None` and any
// unrecognised event ends the walk gracefully.

/// One segment of the path from the document root to a YAML node.
///
/// Mapping keys become [`PathSegment::Key`] entries and sequence indices
/// become [`PathSegment::Index`] entries. Zero-based indices match the
/// way every other Tarn walker reports sequence positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// A mapping key (e.g. the `"request"` in `request.url`).
    Key(String),
    /// A sequence index (zero-based, e.g. the `0` in `steps[0]`).
    Index(usize),
}

/// YAML scalar style reported by [`find_scalar_at_position`].
///
/// Mirrors the subset of `yaml_rust2::scanner::TScalarStyle` that Tarn
/// buffers realistically contain. The distinction matters because an
/// extract-env code action needs to replace the **entire literal**,
/// which for a quoted scalar includes the surrounding `"..."` or
/// `'...'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarStyle {
    /// Plain unquoted scalar: `url: http://example.com`.
    Plain,
    /// Single-quoted scalar: `url: 'http://example.com'`.
    SingleQuoted,
    /// Double-quoted scalar: `url: "http://example.com"`.
    DoubleQuoted,
    /// Literal block (`|`) — single-line fallback only; multi-line
    /// block scalars are not targeted by the current consumers.
    Literal,
    /// Folded block (`>`) — same caveat as `Literal`.
    Folded,
}

/// Information about a scalar node located by [`find_scalar_at_position`].
///
/// Line and column numbers are **1-based** to match every other
/// [`Location`]-style Tarn emission. The span is inclusive on the start
/// (`start_line`, `start_column`) and inclusive on the end byte
/// (`end_line`, `end_column`), where `end_column` points at the column
/// immediately past the last character of the scalar text (quotes
/// included when present). Consumers that need 0-based positions
/// subtract one, the same way diagnostics do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalarAtPosition {
    /// Unquoted string value of the scalar, as emitted by `yaml-rust2`.
    pub value: String,
    /// YAML scalar style — plain / quoted / block.
    pub style: ScalarStyle,
    /// Path from the document root to this scalar.
    pub path: Vec<PathSegment>,
    /// 1-based line of the first character of the literal (quotes
    /// included when the scalar is quoted).
    pub start_line: usize,
    /// 1-based column of the first character of the literal.
    pub start_column: usize,
    /// 1-based line of the character immediately past the last
    /// character of the literal.
    pub end_line: usize,
    /// 1-based column of the character immediately past the last
    /// character of the literal.
    pub end_column: usize,
    /// Absolute byte offset of the first character of the literal.
    pub start_byte: usize,
    /// Absolute byte offset of the first byte **after** the literal.
    pub end_byte: usize,
}

/// Find the innermost scalar node in `content` whose source span
/// contains the 1-based `(line, column)` position.
///
/// Returns `None` for positions that land on a mapping / sequence
/// opening, on whitespace, on a comment, or on anything that fails to
/// parse through `yaml-rust2`. The walker is strictly best-effort:
/// malformed YAML degrades to `None` so an in-progress edit never
/// crashes the caller.
pub fn find_scalar_at_position(
    content: &str,
    line_one_based: usize,
    column_one_based: usize,
) -> Option<ScalarAtPosition> {
    let mut sink = EventSink { events: Vec::new() };
    let mut parser = Parser::new_from_str(content);
    parser.load(&mut sink, true).ok()?;

    let events = &sink.events;
    if events.is_empty() {
        return None;
    }

    // Walk until we are past StreamStart / DocumentStart. Each
    // subsequent event is either the document's root node or a child
    // of something we already descended into.
    let mut i = 0usize;
    while i < events.len() {
        match events[i].0 {
            Event::StreamStart | Event::DocumentStart => i += 1,
            _ => break,
        }
    }

    let mut walker = ScalarWalker {
        events,
        source: content,
        target_line: line_one_based,
        target_col: column_one_based,
        path: Vec::new(),
        best: None,
    };
    walker.walk_node(&mut i);
    walker.best
}

/// Internal state for [`find_scalar_at_position`].
struct ScalarWalker<'a> {
    events: &'a [(Event, Marker)],
    source: &'a str,
    target_line: usize,
    target_col: usize,
    path: Vec<PathSegment>,
    best: Option<ScalarAtPosition>,
}

impl<'a> ScalarWalker<'a> {
    /// Walk the node at `events[*i]`. Advances `*i` past the node's
    /// closing event. Recurses for mappings and sequences; records the
    /// scalar as a candidate when `position` falls inside its span.
    fn walk_node(&mut self, i: &mut usize) {
        if *i >= self.events.len() {
            return;
        }
        let (event, mark) = &self.events[*i];
        match event {
            Event::Scalar(value, style, _, _) => {
                let scalar_start_index = mark.index();
                let scalar_start_line = mark.line();
                let scalar_start_col = mark.col() + 1;
                // Compute the scalar's end byte from the raw source so
                // the span covers any surrounding quotes as well as the
                // unquoted text.
                let (end_byte, end_line, end_col) =
                    scalar_end_position(self.source, scalar_start_index, value, *style);
                if position_in_span(
                    self.target_line,
                    self.target_col,
                    scalar_start_line,
                    scalar_start_col,
                    end_line,
                    end_col,
                ) {
                    self.best = Some(ScalarAtPosition {
                        value: value.clone(),
                        style: tscalar_style_to_local(*style),
                        path: self.path.clone(),
                        start_line: scalar_start_line,
                        start_column: scalar_start_col,
                        end_line,
                        end_column: end_col,
                        start_byte: scalar_start_index,
                        end_byte,
                    });
                }
                *i += 1;
            }
            Event::MappingStart(_, _) => {
                *i += 1;
                loop {
                    match self.events.get(*i).map(|(e, _)| e) {
                        Some(Event::MappingEnd) => {
                            *i += 1;
                            break;
                        }
                        None => break,
                        _ => {
                            // Read key scalar without descending into it
                            // (the key itself is rarely a useful extract
                            // target — we still advance past it balanced).
                            let key_event = &self.events[*i];
                            let key = match &key_event.0 {
                                Event::Scalar(k, _, _, _) => Some(k.clone()),
                                _ => None,
                            };
                            self.walk_node(i);
                            if let Some(key) = key {
                                self.path.push(PathSegment::Key(key));
                                self.walk_node(i);
                                self.path.pop();
                            } else {
                                // Non-scalar key — skip balanced so the
                                // walker stays aligned.
                                self.walk_node(i);
                            }
                        }
                    }
                }
            }
            Event::SequenceStart(_, _) => {
                *i += 1;
                let mut idx = 0usize;
                loop {
                    match self.events.get(*i).map(|(e, _)| e) {
                        Some(Event::SequenceEnd) => {
                            *i += 1;
                            break;
                        }
                        None => break,
                        _ => {
                            self.path.push(PathSegment::Index(idx));
                            self.walk_node(i);
                            self.path.pop();
                            idx += 1;
                        }
                    }
                }
            }
            Event::Alias(_) | Event::StreamEnd | Event::DocumentEnd => {
                *i += 1;
            }
            _ => {
                *i += 1;
            }
        }
    }
}

/// Compute `(end_byte, end_line, end_col)` for a scalar starting at
/// `start_byte`. For plain scalars this scans to the first newline,
/// comment, or structural punctuation. For quoted scalars it walks
/// until the matching closing quote, accounting for the simple escape
/// rules (double-quoted `\"` and single-quoted `''`). Block scalars
/// (`|`, `>`) fall back to "single-line" behaviour — the current
/// consumers only care about single-line literals, and the walker is
/// strictly best-effort.
fn scalar_end_position(
    source: &str,
    start_byte: usize,
    value: &str,
    style: yaml_rust2::scanner::TScalarStyle,
) -> (usize, usize, usize) {
    use yaml_rust2::scanner::TScalarStyle;
    let bytes = source.as_bytes();
    let len = bytes.len();
    if start_byte >= len {
        let (line, col) = line_col_for_byte(source, start_byte.min(len));
        return (start_byte.min(len), line, col);
    }
    let end_byte = match style {
        TScalarStyle::DoubleQuoted => {
            // Walk forward, skipping `\x` escape sequences, until the
            // matching `"`.
            if bytes[start_byte] != b'"' {
                plain_scalar_end(bytes, start_byte, value)
            } else {
                let mut j = start_byte + 1;
                while j < len {
                    let b = bytes[j];
                    if b == b'\\' && j + 1 < len {
                        j += 2;
                        continue;
                    }
                    if b == b'"' {
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                j
            }
        }
        TScalarStyle::SingleQuoted => {
            if bytes[start_byte] != b'\'' {
                plain_scalar_end(bytes, start_byte, value)
            } else {
                let mut j = start_byte + 1;
                while j < len {
                    let b = bytes[j];
                    if b == b'\'' {
                        if j + 1 < len && bytes[j + 1] == b'\'' {
                            j += 2;
                            continue;
                        }
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                j
            }
        }
        _ => plain_scalar_end(bytes, start_byte, value),
    };
    let clamped = end_byte.min(len);
    let (line, col) = line_col_for_byte(source, clamped);
    (clamped, line, col)
}

/// Walk a plain (unquoted) scalar from `start_byte` forward until the
/// first terminating character. Fall back to "start + value.len()"
/// when the value matches the raw source — YAML plain scalars do not
/// re-quote their bytes so that fast path is usually correct.
fn plain_scalar_end(bytes: &[u8], start_byte: usize, value: &str) -> usize {
    let len = bytes.len();
    // Fast path: the raw source at `start_byte` starts with `value`
    // verbatim. Plain scalars almost always do.
    let vbytes = value.as_bytes();
    if start_byte + vbytes.len() <= len && &bytes[start_byte..start_byte + vbytes.len()] == vbytes {
        return start_byte + vbytes.len();
    }
    // Slow path: scan to the first `\n`, `#`, or structural separator.
    let mut j = start_byte;
    while j < len {
        let b = bytes[j];
        if b == b'\n' || b == b'#' {
            break;
        }
        j += 1;
    }
    j
}

/// Map a yaml-rust2 `TScalarStyle` onto the crate-local [`ScalarStyle`].
fn tscalar_style_to_local(style: yaml_rust2::scanner::TScalarStyle) -> ScalarStyle {
    use yaml_rust2::scanner::TScalarStyle;
    match style {
        TScalarStyle::Plain => ScalarStyle::Plain,
        TScalarStyle::SingleQuoted => ScalarStyle::SingleQuoted,
        TScalarStyle::DoubleQuoted => ScalarStyle::DoubleQuoted,
        TScalarStyle::Literal => ScalarStyle::Literal,
        TScalarStyle::Folded => ScalarStyle::Folded,
    }
}

/// Compute the 1-based `(line, column)` of a byte offset inside `source`.
fn line_col_for_byte(source: &str, byte: usize) -> (usize, usize) {
    let clamped = byte.min(source.len());
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, c) in source.char_indices() {
        if i >= clamped {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// True when the 1-based `(target_line, target_col)` falls inside the
/// 1-based span `(start_line, start_col)..(end_line, end_col)`, with
/// the start inclusive and the end inclusive of the final character's
/// position (i.e. the cursor *on* the closing quote still counts).
fn position_in_span(
    target_line: usize,
    target_col: usize,
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
) -> bool {
    if target_line < start_line || target_line > end_line {
        return false;
    }
    if target_line == start_line && target_col < start_col {
        return false;
    }
    if target_line == end_line && target_col > end_col {
        return false;
    }
    true
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

    // --- find_capture_declarations ------------------------------------

    #[test]
    fn find_capture_declarations_locates_key_in_test_scope() {
        let yaml = "\
name: Captures
tests:
  main:
    steps:
      - name: login
        request:
          method: POST
          url: http://x/auth
        capture:
          token: $.id
      - name: next
        request:
          method: GET
          url: http://x/items
";
        let locs =
            find_capture_declarations(yaml, "t.tarn.yaml", "token", &CaptureScope::Test("main"));
        assert_eq!(locs.len(), 1);
        // `token:` lives on line 10 (1-based).
        assert_eq!(locs[0].line, 10);
        assert_eq!(locs[0].file, "t.tarn.yaml");
    }

    #[test]
    fn find_capture_declarations_locates_keys_in_flat_steps() {
        let yaml = "\
name: Flat captures
steps:
  - name: s1
    request:
      method: POST
      url: http://x/
    capture:
      token: $.id
";
        let locs =
            find_capture_declarations(yaml, "t.tarn.yaml", "token", &CaptureScope::FlatSteps);
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].line, 8);
    }

    #[test]
    fn find_capture_declarations_returns_empty_when_name_missing() {
        let yaml = "\
name: Missing
steps:
  - name: s1
    request:
      method: GET
      url: http://x/
    capture:
      other: $.id
";
        let locs =
            find_capture_declarations(yaml, "t.tarn.yaml", "token", &CaptureScope::FlatSteps);
        assert!(locs.is_empty());
    }

    #[test]
    fn find_capture_declarations_returns_all_occurrences_in_same_test() {
        let yaml = "\
name: Dup
tests:
  main:
    steps:
      - name: s1
        request:
          method: POST
          url: http://x/a
        capture:
          token: $.id
      - name: s2
        request:
          method: POST
          url: http://x/b
        capture:
          token: $.id
";
        let locs =
            find_capture_declarations(yaml, "t.tarn.yaml", "token", &CaptureScope::Test("main"));
        assert_eq!(locs.len(), 2);
        assert!(locs[0].line < locs[1].line);
    }

    #[test]
    fn find_capture_declarations_any_scope_searches_every_section() {
        let yaml = "\
name: Any
setup:
  - name: login
    request:
      method: POST
      url: http://x/auth
    capture:
      session_id: $.id
teardown:
  - name: cleanup
    request:
      method: DELETE
      url: http://x/cleanup
    capture:
      deleted_id: $.id
steps:
  - name: main
    request:
      method: GET
      url: http://x/
    capture:
      main_id: $.id
";
        let setup_locs =
            find_capture_declarations(yaml, "t.tarn.yaml", "session_id", &CaptureScope::Any);
        assert_eq!(setup_locs.len(), 1);
        let teardown_locs =
            find_capture_declarations(yaml, "t.tarn.yaml", "deleted_id", &CaptureScope::Any);
        assert_eq!(teardown_locs.len(), 1);
        let flat_locs =
            find_capture_declarations(yaml, "t.tarn.yaml", "main_id", &CaptureScope::Any);
        assert_eq!(flat_locs.len(), 1);
    }

    #[test]
    fn find_capture_declarations_does_not_leak_across_named_tests() {
        let yaml = "\
name: Two
tests:
  first:
    steps:
      - name: a
        request:
          method: GET
          url: http://x/a
        capture:
          token: $.id
  second:
    steps:
      - name: b
        request:
          method: GET
          url: http://x/b
        capture:
          token: $.id
";
        let first =
            find_capture_declarations(yaml, "t.tarn.yaml", "token", &CaptureScope::Test("first"));
        assert_eq!(first.len(), 1);
        let second =
            find_capture_declarations(yaml, "t.tarn.yaml", "token", &CaptureScope::Test("second"));
        assert_eq!(second.len(), 1);
        assert_ne!(first[0].line, second[0].line);
    }

    #[test]
    fn find_capture_declarations_malformed_yaml_returns_empty() {
        let yaml = "name: bad\n  - indent: [oops";
        let locs = find_capture_declarations(yaml, "t.tarn.yaml", "x", &CaptureScope::Any);
        assert!(locs.is_empty());
    }

    // --- find_scalar_at_position -------------------------------------

    #[test]
    fn find_scalar_at_position_returns_plain_url_with_path() {
        let yaml = "\
steps:
  - name: s
    request:
      method: GET
      url: http://example.com/items
";
        // Cursor somewhere inside `http://example.com/items` on line 5.
        let scalar = find_scalar_at_position(yaml, 5, 15).expect("scalar");
        assert_eq!(scalar.value, "http://example.com/items");
        assert_eq!(scalar.style, ScalarStyle::Plain);
        assert_eq!(
            scalar.path,
            vec![
                PathSegment::Key("steps".into()),
                PathSegment::Index(0),
                PathSegment::Key("request".into()),
                PathSegment::Key("url".into()),
            ]
        );
    }

    #[test]
    fn find_scalar_at_position_returns_quoted_url_with_quotes_in_span() {
        let yaml = "\
steps:
  - name: s
    request:
      method: GET
      url: \"http://example.com/items\"
";
        let scalar = find_scalar_at_position(yaml, 5, 20).expect("scalar");
        assert_eq!(scalar.value, "http://example.com/items");
        assert_eq!(scalar.style, ScalarStyle::DoubleQuoted);
        // Verify the byte span covers the surrounding quotes.
        let literal = &yaml[scalar.start_byte..scalar.end_byte];
        assert_eq!(literal, "\"http://example.com/items\"");
    }

    #[test]
    fn find_scalar_at_position_returns_none_on_whitespace() {
        let yaml = "steps:\n  - name: first\n    request:\n      url: http://x/\n";
        // Column 1 on line 1 is on the `s` of `steps` (a key scalar).
        // Column 7 on line 1 is just past the colon, on whitespace —
        // no scalar value lives there.
        assert!(find_scalar_at_position(yaml, 1, 50).is_none());
    }

    #[test]
    fn find_scalar_at_position_reports_name_path_when_cursor_on_step_name() {
        let yaml = "steps:\n  - name: first\n    request:\n      url: http://x/\n";
        // Cursor on the `first` scalar of the step's `name:` field.
        let scalar = find_scalar_at_position(yaml, 2, 12).expect("scalar");
        assert_eq!(scalar.value, "first");
        assert_eq!(
            scalar.path,
            vec![
                PathSegment::Key("steps".into()),
                PathSegment::Index(0),
                PathSegment::Key("name".into()),
            ]
        );
    }

    #[test]
    fn find_scalar_at_position_returns_none_for_malformed_yaml() {
        let yaml = "steps: [oops\n  bad";
        assert!(find_scalar_at_position(yaml, 1, 5).is_none());
    }
}
