//! `tarn inspect` — drill-down into a run's `report.json` (NAZ-405).
//!
//! Surfaces the same information a human would get by opening the
//! archive JSON by hand, but keyed by a stable `FILE::TEST::STEP`
//! target address and filtered to the level the user cares about.
//! Keeping inspection in one command means a failing-step view from
//! the acceptance criteria ("open a failed step without opening the
//! whole JSON file") stays a single invocation — not a `jq` recipe.
//!
//! # Loading
//!
//! The module reads the per-run archive at
//! `<workspace_root>/.tarn/runs/<run_id>/report.json`, falling back to
//! the latest-run pointer at `<workspace_root>/.tarn/last-run.json`
//! when the user passes `last` / `latest` / `@latest` / `prev`. Run
//! id aliases are resolved through
//! [`crate::report::run_dir::resolve_run_id`].
//!
//! # Target syntax
//!
//! - `None` → run-level view (totals, exit code, failed files)
//! - `FILE` → file-level view (tests + setup/teardown outcome)
//! - `FILE::TEST` → test-level view (steps + captures)
//! - `FILE::TEST::STEP` → step-level view (request, response,
//!   assertions, failure category)
//!
//! The separator is the same `::` grammar used by `--select` and
//! `tarn rerun`, so the address strings round-trip with existing
//! tooling.
//!
//! # JSON shape
//!
//! `--format json` emits one of four envelopes keyed by `target`:
//!
//! ```json
//! { "schema_version": 1, "target": "run",
//!   "run_id": "…",
//!   "source": "…/report.json",
//!   "exit_code": 0, "duration_ms": 0,
//!   "totals": { "files": N, "tests": N, "steps": N },
//!   "failed": { "files": N, "tests": N, "steps": N },
//!   "failed_files": [ { "file": "…", "failed_tests": N, "failed_steps": N } ] }
//!
//! { "schema_version": 1, "target": "file",
//!   "run_id": "…", "source": "…",
//!   "file": { "file": "…", "status": "PASSED|FAILED", "duration_ms": N,
//!             "setup": [ {step} … ], "teardown": [ {step} … ],
//!             "tests": [ { "name": "…", "status": "…", "duration_ms": N,
//!                          "steps": [ {short-step} … ] } ] } }
//!
//! { "schema_version": 1, "target": "test",
//!   "run_id": "…", "source": "…",
//!   "file": "…", "test": { "name": "…", "status": "…",
//!                           "duration_ms": N,
//!                           "steps": [ {short-step} … ],
//!                           "captures": { … } } }
//!
//! { "schema_version": 1, "target": "step",
//!   "run_id": "…", "source": "…",
//!   "file": "…", "test": "…",
//!   "step": { "name": "…", "status": "…", "duration_ms": N,
//!             "failure_category": "…"|null,
//!             "request": {…}|null, "response": {…}|null,
//!             "assertions": [ {assertion} … ] } }
//! ```
//!
//! Redaction has already been applied by `report::json` at write time,
//! so no further scrubbing happens here.

use crate::assert::types::FailureCategory;
use crate::report::run_dir::{resolve_run_id, run_directory};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const INSPECT_SCHEMA_VERSION: u32 = 1;

/// Where the report.json that seeds inspection lives.
#[derive(Debug, Clone)]
pub struct InspectSource {
    pub run_id: Option<String>,
    pub path: PathBuf,
}

impl InspectSource {
    /// Path string for JSON output. Forward-slash separators on every
    /// platform so artifacts stay byte-identical between Unix and Windows.
    pub fn display_path(&self) -> String {
        crate::path_util::to_forward_slash(&self.path)
    }
}

/// Errors the command layer maps to exit code 2.
#[derive(Debug)]
pub enum InspectError {
    NotFound(PathBuf),
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Parse {
        path: PathBuf,
        error: String,
    },
    UnknownFile(String),
    UnknownTest {
        file: String,
        test: String,
    },
    UnknownStep {
        file: String,
        test: String,
        step: String,
    },
    InvalidTarget(String),
}

impl std::fmt::Display for InspectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InspectError::NotFound(p) => write!(f, "no report at {}", p.display()),
            InspectError::Io { path, error } => {
                write!(f, "failed to read {}: {}", path.display(), error)
            }
            InspectError::Parse { path, error } => {
                write!(f, "failed to parse {}: {}", path.display(), error)
            }
            InspectError::UnknownFile(file) => write!(f, "file not found in report: {}", file),
            InspectError::UnknownTest { file, test } => {
                write!(f, "test '{}' not found in file '{}'", test, file)
            }
            InspectError::UnknownStep { file, test, step } => {
                write!(f, "step '{}' not found in {}::{}", step, file, test)
            }
            InspectError::InvalidTarget(s) => {
                write!(
                    f,
                    "invalid inspect target '{}': expected FILE[::TEST[::STEP]]",
                    s
                )
            }
        }
    }
}

impl std::error::Error for InspectError {}

/// Parsed `FILE[::TEST[::STEP]]` address. `None` means a run-level view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Run,
    File {
        file: String,
    },
    Test {
        file: String,
        test: String,
    },
    Step {
        file: String,
        test: String,
        step: String,
    },
}

impl Target {
    /// Parse a `FILE::TEST::STEP` expression. Empty string or `None`
    /// yields [`Target::Run`]. The grammar deliberately mirrors
    /// `crate::selector::Selector::parse` so inspection addresses are
    /// transferable to `--select`.
    pub fn parse(raw: Option<&str>) -> Result<Self, InspectError> {
        let raw = match raw {
            Some(s) if !s.is_empty() => s,
            _ => return Ok(Target::Run),
        };
        // Split on `::`; empty segments are rejected. Three-or-more
        // segments fold the remainder into the step name so tests with
        // literal `::` in their step labels round-trip (rare but
        // possible).
        let parts: Vec<&str> = raw.split("::").collect();
        if parts.iter().any(|p| p.is_empty()) {
            return Err(InspectError::InvalidTarget(raw.to_string()));
        }
        match parts.as_slice() {
            [file] => Ok(Target::File {
                file: (*file).to_string(),
            }),
            [file, test] => Ok(Target::Test {
                file: (*file).to_string(),
                test: (*test).to_string(),
            }),
            [file, test, step @ ..] => Ok(Target::Step {
                file: (*file).to_string(),
                test: (*test).to_string(),
                step: step.join("::"),
            }),
            _ => Err(InspectError::InvalidTarget(raw.to_string())),
        }
    }
}

/// Resolve a `run_id` alias (see [`resolve_run_id`]) to the on-disk
/// report.json path. When `alias` is `last` / `latest` / `@latest`
/// and no archives exist yet, fall back to the legacy pointer at
/// `.tarn/last-run.json` so users who have run once (before archives
/// were introduced, or with `--no-last-run-json` disabling archives)
/// can still inspect the most recent output.
pub fn resolve_source(workspace_root: &Path, alias: &str) -> Result<InspectSource, InspectError> {
    let latest_like = matches!(
        alias.to_ascii_lowercase().as_str(),
        "last" | "latest" | "@latest"
    );
    match resolve_run_id(workspace_root, alias) {
        Ok(run_id) => {
            let path = run_directory(workspace_root, &run_id).join("report.json");
            if !path.is_file() {
                return Err(InspectError::NotFound(path));
            }
            Ok(InspectSource {
                run_id: Some(run_id),
                path,
            })
        }
        Err(e) if latest_like => {
            // Fall back to the top-level pointer so `tarn inspect last`
            // still works on a workspace where archives aren't present.
            let pointer = workspace_root.join(".tarn").join("last-run.json");
            if pointer.is_file() {
                Ok(InspectSource {
                    run_id: None,
                    path: pointer,
                })
            } else {
                Err(InspectError::Io {
                    path: pointer,
                    error: e,
                })
            }
        }
        Err(e) => Err(InspectError::Io {
            path: workspace_root.join(".tarn").join("runs").join(alias),
            error: e,
        }),
    }
}

/// Load the report JSON and return it alongside its source path.
pub fn load_report(source: &InspectSource) -> Result<Value, InspectError> {
    let raw = std::fs::read(&source.path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            InspectError::NotFound(source.path.clone())
        } else {
            InspectError::Io {
                path: source.path.clone(),
                error,
            }
        }
    })?;
    serde_json::from_slice::<Value>(&raw).map_err(|e| InspectError::Parse {
        path: source.path.clone(),
        error: e.to_string(),
    })
}

/// Build the JSON envelope for the requested target. Filters out files
/// / tests that don't carry a failure in `filter_category` at the
/// run-level view; the flag is a no-op on deeper targets (they are
/// already scoped to one location).
pub fn build_view(
    source: &InspectSource,
    report: &Value,
    target: &Target,
    filter_category: Option<&str>,
) -> Result<Value, InspectError> {
    match target {
        Target::Run => Ok(build_run_view(source, report, filter_category)),
        Target::File { file } => build_file_view(source, report, file),
        Target::Test { file, test } => build_test_view(source, report, file, test),
        Target::Step { file, test, step } => build_step_view(source, report, file, test, step),
    }
}

fn build_run_view(source: &InspectSource, report: &Value, filter_category: Option<&str>) -> Value {
    let files = report
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut totals = Counts::default();
    let mut failed = Counts::default();
    let mut failed_files: Vec<Value> = Vec::new();

    for file in &files {
        let file_failed = file.get("status").and_then(Value::as_str) == Some("FAILED");
        totals.files += 1;
        if file_failed {
            failed.files += 1;
        }
        let mut per_file_failed_tests = 0usize;
        let mut per_file_failed_steps = 0usize;
        let mut per_file_matches_filter = filter_category.is_none();

        for setup in file
            .get("setup")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            totals.steps += 1;
            if setup.get("status").and_then(Value::as_str) == Some("FAILED") {
                failed.steps += 1;
                per_file_failed_steps += 1;
                if category_matches(setup, filter_category) {
                    per_file_matches_filter = true;
                }
            }
        }
        for teardown in file
            .get("teardown")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            totals.steps += 1;
            if teardown.get("status").and_then(Value::as_str) == Some("FAILED") {
                failed.steps += 1;
                per_file_failed_steps += 1;
                if category_matches(teardown, filter_category) {
                    per_file_matches_filter = true;
                }
            }
        }
        for test in file
            .get("tests")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            totals.tests += 1;
            let test_failed = test.get("status").and_then(Value::as_str) == Some("FAILED");
            if test_failed {
                failed.tests += 1;
                per_file_failed_tests += 1;
            }
            for step in test
                .get("steps")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                totals.steps += 1;
                if step.get("status").and_then(Value::as_str) == Some("FAILED") {
                    failed.steps += 1;
                    per_file_failed_steps += 1;
                    if category_matches(step, filter_category) {
                        per_file_matches_filter = true;
                    }
                }
            }
        }

        if file_failed && per_file_matches_filter {
            failed_files.push(json!({
                "file": file.get("file").cloned().unwrap_or(Value::Null),
                "failed_tests": per_file_failed_tests,
                "failed_steps": per_file_failed_steps,
            }));
        }
    }

    json!({
        "schema_version": INSPECT_SCHEMA_VERSION,
        "target": "run",
        "run_id": source.run_id.clone().or_else(|| run_id_from_report(report)),
        "source": source.display_path(),
        "exit_code": report.get("exit_code").cloned().unwrap_or_else(|| {
            // `exit_code` isn't stamped on report.json by the writer
            // today — infer from the run's top-level status so the
            // run-level view still carries a useful "did this run
            // pass?" signal.
            let status = report
                .get("summary")
                .and_then(|s| s.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("FAILED");
            json!(if status == "PASSED" { 0 } else { 1 })
        }),
        "duration_ms": report.get("duration_ms").cloned().unwrap_or(json!(0)),
        "start_time": report.get("start_time").cloned().unwrap_or(Value::Null),
        "end_time": report.get("end_time").cloned().unwrap_or(Value::Null),
        "totals": totals.to_json(),
        "failed": failed.to_json(),
        "failed_files": failed_files,
        "filter_category": filter_category,
    })
}

fn build_file_view(
    source: &InspectSource,
    report: &Value,
    file_name: &str,
) -> Result<Value, InspectError> {
    let file = find_file(report, file_name)?;
    let tests = file
        .get("tests")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|t| {
            json!({
                "name": t.get("name").cloned().unwrap_or(Value::Null),
                "status": t.get("status").cloned().unwrap_or(Value::Null),
                "duration_ms": t.get("duration_ms").cloned().unwrap_or(json!(0)),
                "steps": t
                    .get("steps")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(short_step)
                    .collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "schema_version": INSPECT_SCHEMA_VERSION,
        "target": "file",
        "run_id": source.run_id.clone().or_else(|| run_id_from_report(report)),
        "source": source.display_path(),
        "file": {
            "file": file.get("file").cloned().unwrap_or(Value::Null),
            "name": file.get("name").cloned().unwrap_or(Value::Null),
            "status": file.get("status").cloned().unwrap_or(Value::Null),
            "duration_ms": file.get("duration_ms").cloned().unwrap_or(json!(0)),
            "setup": file
                .get("setup")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(short_step)
                .collect::<Vec<_>>(),
            "teardown": file
                .get("teardown")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(short_step)
                .collect::<Vec<_>>(),
            "tests": tests,
        }
    }))
}

fn build_test_view(
    source: &InspectSource,
    report: &Value,
    file_name: &str,
    test_name: &str,
) -> Result<Value, InspectError> {
    let file = find_file(report, file_name)?;
    let test = find_test(file, file_name, test_name)?;
    let steps = test
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(short_step)
        .collect::<Vec<_>>();
    let captures = test.get("captures").cloned().unwrap_or(json!({}));

    Ok(json!({
        "schema_version": INSPECT_SCHEMA_VERSION,
        "target": "test",
        "run_id": source.run_id.clone().or_else(|| run_id_from_report(report)),
        "source": source.display_path(),
        "file": file_name,
        "test": {
            "name": test.get("name").cloned().unwrap_or(Value::Null),
            "status": test.get("status").cloned().unwrap_or(Value::Null),
            "duration_ms": test.get("duration_ms").cloned().unwrap_or(json!(0)),
            "steps": steps,
            "captures": captures,
        }
    }))
}

fn build_step_view(
    source: &InspectSource,
    report: &Value,
    file_name: &str,
    test_name: &str,
    step_name: &str,
) -> Result<Value, InspectError> {
    let file = find_file(report, file_name)?;
    let test = find_test(file, file_name, test_name)?;
    let step = find_step(test, file_name, test_name, step_name)?;

    let assertions = step
        .get("assertions")
        .and_then(|a| a.get("details"))
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));

    Ok(json!({
        "schema_version": INSPECT_SCHEMA_VERSION,
        "target": "step",
        "run_id": source.run_id.clone().or_else(|| run_id_from_report(report)),
        "source": source.display_path(),
        "file": file_name,
        "test": test_name,
        "step": {
            "name": step.get("name").cloned().unwrap_or(Value::Null),
            "status": step.get("status").cloned().unwrap_or(Value::Null),
            "duration_ms": step.get("duration_ms").cloned().unwrap_or(json!(0)),
            "failure_category": step.get("failure_category").cloned().unwrap_or(Value::Null),
            "error_code": step.get("error_code").cloned().unwrap_or(Value::Null),
            "response_status": step.get("response_status").cloned().unwrap_or(Value::Null),
            "response_summary": step.get("response_summary").cloned().unwrap_or(Value::Null),
            "request": step.get("request").cloned().unwrap_or(Value::Null),
            "response": step.get("response").cloned().unwrap_or(Value::Null),
            "assertions": assertions,
        }
    }))
}

fn find_file<'a>(report: &'a Value, file_name: &str) -> Result<&'a Value, InspectError> {
    report
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| {
            files
                .iter()
                .find(|f| f.get("file").and_then(Value::as_str) == Some(file_name))
        })
        .ok_or_else(|| InspectError::UnknownFile(file_name.to_string()))
}

fn find_test<'a>(
    file: &'a Value,
    file_name: &str,
    test_name: &str,
) -> Result<&'a Value, InspectError> {
    file.get("tests")
        .and_then(Value::as_array)
        .and_then(|tests| {
            tests
                .iter()
                .find(|t| t.get("name").and_then(Value::as_str) == Some(test_name))
        })
        .ok_or_else(|| InspectError::UnknownTest {
            file: file_name.to_string(),
            test: test_name.to_string(),
        })
}

fn find_step<'a>(
    test: &'a Value,
    file_name: &str,
    test_name: &str,
    step_name: &str,
) -> Result<&'a Value, InspectError> {
    test.get("steps")
        .and_then(Value::as_array)
        .and_then(|steps| {
            steps
                .iter()
                .find(|s| s.get("name").and_then(Value::as_str) == Some(step_name))
        })
        .ok_or_else(|| InspectError::UnknownStep {
            file: file_name.to_string(),
            test: test_name.to_string(),
            step: step_name.to_string(),
        })
}

fn short_step(step: Value) -> Value {
    json!({
        "name": step.get("name").cloned().unwrap_or(Value::Null),
        "status": step.get("status").cloned().unwrap_or(Value::Null),
        "duration_ms": step.get("duration_ms").cloned().unwrap_or(json!(0)),
        "failure_category": step.get("failure_category").cloned().unwrap_or(Value::Null),
        "response_status": step.get("response_status").cloned().unwrap_or(Value::Null),
        "response_summary": step.get("response_summary").cloned().unwrap_or(Value::Null),
    })
}

fn category_matches(step: &Value, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(want) => step
            .get("failure_category")
            .and_then(Value::as_str)
            .map(|s| s.eq_ignore_ascii_case(want))
            .unwrap_or(false),
    }
}

fn run_id_from_report(report: &Value) -> Option<String> {
    report
        .get("run_id")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

/// Validate a `--filter-category` value against the known enum so users
/// get a clear error rather than a silently-empty result.
pub fn validate_category(raw: &str) -> Result<(), String> {
    let try_parse: Result<FailureCategory, _> =
        serde_json::from_value(Value::String(raw.to_string()));
    try_parse.map(|_| ()).map_err(|_| {
        format!(
            "unknown failure category '{}'. Valid values: assertion_failed, connection_error, \
             timeout, parse_error, capture_error, unresolved_template, \
             skipped_due_to_failed_capture, skipped_due_to_fail_fast, skipped_by_condition",
            raw
        )
    })
}

/// Render the inspect view as a human-readable block. Output is
/// structured text, not a dashboard — the audience is humans scanning
/// the terminal, so every line stands alone and grep-friendly.
pub fn render_human(view: &Value) -> String {
    let mut out = String::new();
    let target = view.get("target").and_then(Value::as_str).unwrap_or("run");
    match target {
        "run" => render_run_human(view, &mut out),
        "file" => render_file_human(view, &mut out),
        "test" => render_test_human(view, &mut out),
        "step" => render_step_human(view, &mut out),
        _ => out.push_str("tarn inspect: unknown target\n"),
    }
    out
}

fn render_run_human(view: &Value, out: &mut String) {
    out.push_str(&format!(
        "run: {}\n",
        view.get("run_id").and_then(Value::as_str).unwrap_or("?"),
    ));
    out.push_str(&format!(
        "source: {}\n",
        view.get("source").and_then(Value::as_str).unwrap_or("?"),
    ));
    if let Some(exit) = view.get("exit_code").and_then(Value::as_i64) {
        out.push_str(&format!("exit_code: {}\n", exit));
    }
    if let Some(dur) = view.get("duration_ms").and_then(Value::as_u64) {
        out.push_str(&format!("duration_ms: {}\n", dur));
    }
    let totals = view.get("totals").cloned().unwrap_or(Value::Null);
    let failed = view.get("failed").cloned().unwrap_or(Value::Null);
    out.push_str(&format!(
        "totals: files={} tests={} steps={}\n",
        counts_field(&totals, "files"),
        counts_field(&totals, "tests"),
        counts_field(&totals, "steps"),
    ));
    out.push_str(&format!(
        "failed: files={} tests={} steps={}\n",
        counts_field(&failed, "files"),
        counts_field(&failed, "tests"),
        counts_field(&failed, "steps"),
    ));

    let empty = Vec::new();
    let failed_files = view
        .get("failed_files")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if failed_files.is_empty() {
        out.push_str("failed_files: none\n");
    } else {
        out.push_str("failed_files:\n");
        for ff in failed_files {
            out.push_str(&format!(
                "  - {} (tests={}, steps={})\n",
                ff.get("file").and_then(Value::as_str).unwrap_or("?"),
                ff.get("failed_tests").and_then(Value::as_u64).unwrap_or(0),
                ff.get("failed_steps").and_then(Value::as_u64).unwrap_or(0),
            ));
        }
    }
}

fn render_file_human(view: &Value, out: &mut String) {
    let file = view.get("file").cloned().unwrap_or(Value::Null);
    out.push_str(&format!(
        "file: {}\n",
        file.get("file").and_then(Value::as_str).unwrap_or("?"),
    ));
    out.push_str(&format!(
        "status: {}  duration_ms={}\n",
        file.get("status").and_then(Value::as_str).unwrap_or("?"),
        file.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
    ));
    let empty = Vec::new();
    let setup = file
        .get("setup")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if !setup.is_empty() {
        out.push_str("setup:\n");
        for s in setup {
            out.push_str(&format_short_step_line(s, "  "));
        }
    }
    let tests = file
        .get("tests")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if tests.is_empty() {
        out.push_str("tests: none\n");
    } else {
        out.push_str("tests:\n");
        for t in tests {
            out.push_str(&format!(
                "  - {} [{}] duration_ms={}\n",
                t.get("name").and_then(Value::as_str).unwrap_or("?"),
                t.get("status").and_then(Value::as_str).unwrap_or("?"),
                t.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
            ));
            let steps = t.get("steps").and_then(Value::as_array).unwrap_or(&empty);
            for s in steps {
                out.push_str(&format_short_step_line(s, "      "));
            }
        }
    }
    let teardown = file
        .get("teardown")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if !teardown.is_empty() {
        out.push_str("teardown:\n");
        for s in teardown {
            out.push_str(&format_short_step_line(s, "  "));
        }
    }
}

fn render_test_human(view: &Value, out: &mut String) {
    let file = view.get("file").and_then(Value::as_str).unwrap_or("?");
    let test = view.get("test").cloned().unwrap_or(Value::Null);
    out.push_str(&format!(
        "test: {}::{}\n",
        file,
        test.get("name").and_then(Value::as_str).unwrap_or("?"),
    ));
    out.push_str(&format!(
        "status: {}  duration_ms={}\n",
        test.get("status").and_then(Value::as_str).unwrap_or("?"),
        test.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
    ));
    let empty = Vec::new();
    let steps = test
        .get("steps")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    out.push_str("steps:\n");
    for s in steps {
        out.push_str(&format_short_step_line(s, "  "));
    }
    if let Some(captures) = test.get("captures").and_then(Value::as_object) {
        if captures.is_empty() {
            out.push_str("captures: none\n");
        } else {
            out.push_str("captures:\n");
            for (k, v) in captures {
                out.push_str(&format!("  {} = {}\n", k, v));
            }
        }
    }
}

fn render_step_human(view: &Value, out: &mut String) {
    let file = view.get("file").and_then(Value::as_str).unwrap_or("?");
    let test = view.get("test").and_then(Value::as_str).unwrap_or("?");
    let step = view.get("step").cloned().unwrap_or(Value::Null);
    out.push_str(&format!(
        "step: {}::{}::{}\n",
        file,
        test,
        step.get("name").and_then(Value::as_str).unwrap_or("?"),
    ));
    out.push_str(&format!(
        "status: {}  duration_ms={}\n",
        step.get("status").and_then(Value::as_str).unwrap_or("?"),
        step.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
    ));
    if let Some(cat) = step.get("failure_category").and_then(Value::as_str) {
        out.push_str(&format!("failure_category: {}\n", cat));
    }
    if let Some(code) = step.get("error_code").and_then(Value::as_str) {
        out.push_str(&format!("error_code: {}\n", code));
    }
    if let Some(req) = step.get("request").and_then(Value::as_object) {
        out.push_str("request:\n");
        if let (Some(method), Some(url)) = (
            req.get("method").and_then(Value::as_str),
            req.get("url").and_then(Value::as_str),
        ) {
            out.push_str(&format!("  {} {}\n", method, url));
        }
        if let Some(headers) = req.get("headers").and_then(Value::as_object) {
            for (k, v) in headers {
                out.push_str(&format!("  > {}: {}\n", k, v.as_str().unwrap_or("")));
            }
        }
        if let Some(body) = req.get("body") {
            if !body.is_null() {
                out.push_str(&format!(
                    "  body: {}\n",
                    serde_json::to_string(body).unwrap_or_default()
                ));
            }
        }
    }
    if let Some(resp) = step.get("response").and_then(Value::as_object) {
        out.push_str("response:\n");
        if let Some(status) = resp.get("status").and_then(Value::as_u64) {
            out.push_str(&format!("  status: {}\n", status));
        }
        if let Some(headers) = resp.get("headers").and_then(Value::as_object) {
            for (k, v) in headers {
                out.push_str(&format!("  < {}: {}\n", k, v.as_str().unwrap_or("")));
            }
        }
        if let Some(body) = resp.get("body") {
            if !body.is_null() {
                out.push_str(&format!(
                    "  body: {}\n",
                    serde_json::to_string(body).unwrap_or_default()
                ));
            }
        }
    }
    if let Some(assertions) = step.get("assertions").and_then(Value::as_array) {
        out.push_str("assertions:\n");
        for a in assertions {
            let passed = a.get("passed").and_then(Value::as_bool).unwrap_or(false);
            let marker = if passed { "PASS" } else { "FAIL" };
            out.push_str(&format!(
                "  [{}] {} expected={} actual={}\n",
                marker,
                a.get("assertion").and_then(Value::as_str).unwrap_or("?"),
                a.get("expected").and_then(Value::as_str).unwrap_or(""),
                a.get("actual").and_then(Value::as_str).unwrap_or(""),
            ));
            if !passed {
                if let Some(msg) = a.get("message").and_then(Value::as_str) {
                    if !msg.is_empty() {
                        out.push_str(&format!("        {}\n", msg));
                    }
                }
            }
        }
    }
}

fn format_short_step_line(step: &Value, indent: &str) -> String {
    let name = step.get("name").and_then(Value::as_str).unwrap_or("?");
    let status = step.get("status").and_then(Value::as_str).unwrap_or("?");
    let duration = step.get("duration_ms").and_then(Value::as_u64).unwrap_or(0);
    let mut line = format!("{}- {} [{}] duration_ms={}", indent, name, status, duration);
    if let Some(cat) = step.get("failure_category").and_then(Value::as_str) {
        line.push_str(&format!(" category={}", cat));
    }
    if let Some(status) = step.get("response_status").and_then(Value::as_u64) {
        line.push_str(&format!(" http={}", status));
    }
    line.push('\n');
    line
}

#[derive(Default, Debug)]
struct Counts {
    files: usize,
    tests: usize,
    steps: usize,
}

impl Counts {
    fn to_json(&self) -> Value {
        json!({
            "files": self.files,
            "tests": self.tests,
            "steps": self.steps,
        })
    }
}

fn counts_field(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> Value {
        // Two files: one fully passing, one failing. The failing file
        // carries a failing step with assertions + a request/response
        // block so step-level rendering has real data to exercise.
        json!({
            "schema_version": 1,
            "duration_ms": 123,
            "run_id": "20260401-120000-aabbcc",
            "start_time": "2026-04-01T12:00:00Z",
            "end_time": "2026-04-01T12:00:00Z",
            "exit_code": 1,
            "summary": { "status": "FAILED" },
            "files": [
                {
                    "file": "tests/ok.tarn.yaml",
                    "name": "ok",
                    "status": "PASSED",
                    "duration_ms": 5,
                    "setup": [],
                    "teardown": [],
                    "tests": [
                        {
                            "name": "t1",
                            "status": "PASSED",
                            "duration_ms": 5,
                            "steps": [
                                {
                                    "name": "ping",
                                    "status": "PASSED",
                                    "duration_ms": 5,
                                    "response_status": 200,
                                    "assertions": {"details": []}
                                }
                            ],
                            "captures": {}
                        }
                    ]
                },
                {
                    "file": "tests/bad.tarn.yaml",
                    "name": "bad",
                    "status": "FAILED",
                    "duration_ms": 7,
                    "setup": [],
                    "teardown": [],
                    "tests": [
                        {
                            "name": "sad",
                            "status": "FAILED",
                            "duration_ms": 7,
                            "steps": [
                                {
                                    "name": "boom",
                                    "status": "FAILED",
                                    "duration_ms": 7,
                                    "failure_category": "assertion_failed",
                                    "response_status": 500,
                                    "request": {
                                        "method": "GET",
                                        "url": "https://api.test/x",
                                        "headers": {"accept": "application/json"}
                                    },
                                    "response": {
                                        "status": 500,
                                        "headers": {"content-type": "application/json"},
                                        "body": {"error": "boom"}
                                    },
                                    "assertions": {
                                        "details": [
                                            {
                                                "assertion": "status",
                                                "passed": false,
                                                "expected": "200",
                                                "actual": "500",
                                                "message": "status mismatch"
                                            }
                                        ]
                                    }
                                }
                            ],
                            "captures": {"token": "abc"}
                        },
                        {
                            "name": "sad2",
                            "status": "FAILED",
                            "duration_ms": 2,
                            "steps": [
                                {
                                    "name": "net",
                                    "status": "FAILED",
                                    "duration_ms": 2,
                                    "failure_category": "connection_error",
                                    "assertions": {"details": []}
                                }
                            ],
                            "captures": {}
                        }
                    ]
                }
            ]
        })
    }

    fn sample_source() -> InspectSource {
        InspectSource {
            run_id: Some("20260401-120000-aabbcc".into()),
            path: PathBuf::from("/tmp/report.json"),
        }
    }

    #[test]
    fn target_parse_none_yields_run_target() {
        assert_eq!(Target::parse(None).unwrap(), Target::Run);
        assert_eq!(Target::parse(Some("")).unwrap(), Target::Run);
    }

    #[test]
    fn target_parse_file_test_step_levels() {
        assert_eq!(
            Target::parse(Some("a.yaml")).unwrap(),
            Target::File {
                file: "a.yaml".into()
            }
        );
        assert_eq!(
            Target::parse(Some("a.yaml::t")).unwrap(),
            Target::Test {
                file: "a.yaml".into(),
                test: "t".into()
            }
        );
        assert_eq!(
            Target::parse(Some("a.yaml::t::s")).unwrap(),
            Target::Step {
                file: "a.yaml".into(),
                test: "t".into(),
                step: "s".into()
            }
        );
    }

    #[test]
    fn target_parse_rejects_empty_segment() {
        assert!(matches!(
            Target::parse(Some("a.yaml::")),
            Err(InspectError::InvalidTarget(_))
        ));
        assert!(matches!(
            Target::parse(Some("::a")),
            Err(InspectError::InvalidTarget(_))
        ));
    }

    #[test]
    fn build_view_run_counts_reflect_report() {
        let report = sample_report();
        let view = build_view(&sample_source(), &report, &Target::Run, None).unwrap();
        assert_eq!(view["target"], "run");
        assert_eq!(view["totals"]["files"], 2);
        assert_eq!(view["totals"]["tests"], 3);
        assert_eq!(view["failed"]["files"], 1);
        assert_eq!(view["failed"]["tests"], 2);
        assert_eq!(view["failed"]["steps"], 2);
        let failed_files = view["failed_files"].as_array().unwrap();
        assert_eq!(failed_files.len(), 1);
        assert_eq!(failed_files[0]["file"], "tests/bad.tarn.yaml");
        assert_eq!(failed_files[0]["failed_tests"], 2);
    }

    #[test]
    fn build_view_run_filter_category_narrows_failed_files() {
        let report = sample_report();
        // A filter that matches only connection_error: the bad file
        // should still appear because one of its tests has the cascade.
        let view = build_view(
            &sample_source(),
            &report,
            &Target::Run,
            Some("connection_error"),
        )
        .unwrap();
        let failed_files = view["failed_files"].as_array().unwrap();
        assert_eq!(failed_files.len(), 1);
        assert_eq!(failed_files[0]["file"], "tests/bad.tarn.yaml");

        // Filter that matches nothing in the fail set → empty list.
        let view = build_view(&sample_source(), &report, &Target::Run, Some("timeout")).unwrap();
        assert!(view["failed_files"].as_array().unwrap().is_empty());
    }

    #[test]
    fn build_view_file_returns_setup_teardown_and_tests() {
        let report = sample_report();
        let view = build_view(
            &sample_source(),
            &report,
            &Target::File {
                file: "tests/bad.tarn.yaml".into(),
            },
            None,
        )
        .unwrap();
        assert_eq!(view["target"], "file");
        assert_eq!(view["file"]["status"], "FAILED");
        assert_eq!(view["file"]["tests"].as_array().unwrap().len(), 2);
        assert_eq!(
            view["file"]["tests"][0]["steps"][0]["failure_category"],
            "assertion_failed"
        );
    }

    #[test]
    fn build_view_test_includes_captures() {
        let report = sample_report();
        let view = build_view(
            &sample_source(),
            &report,
            &Target::Test {
                file: "tests/bad.tarn.yaml".into(),
                test: "sad".into(),
            },
            None,
        )
        .unwrap();
        assert_eq!(view["target"], "test");
        assert_eq!(view["test"]["captures"]["token"], "abc");
        assert_eq!(view["test"]["steps"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn build_view_step_embeds_request_response_and_assertions() {
        let report = sample_report();
        let view = build_view(
            &sample_source(),
            &report,
            &Target::Step {
                file: "tests/bad.tarn.yaml".into(),
                test: "sad".into(),
                step: "boom".into(),
            },
            None,
        )
        .unwrap();
        assert_eq!(view["target"], "step");
        assert_eq!(view["step"]["failure_category"], "assertion_failed");
        assert_eq!(view["step"]["request"]["method"], "GET");
        assert_eq!(view["step"]["response"]["status"], 500);
        let details = view["step"]["assertions"].as_array().unwrap();
        assert_eq!(details.len(), 1);
        assert_eq!(details[0]["passed"], false);
    }

    #[test]
    fn build_view_unknown_file_errors() {
        let report = sample_report();
        let err = build_view(
            &sample_source(),
            &report,
            &Target::File {
                file: "missing.yaml".into(),
            },
            None,
        )
        .unwrap_err();
        assert!(matches!(err, InspectError::UnknownFile(_)));
    }

    #[test]
    fn build_view_unknown_test_errors() {
        let report = sample_report();
        let err = build_view(
            &sample_source(),
            &report,
            &Target::Test {
                file: "tests/bad.tarn.yaml".into(),
                test: "nope".into(),
            },
            None,
        )
        .unwrap_err();
        assert!(matches!(err, InspectError::UnknownTest { .. }));
    }

    #[test]
    fn build_view_unknown_step_errors() {
        let report = sample_report();
        let err = build_view(
            &sample_source(),
            &report,
            &Target::Step {
                file: "tests/bad.tarn.yaml".into(),
                test: "sad".into(),
                step: "nope".into(),
            },
            None,
        )
        .unwrap_err();
        assert!(matches!(err, InspectError::UnknownStep { .. }));
    }

    #[test]
    fn validate_category_accepts_known_snake_case_values() {
        assert!(validate_category("assertion_failed").is_ok());
        assert!(validate_category("timeout").is_ok());
        assert!(validate_category("not_a_category").is_err());
    }

    #[test]
    fn render_human_run_includes_counts_and_failed_files() {
        let report = sample_report();
        let view = build_view(&sample_source(), &report, &Target::Run, None).unwrap();
        let rendered = render_human(&view);
        assert!(rendered.contains("tests/bad.tarn.yaml"));
        assert!(rendered.contains("failed: files=1"));
    }

    #[test]
    fn render_human_step_includes_request_url_and_assertion() {
        let report = sample_report();
        let view = build_view(
            &sample_source(),
            &report,
            &Target::Step {
                file: "tests/bad.tarn.yaml".into(),
                test: "sad".into(),
                step: "boom".into(),
            },
            None,
        )
        .unwrap();
        let rendered = render_human(&view);
        assert!(rendered.contains("GET https://api.test/x"));
        assert!(rendered.contains("[FAIL] status"));
        assert!(rendered.contains("status mismatch"));
    }

    #[test]
    fn resolve_source_reads_from_archive_when_present() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir =
            crate::report::run_dir::ensure_run_directory(tmp.path(), "20260101-120000-abcdef")
                .unwrap();
        let report_path = dir.join("report.json");
        std::fs::write(
            &report_path,
            serde_json::to_string(&sample_report()).unwrap(),
        )
        .unwrap();
        let source = resolve_source(tmp.path(), "20260101-120000-abcdef").unwrap();
        assert_eq!(source.path, report_path);
        assert_eq!(source.run_id.as_deref(), Some("20260101-120000-abcdef"));
    }

    #[test]
    fn resolve_source_falls_back_to_last_run_pointer_for_latest_alias() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No `.tarn/runs/` at all — only the legacy pointer.
        let pointer = tmp.path().join(".tarn").join("last-run.json");
        std::fs::create_dir_all(pointer.parent().unwrap()).unwrap();
        std::fs::write(&pointer, serde_json::to_string(&sample_report()).unwrap()).unwrap();
        let source = resolve_source(tmp.path(), "last").unwrap();
        assert_eq!(source.path, pointer);
        assert!(source.run_id.is_none());
    }

    #[test]
    fn resolve_source_unknown_id_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = resolve_source(tmp.path(), "does-not-exist").unwrap_err();
        assert!(matches!(err, InspectError::Io { .. }));
    }
}
