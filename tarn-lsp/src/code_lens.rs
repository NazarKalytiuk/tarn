//! `textDocument/codeLens` handler and renderer.
//!
//! This module is the Phase L2.4 (NAZ-300) final ticket of Phase L2. It
//! emits one `Run test` lens per named test and one `Run step` lens per
//! step inside every test, mirroring the behavioural scope of the VS
//! Code extension's `TestCodeLensProvider.ts` but exposed over LSP so
//! any LSP 3.17 client — Claude Code, Neovim, Helix, Zed, etc. — can
//! render the same affordances without re-implementing the outline walk.
//!
//! # Architecture
//!
//! Structurally identical to [`crate::symbols`] (NAZ-294): a pure
//! renderer over `&Outline` that the dispatcher in [`crate::server`]
//! wraps with a `DocumentStore` lookup. The renderer takes no I/O, no
//! filesystem, no server state — just the outline and the file URI.
//! That keeps the unit tests below exhaustive; the integration tests in
//! `tarn-lsp/tests/code_lens_test.rs` only need to confirm the wiring.
//!
//! # Contract with the client
//!
//! Each lens carries a [`Command`] whose `command` string is one of two
//! stable constants:
//!
//!   * [`RUN_TEST_COMMAND`] = `"tarn.runTest"`
//!   * [`RUN_STEP_COMMAND`] = `"tarn.runStep"`
//!
//! The `arguments` vector carries a single JSON object with the fields
//! the client needs to spawn `tarn run --select <selector>` itself. The
//! `selector` field is produced by [`tarn::selector::format_test_selector`]
//! / [`tarn::selector::format_step_selector`] — the same helpers the VS
//! Code extension uses via `runArgs.ts`. That way both producers emit
//! byte-identical selector strings and a single change to the grammar
//! updates every consumer at once.
//!
//! The server **does not** register an `executeCommand` handler for
//! these IDs. The LSP `workspace/executeCommand` surface is intentionally
//! left to the client — shelling out to `tarn run` is a local concern
//! (environment, cwd, streaming, progress UI all differ per client) and
//! a Phase L3 follow-up can revisit it if we decide the server should
//! stream NDJSON progress back as notifications.
//!
//! # Scope of which nodes get a lens
//!
//! Only **named tests** and their nested **steps** receive lenses:
//!
//!   * `setup:` / `teardown:` / top-level `steps:` (flat) do **not** get
//!     lenses. The VS Code extension intentionally scoped its provider
//!     to test-group children; we match that exactly so a user switching
//!     between the extension and plain LSP sees the same affordances.
//!   * Files without a `tests:` mapping emit zero lenses even if they
//!     have setup / teardown / flat steps. That's consistent with "there
//!     is nothing here the runner can be pointed at via `--select`".

use std::path::Path;

use lsp_types::{CodeLens, Command, Position, Range, Url};
use serde_json::json;
use tarn::outline::{outline_document, Outline, OutlineSpan, StepOutline, TestOutline};
use tarn::selector::{format_step_selector, format_test_selector};

use crate::server::DocumentStore;

/// Well-known command ID for the `Run test` lens. The client is
/// expected to handle this on its own side by shelling out to
/// `tarn run --select <selector>`. The string is part of the server's
/// public contract and must not change without a ticket.
pub const RUN_TEST_COMMAND: &str = "tarn.runTest";

/// Well-known command ID for the `Run step` lens. Same contract as
/// [`RUN_TEST_COMMAND`] — clients dispatch it themselves.
pub const RUN_STEP_COMMAND: &str = "tarn.runStep";

/// Entry point for the request dispatcher.
///
/// Reads the current buffer for `uri` out of the store, extracts an
/// [`Outline`], and renders it as a flat `Vec<CodeLens>`. Always returns
/// a vector — an unknown URI, a non-`*.tarn.yaml` URI, or an
/// un-parseable buffer all yield `Vec::new()` which LSP clients render
/// as "no lenses" rather than an error.
///
/// The function name is in `snake_case` to match the other handler
/// wrappers in this crate (`text_document_hover`,
/// `text_document_document_symbol`, etc.).
pub fn text_document_code_lens(store: &DocumentStore, uri: &Url) -> Vec<CodeLens> {
    if !is_tarn_file_uri(uri) {
        // Matches the ticket's "only .tarn.yaml files get lenses" rule.
        // We don't have `language_id` on the DocumentStore today — the
        // URI suffix is the same signal the workspace walker uses, so
        // gating here keeps both consistent.
        return Vec::new();
    }
    let Some(source) = store.get(uri) else {
        return Vec::new();
    };
    let path = uri_to_path(uri);
    let Some(outline) = outline_document(&path, source) else {
        return Vec::new();
    };
    code_lenses_for_outline(uri, &outline)
}

/// Pure renderer: walk an [`Outline`] and emit one lens per named test
/// plus one lens per step inside every named test.
///
/// Keeping this as a free function — no `self`, no `DocumentStore`,
/// no filesystem — is the main reason the behavioural tests below can
/// fabricate synthetic `Outline`s without ever touching the parser.
///
/// The file component of the selector is the URI's filesystem path, so
/// the string the client passes to `tarn run --select` is always
/// unambiguous. `tarn::selector::Selector::matches_file` uses
/// path-suffix matching with `/` alignment, so the absolute path
/// cleanly matches whichever display path the runner uses internally.
pub fn code_lenses_for_outline(file_uri: &Url, outline: &Outline) -> Vec<CodeLens> {
    let file_path = file_component_from_uri(file_uri);
    let mut lenses = Vec::new();
    for test in &outline.tests {
        lenses.push(build_test_lens(file_uri, &file_path, test));
        for (step_index, step) in test.steps.iter().enumerate() {
            lenses.push(build_step_lens(
                file_uri, &file_path, &test.name, step_index, step,
            ));
        }
    }
    lenses
}

/// Build the lens that sits above a named test's `name:` line.
///
/// The range is anchored on the test's `selection_range` — that's the
/// test-group key scalar, which is always a single line so the lens
/// renders on the same line as the user clicks. This matches the
/// convention [`crate::symbols::outline_to_document_symbols`] already
/// uses for `DocumentSymbol.selection_range`, so the outline pane and
/// the lens gutter light up the same glyph.
fn build_test_lens(file_uri: &Url, file_path: &str, test: &TestOutline) -> CodeLens {
    let selector = format_test_selector(file_path, &test.name);
    let arguments = json!({
        "file": file_uri.as_str(),
        "test": test.name,
        "selector": selector,
    });
    CodeLens {
        range: span_to_range(&test.selection_range),
        command: Some(Command {
            title: "Run test".to_owned(),
            command: RUN_TEST_COMMAND.to_owned(),
            arguments: Some(vec![arguments]),
        }),
        data: None,
    }
}

/// Build the lens that sits above a single step's `name:` line.
///
/// The selector uses the zero-based step index (not the step name) to
/// match `tarn::selector::format_step_selector` and the VS Code
/// extension's existing producer. Indices are robust to duplicate step
/// names within a test, which Tarn permits.
fn build_step_lens(
    file_uri: &Url,
    file_path: &str,
    test_name: &str,
    step_index: usize,
    step: &StepOutline,
) -> CodeLens {
    let selector = format_step_selector(file_path, test_name, step_index);
    let arguments = json!({
        "file": file_uri.as_str(),
        "test": test_name,
        "step": step.name,
        "selector": selector,
    });
    CodeLens {
        range: span_to_range(&step.selection_range),
        command: Some(Command {
            title: "Run step".to_owned(),
            command: RUN_STEP_COMMAND.to_owned(),
            arguments: Some(vec![arguments]),
        }),
        data: None,
    }
}

/// Convert a 1-based [`OutlineSpan`] into an LSP [`Range`]. Kept in
/// this module rather than shared with `symbols.rs` because duplicating
/// a four-line helper is cheaper than plumbing a public conversion
/// through `tarn::outline` just for two callers.
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

/// True when `uri` looks like a Tarn test file — the same rule
/// [`crate::workspace`] uses for the filesystem walk. Everything else
/// short-circuits to an empty lens list.
fn is_tarn_file_uri(uri: &Url) -> bool {
    let path = uri.path();
    let basename = path.rsplit('/').next().unwrap_or("");
    basename.ends_with(".tarn.yaml") || basename.ends_with(".tarn.yml")
}

/// Convert an LSP `Url` to a `PathBuf` for the outline extractor.
/// Mirrors [`crate::symbols::uri_to_path`] so both features anchor on
/// the same display path.
fn uri_to_path(uri: &Url) -> std::path::PathBuf {
    uri.to_file_path()
        .unwrap_or_else(|_| Path::new(uri.path()).to_path_buf())
}

/// Extract the file-path string that goes into the selector's `FILE`
/// component. Prefers the filesystem path so the resulting selector is
/// unambiguous regardless of how the client spells the run command.
fn file_component_from_uri(uri: &Url) -> String {
    uri.to_file_path()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| uri.path().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tarn::outline::{Outline, OutlineSpan, StepOutline, TestOutline};

    fn uri() -> Url {
        Url::parse("file:///tmp/fixture.tarn.yaml").unwrap()
    }

    fn span(start_line: usize, start_col: usize, end_line: usize, end_col: usize) -> OutlineSpan {
        OutlineSpan {
            start_line,
            start_column: start_col,
            end_line,
            end_column: end_col,
        }
    }

    fn sample_step(name: &str, line: usize) -> StepOutline {
        StepOutline {
            name: name.to_owned(),
            range: span(line, 3, line + 1, 10),
            selection_range: OutlineSpan::point(line, 9),
        }
    }

    fn sample_test(name: &str, start_line: usize, steps: Vec<StepOutline>) -> TestOutline {
        TestOutline {
            name: name.to_owned(),
            range: span(start_line, 3, start_line + steps.len() * 2 + 1, 10),
            selection_range: OutlineSpan::point(start_line, 3),
            steps,
        }
    }

    fn extract_args(lens: &CodeLens) -> Value {
        let cmd = lens.command.as_ref().expect("lens must carry a command");
        let args = cmd
            .arguments
            .as_ref()
            .expect("lens command must carry arguments");
        assert_eq!(args.len(), 1, "each lens carries exactly one JSON argument");
        args[0].clone()
    }

    // --- empty / degenerate outlines ---

    #[test]
    fn empty_outline_yields_no_lenses() {
        let outline = Outline::default();
        let lenses = code_lenses_for_outline(&uri(), &outline);
        assert!(lenses.is_empty());
    }

    #[test]
    fn outline_with_only_setup_and_teardown_yields_no_lenses() {
        // Setup/teardown/flat_steps are intentionally ignored — only
        // named tests (and their nested steps) get lenses. This mirrors
        // the VS Code extension's TestCodeLensProvider.ts.
        let outline = Outline {
            setup: vec![sample_step("login", 3)],
            teardown: vec![sample_step("cleanup", 10)],
            flat_steps: vec![sample_step("main", 6)],
            ..Outline::default()
        };
        let lenses = code_lenses_for_outline(&uri(), &outline);
        assert!(
            lenses.is_empty(),
            "setup/teardown/flat steps must not get lenses"
        );
    }

    // --- happy path: tests with steps ---

    #[test]
    fn single_test_with_one_step_emits_two_lenses() {
        let outline = Outline {
            tests: vec![sample_test("main", 3, vec![sample_step("list", 5)])],
            ..Outline::default()
        };
        let lenses = code_lenses_for_outline(&uri(), &outline);
        assert_eq!(lenses.len(), 2, "one test + one step = two lenses");

        let test_lens = &lenses[0];
        assert_eq!(
            test_lens.command.as_ref().unwrap().command,
            RUN_TEST_COMMAND
        );
        assert_eq!(test_lens.command.as_ref().unwrap().title, "Run test");

        let step_lens = &lenses[1];
        assert_eq!(
            step_lens.command.as_ref().unwrap().command,
            RUN_STEP_COMMAND
        );
        assert_eq!(step_lens.command.as_ref().unwrap().title, "Run step");
    }

    #[test]
    fn single_test_with_three_steps_emits_four_lenses_in_order() {
        let outline = Outline {
            tests: vec![sample_test(
                "main",
                3,
                vec![
                    sample_step("list", 5),
                    sample_step("create", 7),
                    sample_step("delete", 9),
                ],
            )],
            ..Outline::default()
        };
        let lenses = code_lenses_for_outline(&uri(), &outline);
        assert_eq!(lenses.len(), 4);
        // Order: test lens, then step lenses in source order.
        let titles: Vec<_> = lenses
            .iter()
            .map(|l| l.command.as_ref().unwrap().title.clone())
            .collect();
        assert_eq!(titles, vec!["Run test", "Run step", "Run step", "Run step"]);
    }

    #[test]
    fn multiple_tests_each_get_their_own_lens_group() {
        let outline = Outline {
            tests: vec![
                sample_test("first", 3, vec![sample_step("a", 5)]),
                sample_test(
                    "second",
                    10,
                    vec![sample_step("b", 12), sample_step("c", 14)],
                ),
            ],
            ..Outline::default()
        };
        let lenses = code_lenses_for_outline(&uri(), &outline);
        // 1 test lens + 1 step + 1 test lens + 2 steps = 5.
        assert_eq!(lenses.len(), 5);

        let first_test_args = extract_args(&lenses[0]);
        assert_eq!(first_test_args["test"], "first");
        assert!(first_test_args["step"].is_null());

        let second_test_args = extract_args(&lenses[2]);
        assert_eq!(second_test_args["test"], "second");
    }

    #[test]
    fn test_with_no_steps_emits_just_the_test_lens() {
        let outline = Outline {
            tests: vec![sample_test("empty_test", 3, vec![])],
            ..Outline::default()
        };
        let lenses = code_lenses_for_outline(&uri(), &outline);
        assert_eq!(lenses.len(), 1);
        assert_eq!(
            lenses[0].command.as_ref().unwrap().command,
            RUN_TEST_COMMAND
        );
    }

    // --- argument shape ---

    #[test]
    fn test_lens_arguments_carry_file_test_and_selector() {
        let outline = Outline {
            tests: vec![sample_test("main", 3, vec![sample_step("list", 5)])],
            ..Outline::default()
        };
        let lenses = code_lenses_for_outline(&uri(), &outline);
        let args = extract_args(&lenses[0]);
        assert_eq!(args["file"], "file:///tmp/fixture.tarn.yaml");
        assert_eq!(args["test"], "main");
        assert_eq!(args["selector"], "/tmp/fixture.tarn.yaml::main");
        // Test lens never carries a `step` field.
        assert!(args.get("step").map(Value::is_null).unwrap_or(true));
    }

    #[test]
    fn step_lens_arguments_carry_file_test_step_name_and_indexed_selector() {
        let outline = Outline {
            tests: vec![sample_test(
                "main",
                3,
                vec![sample_step("list", 5), sample_step("create", 7)],
            )],
            ..Outline::default()
        };
        let lenses = code_lenses_for_outline(&uri(), &outline);
        // lenses[0] = test, lenses[1] = step 0, lenses[2] = step 1
        let step0 = extract_args(&lenses[1]);
        assert_eq!(step0["step"], "list");
        assert_eq!(step0["selector"], "/tmp/fixture.tarn.yaml::main::0");

        let step1 = extract_args(&lenses[2]);
        assert_eq!(step1["step"], "create");
        assert_eq!(step1["selector"], "/tmp/fixture.tarn.yaml::main::1");
    }

    // --- range correctness ---

    #[test]
    fn lens_ranges_come_from_selection_range_and_are_zero_based() {
        let outline = Outline {
            tests: vec![sample_test("main", 4, vec![sample_step("list", 7)])],
            ..Outline::default()
        };
        let lenses = code_lenses_for_outline(&uri(), &outline);
        // Test's selection_range was `OutlineSpan::point(4, 3)` — 1-based.
        assert_eq!(lenses[0].range.start, Position::new(3, 2));
        assert_eq!(lenses[0].range.end, Position::new(3, 2));
        // Step's selection_range was `OutlineSpan::point(7, 9)` — 1-based.
        assert_eq!(lenses[1].range.start, Position::new(6, 8));
        assert_eq!(lenses[1].range.end, Position::new(6, 8));
    }

    // --- selector edge cases ---

    #[test]
    fn selector_preserves_spaces_and_punctuation_in_test_names() {
        let outline = Outline {
            tests: vec![sample_test(
                "GET /users/{id}",
                3,
                vec![sample_step("happy path", 5)],
            )],
            ..Outline::default()
        };
        let lenses = code_lenses_for_outline(&uri(), &outline);
        let test_args = extract_args(&lenses[0]);
        assert_eq!(test_args["test"], "GET /users/{id}");
        assert_eq!(
            test_args["selector"],
            "/tmp/fixture.tarn.yaml::GET /users/{id}"
        );
        let step_args = extract_args(&lenses[1]);
        assert_eq!(step_args["step"], "happy path");
        assert_eq!(
            step_args["selector"],
            "/tmp/fixture.tarn.yaml::GET /users/{id}::0"
        );
    }

    #[test]
    fn file_component_uses_filesystem_path_not_url_string() {
        // The file path component must not carry the `file://` scheme —
        // `tarn run` parses it back through `Selector::parse` which
        // would treat the scheme as part of the file string. The
        // `file` JSON field does carry the URL so clients can open it,
        // but the `selector` field is the `FILE::TEST` string.
        let outline = Outline {
            tests: vec![sample_test("main", 3, vec![])],
            ..Outline::default()
        };
        let lenses = code_lenses_for_outline(&uri(), &outline);
        let args = extract_args(&lenses[0]);
        let selector = args["selector"].as_str().unwrap();
        assert!(
            !selector.starts_with("file://"),
            "selector must not include url scheme, got {selector}"
        );
        assert!(selector.ends_with("/tmp/fixture.tarn.yaml::main"));
    }

    // --- URI filter ---

    #[test]
    fn is_tarn_file_uri_accepts_double_extension_variants() {
        let a = Url::parse("file:///tmp/x.tarn.yaml").unwrap();
        let b = Url::parse("file:///tmp/x.tarn.yml").unwrap();
        let c = Url::parse("file:///tmp/x.yaml").unwrap();
        let d = Url::parse("file:///tmp/x.txt").unwrap();
        assert!(is_tarn_file_uri(&a));
        assert!(is_tarn_file_uri(&b));
        assert!(!is_tarn_file_uri(&c));
        assert!(!is_tarn_file_uri(&d));
    }
}
