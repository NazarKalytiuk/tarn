//! `tarn pack-context` — minimum remediation bundle (NAZ-414).
//!
//! After a run fails, the agent needs the same small bundle of context
//! to decide the next edit: failed YAML snippets, request/response
//! excerpts, captures involved, and rerun guidance. Today that bundle
//! must be assembled manually; this module assembles it.
//!
//! The output is a sibling of NAZ-412's `AgentReport`: it reuses the
//! same artifact layer (summary + failures + optional report) but adds
//! YAML snippets, source file paths, and capture lineage so a coding
//! agent can make the next edit without reopening the full report in
//! common cases. The shape is stable (`schema_version: 1`) and
//! machine-readable; the markdown renderer is a compact human-friendly
//! view of the same data.
//!
//! # JSON schema (v1)
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "run_id": "20260421-…",
//!   "generated_at": "…",
//!   "filters": { "failed": true, "files": ["…"], "tests": ["…"] },
//!   "run": { "started_at": "…", "ended_at": "…", "duration_ms": N,
//!            "exit_code": N, "totals": {…}, "failed": {…},
//!            "args": ["…"], "env_name": "local",
//!            "working_directory": "/…" },
//!   "entries": [
//!     {
//!       "file": "tests/users.tarn.yaml",
//!       "file_path_absolute": "/abs/path",
//!       "test": "creates user",
//!       "step": "post-user",
//!       "step_index": 0,
//!       "location": { "line": 12, "column": 3 },
//!       "yaml_snippet": "…",
//!       "yaml_snippet_line_start": 8,
//!       "yaml_snippet_warning": "source changed since run",
//!       "failure": {
//!         "category": "…", "message": "…",
//!         "request":  { "method": "POST", "url": "…" },
//!         "response": { "status": 500, "body_excerpt": "…" },
//!         "response_shape_mismatch": { … }
//!       },
//!       "captures": {
//!         "produced":    [ { "name": "user_id", "path": "$.id" } ],
//!         "consumed_by": [ { "step": "get-user", "variable": "user_id" } ],
//!         "blocked":     [ { "name": "user_id", "missing_path": null,
//!                            "reason": "…" } ]
//!       },
//!       "related_steps": [
//!         { "step": "get-user", "status": "skipped_due_to_failed_capture" }
//!       ],
//!       "rerun": { "command": "…", "scope": "TEST", "selector": "…" }
//!     }
//!   ],
//!   "artifacts": { "run_dir": "…", "report": "…", "summary": "…",
//!                  "failures": "…", "state": "…", "events": "…" },
//!   "notes": [ "pack truncated to fit max_chars; full context at …" ]
//! }
//! ```

use crate::assert::types::FailureCategory;
use crate::report::shape_diagnosis::ShapeMismatchDiagnosis;
use crate::report::state_writer::StateDoc;
use crate::report::summary::{FailureEntry, FailuresDoc, SummaryDoc};
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Bumped on any incompatible change to the envelope.
pub const PACK_CONTEXT_SCHEMA_VERSION: u32 = 1;

/// Default total-char cap for the serialized output. Chosen to stay
/// well under a typical 8K agent context window while still leaving
/// room for surrounding prompt scaffolding.
pub const DEFAULT_MAX_CHARS: usize = 16_000;

/// Max YAML snippet length in lines before we truncate with a marker.
const YAML_SNIPPET_MAX_LINES: usize = 40;

/// How many lines of leading context to include above the step block.
const YAML_SNIPPET_LEAD_LINES: usize = 2;

/// Per-entry cap on `response.body_excerpt` after the second truncation
/// pass. Only applied when the output is still above `max_chars`.
const BODY_EXCERPT_TRUNCATED_CAP: usize = 500;

// --- Top-level envelope --------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PackContext {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub generated_at: String,
    pub filters: Filters,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<RunInfo>,
    pub entries: Vec<Entry>,
    pub artifacts: Artifacts,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Filters {
    pub failed: bool,
    pub files: Vec<String>,
    pub tests: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunInfo {
    pub started_at: String,
    pub ended_at: String,
    pub duration_ms: u64,
    pub exit_code: i32,
    pub totals: Counts,
    pub failed: Counts,
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
}

/// Counts mirror the summary artifact so consumers can reuse their
/// existing schema.
#[derive(Debug, Clone, Serialize)]
pub struct Counts {
    pub files: usize,
    pub tests: usize,
    pub steps: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path_absolute: Option<String>,
    pub test: String,
    pub step: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yaml_snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yaml_snippet_line_start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yaml_snippet_warning: Option<String>,
    pub failure: FailureBlock,
    pub captures: CapturesBlock,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related_steps: Vec<RelatedStep>,
    pub rerun: Rerun,
}

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FailureBlock {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<FailureCategory>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestExcerpt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<ResponseExcerpt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_shape_mismatch: Option<ShapeMismatchDiagnosis>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestExcerpt {
    pub method: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseExcerpt {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_excerpt: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CapturesBlock {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub produced: Vec<ProducedCapture>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub consumed_by: Vec<ConsumedCapture>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<BlockedCapture>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProducedCapture {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsumedCapture {
    pub step: String,
    pub variable: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlockedCapture {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing_path: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelatedStep {
    pub step: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Rerun {
    pub command: String,
    pub scope: String,
    pub selector: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Artifacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failures: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<String>,
}

// --- Inputs --------------------------------------------------------------

/// Everything the builder needs on top of the parsed artifacts.
///
/// Kept as a struct so future fields (e.g. an MCP session id) do not
/// force a cascading signature change.
#[derive(Debug, Clone)]
pub struct PackContextInputs<'a> {
    pub summary: &'a SummaryDoc,
    pub failures: &'a FailuresDoc,
    /// Full run report, when available. Used to surface `step_index`,
    /// `captures_set`, and same-test cascade fallout. Degrades gracefully
    /// to `None` for older archives or missing artifacts.
    pub report: Option<&'a Value>,
    /// Condensed state document — supplies `args`, `env.name`, and the
    /// working directory that produced the run. Optional.
    pub state: Option<&'a StateDoc>,
    /// Absolute path to the per-run artifact directory, when one exists.
    pub run_dir: Option<&'a Path>,
    /// Filter narrowing: empty slice means "all".
    pub file_filters: &'a [String],
    pub test_filters: &'a [String],
    /// Pack only failing entries. Default when no other filter is set.
    pub failed_only: bool,
    /// Anchor for resolving relative source file paths on disk. Defaults
    /// to the process working directory at call time; tests override.
    pub workspace_root: &'a Path,
}

// --- Builder -------------------------------------------------------------

/// Assemble the pack-context envelope without truncation.
///
/// The output format shaper ([`render_json`] / [`render_markdown`])
/// applies the [`max_chars`] budget via [`apply_truncation`]. Keeping
/// build pure from truncation lets tests assert the full shape before
/// exercising the budget logic.
pub fn build(inputs: &PackContextInputs<'_>) -> PackContext {
    let filters = Filters {
        failed: inputs.failed_only,
        files: inputs.file_filters.to_vec(),
        tests: inputs.test_filters.to_vec(),
    };

    let run_info = build_run_info(inputs.summary, inputs.state);
    let artifacts = build_artifacts(inputs.run_dir);

    let entries = build_entries(inputs);

    PackContext {
        schema_version: PACK_CONTEXT_SCHEMA_VERSION,
        run_id: inputs
            .summary
            .run_id
            .clone()
            .or_else(|| inputs.failures.run_id.clone()),
        generated_at: Utc::now().to_rfc3339(),
        filters,
        run: Some(run_info),
        entries,
        artifacts,
        notes: Vec::new(),
    }
}

fn build_run_info(summary: &SummaryDoc, state: Option<&StateDoc>) -> RunInfo {
    let env_name = state.and_then(|s| s.env.name.clone());
    // `args` / `working_directory` live on state.json; summary.json does
    // not carry them. Missing state → empty args and a null `working_dir`.
    let args = state.map(|s| s.last_run.args.clone()).unwrap_or_default();
    let working_directory = state.and_then(|s| {
        s.debug_session
            .as_ref()
            .and_then(|v| v.get("working_directory"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });

    RunInfo {
        started_at: summary.started_at.clone(),
        ended_at: summary.ended_at.clone(),
        duration_ms: summary.duration_ms,
        exit_code: summary.exit_code,
        totals: Counts {
            files: summary.totals.files,
            tests: summary.totals.tests,
            steps: summary.totals.steps,
        },
        failed: Counts {
            files: summary.failed.files,
            tests: summary.failed.tests,
            steps: summary.failed.steps,
        },
        args,
        env_name,
        working_directory,
    }
}

fn build_artifacts(run_dir: Option<&Path>) -> Artifacts {
    match run_dir {
        None => Artifacts::default(),
        Some(dir) => Artifacts {
            run_dir: Some(dir.display().to_string()),
            report: Some(dir.join("report.json").display().to_string()),
            summary: Some(dir.join("summary.json").display().to_string()),
            failures: Some(dir.join("failures.json").display().to_string()),
            state: Some(dir.join("state.json").display().to_string()),
            events: Some(dir.join("events.jsonl").display().to_string()),
        },
    }
}

fn build_entries(inputs: &PackContextInputs<'_>) -> Vec<Entry> {
    // Filter composition is AND: a failure must match every active
    // filter family. An empty family is a wildcard.
    inputs
        .failures
        .failures
        .iter()
        .filter(|f| filter_matches(f, inputs.file_filters, inputs.test_filters))
        .map(|f| build_entry(f, inputs))
        .collect()
}

fn filter_matches(entry: &FailureEntry, files: &[String], tests: &[String]) -> bool {
    let file_ok = files.is_empty() || files.iter().any(|f| matches_file(&entry.file, f));
    let test_ok = tests.is_empty() || tests.contains(&entry.test);
    file_ok && test_ok
}

/// Match a failure entry's `file` against a user-supplied filter.
/// Accepts either exact-string equality or a trailing-path match, so
/// users can pass either the relative path they saw in the report or
/// the absolute path their editor shows.
fn matches_file(entry_file: &str, filter: &str) -> bool {
    if entry_file == filter {
        return true;
    }
    let entry_path = Path::new(entry_file);
    let filter_path = Path::new(filter);
    if entry_path == filter_path {
        return true;
    }
    // Trailing-component match so `--file users.tarn.yaml` finds
    // `tests/users.tarn.yaml`.
    entry_file.ends_with(filter) || filter.ends_with(entry_file)
}

fn build_entry(failure: &FailureEntry, inputs: &PackContextInputs<'_>) -> Entry {
    let (step_index, captures_produced, consumed_by, related_steps) =
        enrich_from_report(failure, inputs.report);

    let blocked = extract_blocked_captures(failure);

    let source = extract_source_details(failure, inputs.workspace_root);

    let failure_block = FailureBlock {
        category: failure.failure_category,
        message: failure.message.clone(),
        request: failure.request.as_ref().map(|r| RequestExcerpt {
            method: r.method.clone(),
            url: r.url.clone(),
        }),
        response: failure.response.as_ref().map(|r| ResponseExcerpt {
            status: r.status,
            body_excerpt: r.body_excerpt.clone(),
        }),
        response_shape_mismatch: failure.response_shape_mismatch.clone(),
    };

    let rerun = build_rerun_hint(failure, inputs);

    Entry {
        file: failure.file.clone(),
        file_path_absolute: source.absolute_path,
        test: failure.test.clone(),
        step: failure.step.clone(),
        step_index,
        location: source.location,
        yaml_snippet: source.snippet,
        yaml_snippet_line_start: source.snippet_line_start,
        yaml_snippet_warning: source.warning,
        failure: failure_block,
        captures: CapturesBlock {
            produced: captures_produced,
            consumed_by,
            blocked,
        },
        related_steps,
        rerun,
    }
}

/// Walk the stored `report.json` for the failing entry to pull the
/// step's index, produced captures, and same-test cascade fallout.
///
/// We scan later steps in the same test for `{{ capture.X }}`
/// references against the produced captures to build `consumed_by`.
/// Keeps the dependency on report.json soft: every derived field
/// degrades to an empty default when the report is missing or the
/// matching coordinates are not found.
fn enrich_from_report(
    failure: &FailureEntry,
    report: Option<&Value>,
) -> (
    Option<usize>,
    Vec<ProducedCapture>,
    Vec<ConsumedCapture>,
    Vec<RelatedStep>,
) {
    let Some(report) = report else {
        return (None, Vec::new(), Vec::new(), Vec::new());
    };

    let Some(test_steps) = find_test_steps(report, &failure.file, &failure.test) else {
        return (None, Vec::new(), Vec::new(), Vec::new());
    };

    let step_index = test_steps
        .iter()
        .position(|s| s.get("name").and_then(|v| v.as_str()) == Some(failure.step.as_str()));

    let produced = step_index
        .map(|i| extract_produced_captures(test_steps[i]))
        .unwrap_or_default();

    let consumed_by = match step_index {
        Some(i) => extract_consumed_by(&test_steps[i + 1..], &produced),
        None => Vec::new(),
    };

    let related_steps = match step_index {
        Some(i) => extract_related_steps(&test_steps[i + 1..]),
        None => Vec::new(),
    };

    (step_index, produced, consumed_by, related_steps)
}

/// Find the array of step objects for `(file, test)` inside the full
/// report.json document. Searches the `tests` block of the matching
/// file, falling back to `setup` / `teardown` using the known slug
/// values so cascade fallout inside setup/teardown still resolves.
fn find_test_steps<'a>(
    report: &'a Value,
    file_name: &str,
    test_name: &str,
) -> Option<Vec<&'a Value>> {
    let files = report.get("files")?.as_array()?;
    let file = files
        .iter()
        .find(|f| f.get("file").and_then(|v| v.as_str()) == Some(file_name))?;

    if test_name == crate::fixtures::SETUP_TEST_SLUG {
        return file
            .get("setup")
            .and_then(|s| s.as_array())
            .map(|arr| arr.iter().collect());
    }
    if test_name == crate::fixtures::TEARDOWN_TEST_SLUG {
        return file
            .get("teardown")
            .and_then(|s| s.as_array())
            .map(|arr| arr.iter().collect());
    }

    let tests = file.get("tests").and_then(|t| t.as_array())?;
    let test = tests
        .iter()
        .find(|t| t.get("name").and_then(|v| v.as_str()) == Some(test_name))?;
    test.get("steps")
        .and_then(|s| s.as_array())
        .map(|arr| arr.iter().collect())
}

fn extract_produced_captures(step: &Value) -> Vec<ProducedCapture> {
    let Some(set) = step.get("captures_set").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    set.iter()
        .filter_map(|v| v.as_str())
        .map(|name| ProducedCapture {
            name: name.to_string(),
            // `path` and `value_type` live on the step's `capture:` map
            // in the source YAML, not in the report. We leave them null
            // here — the source YAML snippet carries the path verbatim
            // and an agent that needs the type can inspect the response
            // body excerpt.
            path: None,
            value_type: None,
        })
        .collect()
}

fn extract_consumed_by(
    later_steps: &[&Value],
    produced: &[ProducedCapture],
) -> Vec<ConsumedCapture> {
    if produced.is_empty() {
        return Vec::new();
    }
    let names: Vec<&str> = produced.iter().map(|c| c.name.as_str()).collect();
    let mut out: Vec<ConsumedCapture> = Vec::new();

    for step in later_steps {
        let Some(step_name) = step.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        // Cheap recursive string scan of the step subtree; any string
        // containing `{{ capture.NAME }}` counts as a consumer. A regex
        // over the serialized step is simpler than walking every
        // potential interpolation site, and Tarn's per-step payloads
        // stay small enough that the overhead is negligible next to
        // the HTTP work that generated them.
        let haystack = serde_json::to_string(step).unwrap_or_default();
        for name in &names {
            if contains_capture_reference(&haystack, name)
                && !out
                    .iter()
                    .any(|c| c.step == step_name && c.variable == *name)
            {
                out.push(ConsumedCapture {
                    step: step_name.to_string(),
                    variable: (*name).to_string(),
                });
            }
        }
    }

    out
}

/// Scan a serialized step for `{{ capture.NAME }}` mentions.
///
/// Mirrors `interpolation::resolve_expression`: the interpolator
/// matches `capture.NAME` with optional internal whitespace around
/// `{{`/`}}`. A simple substring scan with whitespace-tolerant bounds
/// is enough — we deliberately ignore transforms
/// (`{{ capture.NAME | foo }}`) since any transform still contains the
/// same `capture.NAME` token.
fn contains_capture_reference(haystack: &str, name: &str) -> bool {
    let needle = format!("capture.{}", name);
    let mut search_from = 0;
    while let Some(idx) = haystack[search_from..].find(&needle) {
        let abs = search_from + idx;
        let after = abs + needle.len();
        // Must be bounded on the trailing side by a non-identifier char
        // so `capture.foo` does not spuriously match `capture.foobar`.
        let boundary_ok = haystack
            .as_bytes()
            .get(after)
            .is_none_or(|b| !(b.is_ascii_alphanumeric() || *b == b'_'));
        // Must be preceded by `{{` somewhere on the line, allowing
        // interior whitespace.
        let prefix_ok = haystack[..abs]
            .rfind("{{")
            .map(|open| {
                let gap = &haystack[open + 2..abs];
                gap.chars().all(|c| c.is_whitespace())
            })
            .unwrap_or(false);
        if boundary_ok && prefix_ok {
            return true;
        }
        search_from = abs + 1;
    }
    false
}

fn extract_related_steps(later_steps: &[&Value]) -> Vec<RelatedStep> {
    let mut out: Vec<RelatedStep> = Vec::new();
    for step in later_steps {
        let Some(step_name) = step.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let passed = step
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.eq_ignore_ascii_case("PASSED"))
            .unwrap_or(true);
        if passed {
            continue;
        }
        // Cascade fallout is detected via `error_category`; primary
        // follow-on failures surface with their category too so agents
        // can tell "skipped because upstream broke" from "also failed
        // on its own merits".
        let category = step
            .get("error_category")
            .and_then(|v| v.as_str())
            .unwrap_or("failed")
            .to_string();
        out.push(RelatedStep {
            step: step_name.to_string(),
            status: category,
        });
    }
    out
}

/// Pull capture names out of the cascade assertion's `actual`
/// (`missing: a, b`), or out of a shape diagnosis when the failing
/// step lost a capture. Empty result keeps the block omitted.
fn extract_blocked_captures(entry: &FailureEntry) -> Vec<BlockedCapture> {
    // Shape drift: diagnosis carries `expected_path` as the missing
    // JSONPath, but not the capture name. We surface the expected path
    // under `missing_path` with a generic reason; the message already
    // explains the contract drift.
    if let Some(hint) = entry.response_shape_mismatch.as_ref() {
        return vec![BlockedCapture {
            name: capture_name_from_message(&entry.message)
                .unwrap_or_else(|| hint.expected_path.clone()),
            missing_path: Some(hint.expected_path.clone()),
            reason: "response shape drifted; JSONPath did not match".to_string(),
        }];
    }

    // Cascade skip: the synthetic `cascade` assertion's `actual` is
    // `missing: a, b, c`. We only have `message` on the failure entry,
    // so we parse "capture(s)… : a, b" out of the human message the
    // runner emits.
    if matches!(
        entry.failure_category,
        Some(FailureCategory::SkippedDueToFailedCapture)
    ) {
        let names = capture_names_from_cascade_message(&entry.message);
        if !names.is_empty() {
            return names
                .into_iter()
                .map(|name| BlockedCapture {
                    name,
                    missing_path: None,
                    reason: "upstream step did not produce the capture".to_string(),
                })
                .collect();
        }
    }

    Vec::new()
}

fn capture_name_from_message(msg: &str) -> Option<String> {
    // Matches messages like "Capture 'user_id' failed: ..."
    let rest = msg.strip_prefix("Capture '")?;
    let end = rest.find('\'')?;
    let name = &rest[..end];
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn capture_names_from_cascade_message(msg: &str) -> Vec<String> {
    // The runner's cascade message is:
    //   "Skipped: step references capture(s) that failed earlier in
    //    this test: a, b. Fix …"
    // Be tolerant: split on the colon that follows "test", then take
    // up to the first period. Fall back to an empty list.
    let anchor = match msg.find("test: ") {
        Some(i) => i + "test: ".len(),
        None => return Vec::new(),
    };
    let tail = &msg[anchor..];
    let stop = tail.find('.').unwrap_or(tail.len());
    tail[..stop]
        .split(',')
        .map(|s| s.trim().trim_end_matches('.').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Bundle of source-derived fields attached to an entry.
///
/// Any field may be `None` individually — we degrade gracefully when
/// the source file has moved, been edited, or cannot be parsed.
#[derive(Debug, Default, Clone)]
struct SourceDetails {
    location: Option<Location>,
    snippet: Option<String>,
    snippet_line_start: Option<usize>,
    warning: Option<String>,
    absolute_path: Option<String>,
}

/// Resolve line/column and a YAML snippet for a failure entry.
fn extract_source_details(failure: &FailureEntry, workspace_root: &Path) -> SourceDetails {
    let candidate = resolve_source_path(&failure.file, workspace_root);
    let Some(path) = candidate else {
        return SourceDetails::default();
    };
    let abs = std::fs::canonicalize(&path)
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| path.display().to_string());

    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => {
            return SourceDetails {
                warning: Some("source file unreadable since run".to_string()),
                absolute_path: Some(abs),
                ..SourceDetails::default()
            };
        }
    };

    // Re-parse so we can attach the step's `location` even though
    // `failures.json` itself does not carry it. parse_file returns an
    // error for malformed YAML — we still want the rest of the entry,
    // so on failure we warn and skip the snippet.
    let test_file = match crate::parser::parse_file(&path) {
        Ok(tf) => tf,
        Err(_) => {
            return SourceDetails {
                warning: Some("source file failed to parse".to_string()),
                absolute_path: Some(abs),
                ..SourceDetails::default()
            };
        }
    };

    let location =
        find_step_location(&test_file, &failure.test, &failure.step).map(|loc| Location {
            line: loc.line,
            column: loc.column,
        });

    let Some(loc) = location.as_ref() else {
        return SourceDetails {
            warning: Some("source changed since run".to_string()),
            absolute_path: Some(abs),
            ..SourceDetails::default()
        };
    };

    let (snippet, snippet_start) = extract_yaml_snippet(&raw, loc.line);

    SourceDetails {
        location: Some(loc.clone()),
        snippet: Some(snippet),
        snippet_line_start: Some(snippet_start),
        warning: None,
        absolute_path: Some(abs),
    }
}

/// Try to locate the failing step's source path on disk.
///
/// The `FailureEntry.file` may be a relative path captured at run time.
/// We first try it verbatim; if that misses, we join it to the
/// workspace root. Absolute paths are used directly.
fn resolve_source_path(file: &str, workspace_root: &Path) -> Option<PathBuf> {
    let direct = PathBuf::from(file);
    if direct.is_absolute() && direct.is_file() {
        return Some(direct);
    }
    if direct.is_file() {
        return Some(direct);
    }
    let joined = workspace_root.join(file);
    if joined.is_file() {
        return Some(joined);
    }
    None
}

fn find_step_location(
    file: &crate::model::TestFile,
    test_name: &str,
    step_name: &str,
) -> Option<crate::model::Location> {
    if test_name == crate::fixtures::SETUP_TEST_SLUG {
        return file
            .setup
            .iter()
            .find(|s| s.name == step_name)
            .and_then(|s| s.location.clone());
    }
    if test_name == crate::fixtures::TEARDOWN_TEST_SLUG {
        return file
            .teardown
            .iter()
            .find(|s| s.name == step_name)
            .and_then(|s| s.location.clone());
    }

    // Flat-steps files keep the test label stable as the file's `name`
    // field. Try that path first so the common single-test file format
    // is covered before we scan named tests.
    if file.tests.is_empty() {
        return file
            .steps
            .iter()
            .find(|s| s.name == step_name)
            .and_then(|s| s.location.clone());
    }

    let group = file.tests.get(test_name)?;
    group
        .steps
        .iter()
        .find(|s| s.name == step_name)
        .and_then(|s| s.location.clone())
}

/// Extract an N-line YAML snippet centered on `step_line` (1-based).
///
/// Starts `YAML_SNIPPET_LEAD_LINES` lines above the step name, walks
/// forward until the next top-level step bullet (detected by a `- `
/// at the step's leading indent) or `YAML_SNIPPET_MAX_LINES` — whichever
/// comes first. The goal is "show the whole failing step plus two lines
/// of context" without pulling in the following step.
fn extract_yaml_snippet(source: &str, step_line: usize) -> (String, usize) {
    let lines: Vec<&str> = source.lines().collect();
    if step_line == 0 || step_line > lines.len() {
        return (String::new(), 1);
    }
    let step_idx = step_line - 1;
    let start_idx = step_idx.saturating_sub(YAML_SNIPPET_LEAD_LINES);

    // Detect the step's leading indent from the `- name:` bullet so we
    // can stop the snippet at the next sibling bullet.
    let step_leading = leading_whitespace(lines[step_idx]);

    let mut end_idx = step_idx;
    for (offset, line) in lines[step_idx + 1..].iter().enumerate() {
        // Stop at the next bullet at the same column.
        let indent = leading_whitespace(line);
        let trimmed = line.trim_start();
        if trimmed.starts_with("- ") && indent.len() == step_leading.len() && offset > 0 {
            break;
        }
        end_idx = step_idx + 1 + offset;
        if end_idx - start_idx + 1 >= YAML_SNIPPET_MAX_LINES {
            break;
        }
    }

    let mut snippet: String = lines[start_idx..=end_idx.min(lines.len() - 1)].join("\n");
    // Append a truncation marker if we hit the line cap before the next
    // bullet so an agent knows the file continues.
    if end_idx - start_idx + 1 >= YAML_SNIPPET_MAX_LINES && end_idx < lines.len() - 1 {
        snippet.push_str("\n…");
    }
    (snippet, start_idx + 1)
}

fn leading_whitespace(line: &str) -> &str {
    let end = line
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    &line[..end]
}

fn build_rerun_hint(failure: &FailureEntry, inputs: &PackContextInputs<'_>) -> Rerun {
    let run_id_ref = inputs
        .summary
        .run_id
        .as_deref()
        .or(inputs.failures.run_id.as_deref());
    let command = match run_id_ref {
        Some(id) => format!("tarn rerun --failed --run {}", id),
        None => "tarn rerun --failed".to_string(),
    };
    // Setup/teardown failures force a file-level rerun in tarn — the
    // `rerun` module already implements this; we surface that nuance in
    // `scope` so the agent does not pass a `--test-filter` that would
    // never match.
    let scope = match failure.test.as_str() {
        t if t == crate::fixtures::SETUP_TEST_SLUG || t == crate::fixtures::TEARDOWN_TEST_SLUG => {
            "FILE"
        }
        _ => "TEST",
    };
    let selector = format!("{}::{}", failure.file, failure.test);
    Rerun {
        command,
        scope: scope.to_string(),
        selector,
    }
}

// --- Rendering -----------------------------------------------------------

/// Render the packed context as pretty-printed JSON with trailing
/// newline, applying `max_chars` truncation.
pub fn render_json(pack: &mut PackContext, max_chars: usize) -> String {
    apply_truncation(pack, max_chars, RenderFormat::Json);
    let mut out =
        serde_json::to_string_pretty(pack).expect("PackContext is always JSON-serializable");
    out.push('\n');
    out
}

/// Render the packed context as compact markdown (headings + code
/// fences), applying `max_chars` truncation.
pub fn render_markdown(pack: &mut PackContext, max_chars: usize) -> String {
    apply_truncation(pack, max_chars, RenderFormat::Markdown);
    render_markdown_inner(pack)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderFormat {
    Json,
    Markdown,
}

/// Apply the documented truncation strategy in-place until the
/// serialized form fits within `max_chars`. Records a trailing note
/// when any step ran.
///
/// Order (lowest priority first):
/// 1. Markdown-only: strip YAML snippets past the first 3 entries
/// 2. Drop `captures.consumed_by` past the first 3 per entry
/// 3. Drop `related_steps` past the first 3 per entry
/// 4. Truncate `response.body_excerpt` past the first 500 chars
/// 5. Drop entries past the 10th
/// 6. Add a trailing note if we truncated anything
pub fn apply_truncation(pack: &mut PackContext, max_chars: usize, format: RenderFormat) {
    if size_of(pack, format) <= max_chars {
        return;
    }
    let mut truncated = false;

    // Step 1: markdown-only snippet stripping past entry index 3.
    if format == RenderFormat::Markdown {
        let indices_to_strip: Vec<usize> = pack
            .entries
            .iter()
            .enumerate()
            .filter(|(idx, entry)| *idx >= 3 && entry.yaml_snippet.is_some())
            .map(|(idx, _)| idx)
            .collect();
        for idx in indices_to_strip {
            pack.entries[idx].yaml_snippet = None;
            truncated = true;
            if size_of(pack, format) <= max_chars {
                break;
            }
        }
        if size_of(pack, format) <= max_chars {
            if truncated {
                pack.notes
                    .push(truncation_note(pack.artifacts.report.as_deref()));
            }
            return;
        }
    }

    // Step 2: drop consumed_by past first 3 per entry.
    for entry in pack.entries.iter_mut() {
        if entry.captures.consumed_by.len() > 3 {
            entry.captures.consumed_by.truncate(3);
            truncated = true;
        }
    }
    if size_of(pack, format) <= max_chars {
        if truncated {
            pack.notes
                .push(truncation_note(pack.artifacts.report.as_deref()));
        }
        return;
    }

    // Step 3: drop related_steps past first 3 per entry.
    for entry in pack.entries.iter_mut() {
        if entry.related_steps.len() > 3 {
            entry.related_steps.truncate(3);
            truncated = true;
        }
    }
    if size_of(pack, format) <= max_chars {
        if truncated {
            pack.notes
                .push(truncation_note(pack.artifacts.report.as_deref()));
        }
        return;
    }

    // Step 4: cap body excerpts at BODY_EXCERPT_TRUNCATED_CAP chars.
    for entry in pack.entries.iter_mut() {
        if let Some(resp) = entry.failure.response.as_mut() {
            if let Some(body) = resp.body_excerpt.as_mut() {
                if body.chars().count() > BODY_EXCERPT_TRUNCATED_CAP {
                    *body = truncate_chars(body, BODY_EXCERPT_TRUNCATED_CAP);
                    truncated = true;
                }
            }
        }
    }
    if size_of(pack, format) <= max_chars {
        if truncated {
            pack.notes
                .push(truncation_note(pack.artifacts.report.as_deref()));
        }
        return;
    }

    // Step 5: drop entries past the 10th.
    if pack.entries.len() > 10 {
        pack.entries.truncate(10);
        truncated = true;
    }

    if truncated {
        pack.notes
            .push(truncation_note(pack.artifacts.report.as_deref()));
    }
}

fn truncation_note(report_path: Option<&str>) -> String {
    match report_path {
        Some(p) => format!("pack truncated to fit max_chars; full context at {}", p),
        None => "pack truncated to fit max_chars; see .tarn/runs/<id>/report.json".to_string(),
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    // Cut on a char boundary so we never produce invalid UTF-8.
    let end = s
        .char_indices()
        .take(max)
        .last()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    format!("{}…[truncated]", &s[..end])
}

fn size_of(pack: &PackContext, format: RenderFormat) -> usize {
    match format {
        RenderFormat::Json => serde_json::to_string(pack).map(|s| s.len()).unwrap_or(0),
        RenderFormat::Markdown => render_markdown_inner(pack).len(),
    }
}

fn render_markdown_inner(pack: &PackContext) -> String {
    let mut out = String::new();
    out.push_str("# tarn pack-context\n\n");
    if let Some(id) = pack.run_id.as_deref() {
        out.push_str(&format!("Run id: `{}`\n\n", id));
    }
    if let Some(run) = pack.run.as_ref() {
        out.push_str(&format!(
            "Totals: {} files / {} tests / {} steps — failed {} / {} / {} (exit {})\n\n",
            run.totals.files,
            run.totals.tests,
            run.totals.steps,
            run.failed.files,
            run.failed.tests,
            run.failed.steps,
            run.exit_code,
        ));
    }
    if pack.entries.is_empty() {
        out.push_str("No failing entries match the supplied filters.\n");
        if !pack.notes.is_empty() {
            out.push('\n');
            for note in &pack.notes {
                out.push_str(&format!("> {}\n", note));
            }
        }
        return out;
    }

    for entry in &pack.entries {
        out.push_str(&format!(
            "### {}::{}::{}\n\n",
            entry.file, entry.test, entry.step
        ));
        if let Some(loc) = entry.location.as_ref() {
            out.push_str(&format!(
                "- Location: line {}, column {}\n",
                loc.line, loc.column
            ));
        }
        if let Some(warning) = entry.yaml_snippet_warning.as_deref() {
            out.push_str(&format!("- Source warning: {}\n", warning));
        }
        if let Some(snippet) = entry.yaml_snippet.as_deref() {
            if let Some(start) = entry.yaml_snippet_line_start {
                out.push_str(&format!("- Snippet starts at line {}\n", start));
            }
            out.push_str("\n```yaml\n");
            out.push_str(snippet);
            if !snippet.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n\n");
        }
        out.push_str(&format!(
            "- Failure: {}\n",
            entry
                .failure
                .message
                .lines()
                .next()
                .unwrap_or(&entry.failure.message)
        ));
        if let Some(req) = entry.failure.request.as_ref() {
            out.push_str(&format!("- Request: `{} {}`\n", req.method, req.url));
        }
        if let Some(resp) = entry.failure.response.as_ref() {
            if let Some(s) = resp.status {
                out.push_str(&format!("- Response status: {}\n", s));
            }
            if let Some(body) = resp.body_excerpt.as_deref() {
                out.push_str(&format!("- Response body: `{}`\n", body));
            }
        }
        if !entry.captures.produced.is_empty() {
            out.push_str("- Captures produced: ");
            let names: Vec<&str> = entry
                .captures
                .produced
                .iter()
                .map(|c| c.name.as_str())
                .collect();
            out.push_str(&names.join(", "));
            out.push('\n');
        }
        if !entry.captures.consumed_by.is_empty() {
            out.push_str("- Captures consumed by:\n");
            for cb in &entry.captures.consumed_by {
                out.push_str(&format!("  - {} ← `{}`\n", cb.step, cb.variable));
            }
        }
        if !entry.captures.blocked.is_empty() {
            out.push_str("- Captures blocked:\n");
            for b in &entry.captures.blocked {
                out.push_str(&format!("  - {}: {}\n", b.name, b.reason));
            }
        }
        if !entry.related_steps.is_empty() {
            out.push_str("- Related steps:\n");
            for r in &entry.related_steps {
                out.push_str(&format!("  - {} ({})\n", r.step, r.status));
            }
        }
        out.push_str(&format!(
            "- Rerun: `{}` — scope {}, selector `{}`\n\n",
            entry.rerun.command, entry.rerun.scope, entry.rerun.selector
        ));
    }

    if !pack.notes.is_empty() {
        out.push_str("---\n\n");
        for note in &pack.notes {
            out.push_str(&format!("> {}\n", note));
        }
    }
    out
}

// --- Tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::shape_diagnosis::{CandidateFix, ShapeConfidence, ShapeMismatchDiagnosis};
    use crate::report::summary::{
        Counts as SumCounts, FailureEntry, FailureRequest, FailureResponse, FailuresDoc,
        SummaryDoc, SUMMARY_SCHEMA_VERSION,
    };
    use serde_json::json;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn base_summary() -> SummaryDoc {
        SummaryDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid-test".into()),
            started_at: "2026-04-21T12:00:00+00:00".into(),
            ended_at: "2026-04-21T12:00:05+00:00".into(),
            duration_ms: 5000,
            exit_code: 1,
            totals: SumCounts {
                files: 1,
                tests: 2,
                steps: 3,
            },
            failed: SumCounts {
                files: 1,
                tests: 1,
                steps: 1,
            },
            failed_files: vec!["tests/a.tarn.yaml".into()],
            rerun_source: None,
        }
    }

    fn failing_entry(file: &str, test: &str, step: &str) -> FailureEntry {
        FailureEntry {
            file: file.into(),
            test: test.into(),
            step: step.into(),
            failure_category: Some(FailureCategory::AssertionFailed),
            message: "Expected HTTP status 200, got 500".into(),
            request: Some(FailureRequest {
                method: "POST".into(),
                url: "https://api.test/users".into(),
            }),
            response: Some(FailureResponse {
                status: Some(500),
                body_excerpt: Some(r#"{"error":"boom"}"#.into()),
            }),
            root_cause: None,
            response_shape_mismatch: None,
        }
    }

    fn pack_inputs<'a>(
        summary: &'a SummaryDoc,
        failures: &'a FailuresDoc,
        report: Option<&'a Value>,
        workspace_root: &'a Path,
        file_filters: &'a [String],
        test_filters: &'a [String],
        failed_only: bool,
    ) -> PackContextInputs<'a> {
        PackContextInputs {
            summary,
            failures,
            report,
            state: None,
            run_dir: None,
            file_filters,
            test_filters,
            failed_only,
            workspace_root,
        }
    }

    // --- filter composition ---

    #[test]
    fn filter_composition_intersects_file_and_test() {
        let summary = base_summary();
        let failures = FailuresDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid-test".into()),
            failures: vec![
                failing_entry("tests/a.tarn.yaml", "creates_user", "post"),
                failing_entry("tests/a.tarn.yaml", "deletes_user", "del"),
                failing_entry("tests/b.tarn.yaml", "creates_user", "post"),
            ],
        };
        let tmp = TempDir::new().unwrap();
        let files = vec!["tests/a.tarn.yaml".to_string()];
        let tests = vec!["creates_user".to_string()];
        let inputs = pack_inputs(&summary, &failures, None, tmp.path(), &files, &tests, true);

        let pack = build(&inputs);

        // Only the (tests/a.tarn.yaml, creates_user) pair survives the
        // AND intersection; the other two are excluded on at least one
        // axis.
        assert_eq!(pack.entries.len(), 1);
        assert_eq!(pack.entries[0].file, "tests/a.tarn.yaml");
        assert_eq!(pack.entries[0].test, "creates_user");
    }

    #[test]
    fn empty_filters_include_every_failure() {
        let summary = base_summary();
        let failures = FailuresDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid-test".into()),
            failures: vec![
                failing_entry("tests/a.tarn.yaml", "t1", "s"),
                failing_entry("tests/b.tarn.yaml", "t2", "s"),
            ],
        };
        let tmp = TempDir::new().unwrap();
        let inputs = pack_inputs(&summary, &failures, None, tmp.path(), &[], &[], true);
        let pack = build(&inputs);
        assert_eq!(pack.entries.len(), 2);
    }

    // --- YAML snippet extraction ---

    fn write_file(root: &Path, rel: &str, body: &str) -> PathBuf {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn yaml_snippet_extraction_picks_step_block_with_context_lines() {
        let tmp = TempDir::new().unwrap();
        let yaml = r#"name: Sample
tests:
  t1:
    steps:
      - name: first
        request:
          method: GET
          url: http://x/one
      - name: failing
        request:
          method: POST
          url: http://x/two
        assert:
          status: 200
      - name: third
        request:
          method: GET
          url: http://x/three
"#;
        write_file(tmp.path(), "s.tarn.yaml", yaml);

        let summary = base_summary();
        let failures = FailuresDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid-test".into()),
            failures: vec![failing_entry("s.tarn.yaml", "t1", "failing")],
        };
        let inputs = pack_inputs(&summary, &failures, None, tmp.path(), &[], &[], true);
        let pack = build(&inputs);

        assert_eq!(pack.entries.len(), 1);
        let entry = &pack.entries[0];
        let snippet = entry
            .yaml_snippet
            .as_deref()
            .expect("snippet extracted for a present source file");
        // Target bullet must be in the snippet.
        assert!(
            snippet.contains("- name: failing"),
            "snippet missing failing step: {snippet}"
        );
        // Following bullet must NOT appear — the extractor stops at the
        // next sibling `- name:` bullet.
        assert!(
            !snippet.contains("- name: third"),
            "snippet should not include the following step: {snippet}"
        );
        assert!(entry.yaml_snippet_warning.is_none());
        assert!(entry.location.is_some());
    }

    #[test]
    fn yaml_snippet_warning_when_step_not_found_post_edit() {
        let tmp = TempDir::new().unwrap();
        // The file exists but the named step is no longer there — i.e.
        // the source was edited after the run. The command must degrade
        // gracefully with a warning and no snippet.
        let yaml = r#"name: Sample
steps:
  - name: different
    request:
      method: GET
      url: http://x
"#;
        write_file(tmp.path(), "s.tarn.yaml", yaml);

        let summary = base_summary();
        let failures = FailuresDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid-test".into()),
            failures: vec![failing_entry("s.tarn.yaml", "Sample", "used_to_exist")],
        };
        let inputs = pack_inputs(&summary, &failures, None, tmp.path(), &[], &[], true);
        let pack = build(&inputs);
        let entry = &pack.entries[0];
        assert!(entry.yaml_snippet.is_none());
        assert_eq!(
            entry.yaml_snippet_warning.as_deref(),
            Some("source changed since run")
        );
        // Rest of the entry should still be populated.
        assert_eq!(entry.failure.message, "Expected HTTP status 200, got 500");
    }

    // --- consumed_by scanning ---

    #[test]
    fn consumed_by_detects_capture_references_in_later_steps() {
        // report.json excerpt: the failing step produces `user_id`; a
        // later step references it in a URL template.
        let report = json!({
            "files": [
                {
                    "file": "tests/a.tarn.yaml",
                    "tests": [
                        {
                            "name": "t1",
                            "steps": [
                                {
                                    "name": "create",
                                    "status": "FAILED",
                                    "captures_set": ["user_id"],
                                },
                                {
                                    "name": "read",
                                    "status": "PASSED",
                                    "request": {
                                        "url": "https://api.test/users/{{ capture.user_id }}"
                                    }
                                },
                                {
                                    "name": "unrelated",
                                    "status": "PASSED",
                                    "request": { "url": "https://api.test/other" }
                                }
                            ]
                        }
                    ]
                }
            ]
        });

        let summary = base_summary();
        let failures = FailuresDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid-test".into()),
            failures: vec![failing_entry("tests/a.tarn.yaml", "t1", "create")],
        };
        let tmp = TempDir::new().unwrap();
        let inputs = pack_inputs(
            &summary,
            &failures,
            Some(&report),
            tmp.path(),
            &[],
            &[],
            true,
        );
        let pack = build(&inputs);
        let captures = &pack.entries[0].captures;
        assert_eq!(captures.produced.len(), 1);
        assert_eq!(captures.produced[0].name, "user_id");
        assert_eq!(captures.consumed_by.len(), 1);
        assert_eq!(captures.consumed_by[0].step, "read");
        assert_eq!(captures.consumed_by[0].variable, "user_id");
    }

    #[test]
    fn contains_capture_reference_respects_word_boundaries() {
        // `capture.user_id` must not match inside `capture.user_id_extra`
        // — the word-boundary check is load-bearing for correctness.
        let haystack = r#"{{ capture.user_id_extra }}"#;
        assert!(!contains_capture_reference(haystack, "user_id"));
        let haystack2 = r#"{{ capture.user_id }}"#;
        assert!(contains_capture_reference(haystack2, "user_id"));
    }

    // --- blocked captures ---

    #[test]
    fn blocked_captures_extracted_from_cascade_assertion_message() {
        // A `SkippedDueToFailedCapture` failure's message contains the
        // missing capture names after "test: …".
        let entry = FailureEntry {
            file: "a.tarn.yaml".into(),
            test: "t".into(),
            step: "get".into(),
            failure_category: Some(FailureCategory::SkippedDueToFailedCapture),
            message: "Skipped: step references capture(s) that failed earlier in \
                      this test: user_id, session. Fix the root-cause step first"
                .into(),
            request: None,
            response: None,
            root_cause: None,
            response_shape_mismatch: None,
        };
        let blocked = extract_blocked_captures(&entry);
        assert_eq!(blocked.len(), 2);
        assert_eq!(blocked[0].name, "user_id");
        assert_eq!(blocked[1].name, "session");
        assert!(blocked[0].reason.contains("upstream step"));
    }

    #[test]
    fn blocked_captures_extracted_from_shape_diagnosis() {
        let mut entry = failing_entry("a.tarn.yaml", "t", "s");
        entry.failure_category = Some(FailureCategory::ResponseShapeMismatch);
        entry.message = "Capture 'user_id' failed: JSONPath $.id did not match".into();
        entry.response_shape_mismatch = Some(ShapeMismatchDiagnosis {
            expected_path: "$.id".into(),
            observed_keys: vec!["data".into()],
            observed_type: "object".into(),
            candidate_fixes: vec![CandidateFix {
                path: "$.data.id".into(),
                confidence: ShapeConfidence::High,
                reason: "wrap".into(),
            }],
            high_confidence: true,
        });
        let blocked = extract_blocked_captures(&entry);
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].name, "user_id");
        assert_eq!(blocked[0].missing_path.as_deref(), Some("$.id"));
    }

    // --- truncation strategy ---

    #[test]
    fn truncation_drops_consumed_by_past_three_per_entry_before_entries() {
        // Build one entry that intentionally overflows the budget by
        // packing a long consumed_by list. The documented order says
        // consumed_by gets trimmed before entries do.
        let mut pack = PackContext {
            schema_version: PACK_CONTEXT_SCHEMA_VERSION,
            run_id: Some("rid".into()),
            generated_at: "now".into(),
            filters: Filters::default(),
            run: None,
            entries: vec![Entry {
                file: "a.tarn.yaml".into(),
                file_path_absolute: None,
                test: "t".into(),
                step: "s".into(),
                step_index: Some(0),
                location: None,
                yaml_snippet: None,
                yaml_snippet_line_start: None,
                yaml_snippet_warning: None,
                failure: FailureBlock {
                    category: None,
                    message: "x".into(),
                    request: None,
                    response: None,
                    response_shape_mismatch: None,
                },
                captures: CapturesBlock {
                    produced: vec![],
                    consumed_by: (0..20)
                        .map(|i| ConsumedCapture {
                            step: format!("step_{}", i),
                            variable: format!("var_{}_with_a_long_suffix_to_consume_bytes", i),
                        })
                        .collect(),
                    blocked: vec![],
                },
                related_steps: vec![],
                rerun: Rerun {
                    command: "tarn rerun --failed".into(),
                    scope: "TEST".into(),
                    selector: "a::t".into(),
                },
            }],
            artifacts: Artifacts::default(),
            notes: vec![],
        };

        // Pick a budget tight enough to trigger consumed_by truncation
        // but loose enough to keep the single entry intact.
        apply_truncation(&mut pack, 400, RenderFormat::Json);
        assert_eq!(
            pack.entries[0].captures.consumed_by.len(),
            3,
            "consumed_by must be trimmed to 3 before entries are dropped"
        );
        assert_eq!(pack.entries.len(), 1, "entry must survive the trim");
        assert!(
            pack.notes.iter().any(|n| n.contains("pack truncated")),
            "truncation must record a note, got: {:?}",
            pack.notes
        );
    }

    // --- renderers ---

    #[test]
    fn markdown_render_contains_yaml_fence_and_rerun_section() {
        let tmp = TempDir::new().unwrap();
        let yaml = r#"name: Sample
steps:
  - name: failing
    request:
      method: POST
      url: http://x
    assert:
      status: 200
"#;
        write_file(tmp.path(), "m.tarn.yaml", yaml);

        let summary = base_summary();
        let failures = FailuresDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid-test".into()),
            failures: vec![failing_entry("m.tarn.yaml", "Sample", "failing")],
        };
        let inputs = pack_inputs(&summary, &failures, None, tmp.path(), &[], &[], true);
        let mut pack = build(&inputs);
        let md = render_markdown(&mut pack, DEFAULT_MAX_CHARS);
        assert!(
            md.contains("```yaml"),
            "markdown must include a yaml fence: {md}"
        );
        assert!(
            md.contains("- Rerun:"),
            "markdown must include a rerun bullet: {md}"
        );
        assert!(
            md.contains("m.tarn.yaml::Sample::failing"),
            "markdown heading must name the failing coordinates: {md}"
        );
    }

    #[test]
    fn markdown_render_handles_empty_entries_cleanly() {
        let tmp = TempDir::new().unwrap();
        let summary = base_summary();
        let failures = FailuresDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid-test".into()),
            failures: vec![],
        };
        let inputs = pack_inputs(&summary, &failures, None, tmp.path(), &[], &[], true);
        let mut pack = build(&inputs);
        let md = render_markdown(&mut pack, DEFAULT_MAX_CHARS);
        assert!(md.contains("No failing entries"));
    }
}
