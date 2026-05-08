//! Run-from-gutter command handlers (NAZ-256 Requirement A).
//!
//! Ships the four `workspace/executeCommand` entry points the VS Code
//! code-lens and the "last failures" command rely on:
//!
//!   * `tarn.runFile`         — execute every test in a single `.tarn.yaml`
//!   * `tarn.runTest`         — execute exactly one named test
//!   * `tarn.runStep`         — execute exactly one step in one test
//!   * `tarn.runLastFailures` — reread `.tarn/last-run.json` and rerun every
//!     `(file, test)` pair that failed on the previous run
//!
//! The handlers use Tarn's in-process library surface
//! (`tarn::runner::run_file_with_cookie_jars`) rather than spawning a
//! child process. That buys us two things:
//!
//!   1. `tarn-lsp` gets an authoritative `RunResult` value it can shape
//!      into the stable `{ schema_version: 1, data: ... }` envelope
//!      documented in `docs/TARN_LSP.md` — no JSON reparsing, no
//!      exit-code ambiguity, no stdio race conditions.
//!   2. Tests in `tarn-lsp/tests/run_commands_test.rs` can drive the
//!      handlers directly against a fixture test file, asserting on the
//!      returned envelope and the notifications published along the way,
//!      without needing to compile the `tarn` binary into the test
//!      harness.
//!
//! Progress streaming uses a pluggable [`NotificationSink`] so integration
//! tests can capture every `tarn/progress` notification and assert on the
//! exact sequence. Production wiring passes a sink backed by the real
//! `lsp_server::Connection`.
//!
//! # Error policy
//!
//! Every soft failure collapses to [`lsp_server::ErrorCode::InvalidParams`]
//! so the client sees a precise reason string. Internal server errors
//! (JSON serialise failures, unexpected IO) surface as
//! [`lsp_server::ErrorCode::InternalError`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lsp_server::{ErrorCode, Notification, ResponseError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tarn::assert::types::{FileResult, RunResult, StepResult, TestResult};
use tarn::config;
use tarn::env;
use tarn::model::{HttpTransportConfig, TestFile};
use tarn::parser;
use tarn::report::progress::{ProgressReporter, ReportContext};
use tarn::runner::{self, RunOptions};

/// Stable LSP command IDs advertised in [`crate::capabilities`] and
/// dispatched by [`crate::server::dispatch_request`]. Pinned as
/// constants so the command registry and tests never drift.
pub const RUN_FILE_COMMAND: &str = "tarn.runFile";
pub const RUN_TEST_COMMAND: &str = "tarn.runTest";
pub const RUN_STEP_COMMAND: &str = "tarn.runStep";
pub const RUN_LAST_FAILURES_COMMAND: &str = "tarn.runLastFailures";

/// Notification method used to stream per-step progress while a run is
/// in flight. Client implementations (VS Code, Claude Code) subscribe to
/// this method to update a progress panel; unit tests record notifications
/// through an in-memory sink for assertions.
pub const PROGRESS_NOTIFICATION: &str = "tarn/progress";

/// Arguments to `tarn.runFile`.
#[derive(Debug, Clone, Deserialize)]
pub struct RunFileArgs {
    /// Absolute filesystem path (or `file://` URI) of the `.tarn.yaml`
    /// buffer to execute.
    pub file: String,
    /// Optional environment name. Resolves to `tarn.env.{name}.yaml` if
    /// present, otherwise falls back to the default env chain.
    #[serde(default)]
    pub env: Option<String>,
}

/// Arguments to `tarn.runTest`.
#[derive(Debug, Clone, Deserialize)]
pub struct RunTestArgs {
    /// Absolute filesystem path (or `file://` URI) of the `.tarn.yaml`
    /// buffer containing the test.
    pub file: String,
    /// Name of the named test group (matches the key under `tests:`).
    pub test: String,
    /// Optional environment name (see [`RunFileArgs::env`]).
    #[serde(default)]
    pub env: Option<String>,
}

/// Arguments to `tarn.runStep`. The `step` field accepts either a
/// zero-based numeric index or a step name string so clients that only
/// have one or the other can still invoke the command.
#[derive(Debug, Clone, Deserialize)]
pub struct RunStepArgs {
    /// Absolute filesystem path (or `file://` URI) of the `.tarn.yaml`
    /// buffer.
    pub file: String,
    /// Enclosing test group's name.
    pub test: String,
    /// Step index (as JSON number) or step name (as JSON string).
    pub step: Value,
    /// Optional environment name.
    #[serde(default)]
    pub env: Option<String>,
}

/// Arguments to `tarn.runLastFailures`. Both fields are optional; when
/// unset the handler walks up the working directory looking for
/// `.tarn/last-run.json` the same way the CLI does.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RunLastFailuresArgs {
    /// Explicit path to a `last-run.json` artifact. Overrides discovery.
    #[serde(default)]
    pub path: Option<String>,
    /// Optional environment override. When absent the handler reuses the
    /// `env_name` field recorded in the artifact (see main.rs's
    /// `augment_last_run_json`).
    #[serde(default)]
    pub env: Option<String>,
}

/// Structured envelope returned by every run-from-gutter command. Mirrors
/// the convention established by `tarn.evaluateJsonpath` so clients
/// always see `{ schema_version: 1, data: ... }` regardless of which
/// command fired. Separate `outcome` carries the machine-readable
/// `RunResult` JSON — identical in shape to `tarn run --format json`.
#[derive(Debug, Clone, Serialize)]
pub struct RunOutcomeEnvelope {
    /// Bumped when the outer envelope shape changes. Stays at `1` for
    /// the life of Phase L4.
    pub schema_version: u32,
    /// Per-command payload. Stable across versions: new fields may be
    /// added, existing fields never change type.
    pub data: Value,
}

/// Structured per-step progress notification sent to the client while a
/// run is in flight. One notification fires per executed step; a trailing
/// notification with `stage == "finished"` signals the run has ended.
#[derive(Debug, Clone, Serialize)]
pub struct ProgressPayload {
    /// Absolute path of the test file the notification targets.
    pub file: String,
    /// Name of the enclosing test group (or the file name for flat-step
    /// files) so clients can group progress by test.
    pub test: Option<String>,
    /// Name of the step that just finished. `None` for the trailing
    /// `finished` notification.
    pub step: Option<String>,
    /// Phase identifier: `"setup"`, `"test"`, `"teardown"`, or
    /// `"finished"` (for the end-of-run sentinel).
    pub stage: String,
    /// Whether the step's assertions passed. `None` for stage =
    /// `"finished"`.
    pub passed: Option<bool>,
    /// Step duration in milliseconds. `None` for stage = `"finished"`.
    pub duration_ms: Option<u64>,
}

/// Abstraction over the LSP connection's notification channel so tests
/// can capture notifications without a real `lsp_server::Connection`.
///
/// `Send + Sync` is required because the debug-session worker thread
/// holds a shared sink and publishes notifications from outside the
/// main LSP loop.
pub trait NotificationSink: Send + Sync {
    /// Send one `Notification`. Errors propagate so the dispatcher can
    /// decide whether to bail out or continue.
    fn send(&self, note: Notification) -> Result<(), String>;
}

/// In-memory notification sink used by tests. Every `send` appends to the
/// internal `Vec<Notification>` so assertions can inspect the full stream
/// after the command finishes.
#[derive(Debug, Default, Clone)]
pub struct CapturingSink {
    inner: Arc<Mutex<Vec<Notification>>>,
}

impl CapturingSink {
    /// Construct an empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clone of every notification captured so far, in send order.
    pub fn notifications(&self) -> Vec<Notification> {
        self.inner.lock().expect("capturing sink mutex").clone()
    }

    /// Number of notifications captured. Convenience for tests.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("capturing sink mutex").len()
    }

    /// True when no notifications have been captured.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().expect("capturing sink mutex").is_empty()
    }
}

impl NotificationSink for CapturingSink {
    fn send(&self, note: Notification) -> Result<(), String> {
        self.inner
            .lock()
            .map_err(|e| format!("poisoned sink mutex: {e}"))?
            .push(note);
        Ok(())
    }
}

/// Progress reporter that turns every finished step into a
/// `tarn/progress` notification on the provided sink. Shared between
/// the four run-from-gutter commands so every command publishes the
/// same notification shape.
struct ProgressSinkReporter<'a> {
    sink: &'a dyn NotificationSink,
    file: Mutex<Option<String>>,
    current_test: Mutex<Option<String>>,
}

impl<'a> ProgressSinkReporter<'a> {
    fn new(sink: &'a dyn NotificationSink) -> Self {
        Self {
            sink,
            file: Mutex::new(None),
            current_test: Mutex::new(None),
        }
    }

    fn emit(&self, payload: ProgressPayload) {
        let params = match serde_json::to_value(&payload) {
            Ok(v) => v,
            Err(_) => return,
        };
        let note = Notification {
            method: PROGRESS_NOTIFICATION.to_owned(),
            params,
        };
        let _ = self.sink.send(note);
    }

    fn active_file(&self) -> String {
        self.file
            .lock()
            .expect("progress reporter mutex")
            .clone()
            .unwrap_or_default()
    }

    fn active_test(&self) -> Option<String> {
        self.current_test
            .lock()
            .expect("progress reporter mutex")
            .clone()
    }

    fn emit_step_batch(&self, stage: &str, steps: &[StepResult]) {
        for step in steps {
            self.emit(ProgressPayload {
                file: self.active_file(),
                test: self.active_test(),
                step: Some(step.name.clone()),
                stage: stage.to_string(),
                passed: Some(step.passed),
                duration_ms: Some(step.duration_ms),
            });
        }
    }
}

impl ProgressReporter for ProgressSinkReporter<'_> {
    fn file_started(&self, file: &str, _name: &str) {
        *self.file.lock().expect("progress reporter mutex") = Some(file.to_owned());
    }

    fn setup_finished(&self, results: &[StepResult], _ctx: &ReportContext) {
        self.emit_step_batch("setup", results);
    }

    fn test_finished(&self, test: &TestResult, _ctx: &ReportContext) {
        {
            let mut cur = self.current_test.lock().expect("progress reporter mutex");
            *cur = Some(test.name.clone());
        }
        self.emit_step_batch("test", &test.step_results);
        {
            let mut cur = self.current_test.lock().expect("progress reporter mutex");
            *cur = None;
        }
    }

    fn teardown_finished(&self, results: &[StepResult], _ctx: &ReportContext) {
        self.emit_step_batch("teardown", results);
    }

    fn file_finished(&self, _result: &FileResult) {}

    fn run_finished(&self, _result: &RunResult) {}
}

/// Entry point for `tarn.runFile`.
pub fn execute_run_file(
    args: &RunFileArgs,
    sink: &dyn NotificationSink,
) -> Result<RunOutcomeEnvelope, ResponseError> {
    let file_path = parse_file_arg(&args.file)?;
    let run = run_file_internal(&file_path, None, args.env.as_deref(), sink)?;
    finalize(sink, run)
}

/// Entry point for `tarn.runTest`.
pub fn execute_run_test(
    args: &RunTestArgs,
    sink: &dyn NotificationSink,
) -> Result<RunOutcomeEnvelope, ResponseError> {
    let file_path = parse_file_arg(&args.file)?;
    let selector = runner::build_filter_selector(Some(&args.test), None)
        .map_err(|e| invalid_params(format!("tarn.runTest: {e}")))?;
    let run = run_file_internal(&file_path, Some(vec![selector]), args.env.as_deref(), sink)?;
    finalize(sink, run)
}

/// Entry point for `tarn.runStep`.
pub fn execute_run_step(
    args: &RunStepArgs,
    sink: &dyn NotificationSink,
) -> Result<RunOutcomeEnvelope, ResponseError> {
    let file_path = parse_file_arg(&args.file)?;
    let step_string = step_value_to_string(&args.step)?;
    let selector = runner::build_filter_selector(Some(&args.test), Some(&step_string))
        .map_err(|e| invalid_params(format!("tarn.runStep: {e}")))?;
    let run = run_file_internal(&file_path, Some(vec![selector]), args.env.as_deref(), sink)?;
    finalize(sink, run)
}

/// Entry point for `tarn.runLastFailures`.
pub fn execute_run_last_failures(
    args: &RunLastFailuresArgs,
    sink: &dyn NotificationSink,
) -> Result<RunOutcomeEnvelope, ResponseError> {
    let artifact_path = resolve_last_run_path(args.path.as_deref())
        .ok_or_else(|| invalid_params("tarn.runLastFailures: no .tarn/last-run.json found (pass `path` explicitly or run a test at least once)"))?;
    let last_run = read_last_run(&artifact_path)?;
    let failures = extract_failures(&last_run);
    if failures.is_empty() {
        // Graceful success: no failures means there's nothing to rerun.
        // We still emit a finished notification so clients can clear
        // their progress UI.
        let reporter = ProgressSinkReporter::new(sink);
        reporter.emit(ProgressPayload {
            file: String::new(),
            test: None,
            step: None,
            stage: "finished".to_string(),
            passed: None,
            duration_ms: None,
        });
        return Ok(RunOutcomeEnvelope {
            schema_version: 1,
            data: serde_json::json!({
                "files": [],
                "duration_ms": 0,
                "summary": { "files": 0, "tests": 0, "steps": { "total": 0, "passed": 0, "failed": 0 }, "status": "PASSED" },
                "replay": { "count": 0 },
            }),
        });
    }

    let env_override = args.env.clone().or_else(|| last_run.env_name.clone());

    // Replay each failed (file, test) pair sequentially. We merge the
    // resulting RunResults into one synthetic report so the client
    // always sees a single envelope per command call.
    let mut aggregated_files: Vec<FileResult> = Vec::new();
    let mut total_duration = 0u64;
    let mut replayed_pairs: Vec<Value> = Vec::new();
    for failure in &failures {
        let file_path = PathBuf::from(&failure.file);
        let selector = runner::build_filter_selector(Some(&failure.test), None)
            .map_err(|e| invalid_params(format!("tarn.runLastFailures: {e}")))?;
        let run = run_file_internal(
            &file_path,
            Some(vec![selector]),
            env_override.as_deref(),
            sink,
        )?;
        total_duration = total_duration.saturating_add(run.duration_ms);
        aggregated_files.extend(run.file_results);
        replayed_pairs.push(serde_json::json!({
            "file": failure.file,
            "test": failure.test,
        }));
    }

    let aggregate = RunResult {
        file_results: aggregated_files,
        duration_ms: total_duration,
    };
    let mut envelope = finalize(sink, aggregate)?;
    if let Value::Object(obj) = &mut envelope.data {
        obj.insert(
            "replay".to_string(),
            serde_json::json!({ "count": replayed_pairs.len(), "pairs": replayed_pairs }),
        );
    }
    Ok(envelope)
}

/// Shared in-process execution wrapper used by every command.
fn run_file_internal(
    file_path: &Path,
    selectors: Option<Vec<tarn::selector::Selector>>,
    env_name: Option<&str>,
    sink: &dyn NotificationSink,
) -> Result<RunResult, ResponseError> {
    let start = std::time::Instant::now();
    let mut test_file: TestFile = parser::parse_file(file_path)
        .map_err(|e| invalid_params(format!("failed to parse `{}`: {}", file_path.display(), e)))?;

    let file_dir = file_path.parent().unwrap_or(Path::new("."));
    // NAZ-466: discover the workspace root the same way the CLI does,
    // walking up from the test file to find `tarn.config.yaml` /
    // `tarn.env.yaml`. The previous behavior used the file's immediate
    // parent as `project_root`, so a project config sitting at the repo
    // root was invisible — env resolution worked by accident (the LSP
    // does not require env files), but TLS settings (`insecure`,
    // `cacert`) were silently dropped.
    let project_root =
        config::find_project_root(file_dir).unwrap_or_else(|| file_dir.to_path_buf());
    let project_config = config::load_config(&project_root)
        .map_err(|e| invalid_params(format!("failed to load tarn.config.yaml: {e}")))?;

    let resolved_env = env::resolve_env_with_profiles(
        &test_file.env,
        env_name,
        &[],
        &project_root,
        &project_config.env_file,
        &project_config.environments,
    )
    .map_err(|e| invalid_params(format!("env resolution failed: {e}")))?;

    if test_file.redaction.is_none() {
        test_file.redaction = None;
    }

    let reporter = ProgressSinkReporter::new(sink);
    let opts = RunOptions {
        http: HttpTransportConfig::merge(
            &project_config.http_transport(),
            &HttpTransportConfig::default(),
        ),
        ..RunOptions::default()
    };
    let selectors_vec = selectors.unwrap_or_default();
    let mut cookie_jars = std::collections::HashMap::new();

    let file_result = runner::run_file_with_cookie_jars(
        &test_file,
        &file_path.display().to_string(),
        &resolved_env,
        &[],
        &selectors_vec,
        &opts,
        &mut cookie_jars,
        Some(&reporter),
    )
    .map_err(|e| internal_error(format!("run_file_with_cookie_jars: {e}")))?;

    Ok(RunResult {
        file_results: vec![file_result],
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Build the envelope + trailing `finished` progress notification every
/// command ends with.
fn finalize(
    sink: &dyn NotificationSink,
    run: RunResult,
) -> Result<RunOutcomeEnvelope, ResponseError> {
    let json =
        tarn::report::json::render_with_mode(&run, tarn::report::json::JsonOutputMode::Verbose);
    let data: Value =
        serde_json::from_str(&json).map_err(|e| internal_error(format!("render json: {e}")))?;
    let reporter = ProgressSinkReporter::new(sink);
    // We do not know the "last file" here without threading state — use
    // the first file result's path if available so clients can anchor
    // the finished event. Empty string is fine when no files ran.
    let file = run
        .file_results
        .first()
        .map(|f| f.file.clone())
        .unwrap_or_default();
    reporter.emit(ProgressPayload {
        file,
        test: None,
        step: None,
        stage: "finished".to_string(),
        passed: Some(run.passed()),
        duration_ms: Some(run.duration_ms),
    });
    Ok(RunOutcomeEnvelope {
        schema_version: 1,
        data,
    })
}

/// Convert a step field accepted as either a JSON number or JSON string
/// into the string form [`runner::build_filter_selector`] wants.
fn step_value_to_string(value: &Value) -> Result<String, ResponseError> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(invalid_params(format!(
            "step must be a string or integer, got {other}"
        ))),
    }
}

/// Parse the `file` field. Accepts plain filesystem paths and `file://`
/// URIs so clients that only hand out URIs do not need to convert.
pub fn parse_file_arg(raw: &str) -> Result<PathBuf, ResponseError> {
    if let Ok(url) = lsp_types::Url::parse(raw) {
        if let Ok(p) = url.to_file_path() {
            return Ok(p);
        }
    }
    Ok(PathBuf::from(raw))
}

/// Walk up from the current working directory looking for
/// `.tarn/last-run.json`. Used when the client does not pass an explicit
/// path.
fn resolve_last_run_path(explicit: Option<&str>) -> Option<PathBuf> {
    if let Some(raw) = explicit {
        let p = PathBuf::from(raw);
        return if p.exists() { Some(p) } else { None };
    }
    let cwd = std::env::current_dir().ok()?;
    let mut current: &Path = cwd.as_path();
    loop {
        let candidate = current.join(".tarn").join("last-run.json");
        if candidate.exists() {
            return Some(candidate);
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    None
}

/// In-memory decoding of a `last-run.json` artifact. Matches the shape
/// produced by `tarn::report::json::render_with_mode` plus the extra
/// fields added by `main.rs::augment_last_run_json`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LastRunArtifact {
    #[serde(default)]
    pub files: Vec<LastRunFile>,
    #[serde(default)]
    pub env_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LastRunFile {
    pub file: String,
    #[serde(default)]
    pub tests: Vec<LastRunTest>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LastRunTest {
    pub name: String,
    #[serde(default)]
    pub status: String,
}

/// `(file, test)` pair extracted from a last-run artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureRef {
    pub file: String,
    pub test: String,
}

/// Parse a last-run artifact from disk.
pub fn read_last_run(path: &Path) -> Result<LastRunArtifact, ResponseError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| invalid_params(format!("failed to read `{}`: {}", path.display(), e)))?;
    serde_json::from_str(&content)
        .map_err(|e| invalid_params(format!("failed to parse `{}`: {}", path.display(), e)))
}

/// Walk the artifact, producing one `FailureRef` per failed `(file, test)`
/// pair. Order matches the artifact's natural order so reruns preserve
/// the user's original test ordering.
pub fn extract_failures(artifact: &LastRunArtifact) -> Vec<FailureRef> {
    let mut out = Vec::new();
    for file in &artifact.files {
        for test in &file.tests {
            if test.status.eq_ignore_ascii_case("FAILED") {
                out.push(FailureRef {
                    file: file.file.clone(),
                    test: test.name.clone(),
                });
            }
        }
    }
    out
}

fn invalid_params(msg: impl Into<String>) -> ResponseError {
    ResponseError {
        code: ErrorCode::InvalidParams as i32,
        message: msg.into(),
        data: None,
    }
}

fn internal_error(msg: impl Into<String>) -> ResponseError {
    ResponseError {
        code: ErrorCode::InternalError as i32,
        message: msg.into(),
        data: None,
    }
}

/// Dispatch a `workspace/executeCommand` request to the matching
/// run-from-gutter handler. Returns `Ok(None)` when the command ID does
/// not belong to this module so the caller can route elsewhere.
pub fn dispatch(
    command: &str,
    arguments: &[Value],
    sink: &dyn NotificationSink,
) -> Option<Result<Value, ResponseError>> {
    let arg0 = match arguments.first() {
        Some(v) => v.clone(),
        None => {
            return Some(Err(invalid_params(format!(
                "{command} requires one argument object"
            ))))
        }
    };
    let envelope_to_value = |env: RunOutcomeEnvelope| -> Result<Value, ResponseError> {
        serde_json::to_value(env).map_err(|e| internal_error(format!("serialize envelope: {e}")))
    };
    let result = match command {
        RUN_FILE_COMMAND => {
            let args: RunFileArgs = match serde_json::from_value(arg0) {
                Ok(v) => v,
                Err(e) => return Some(Err(invalid_params(format!("{command}: {e}")))),
            };
            execute_run_file(&args, sink).and_then(envelope_to_value)
        }
        RUN_TEST_COMMAND => {
            let args: RunTestArgs = match serde_json::from_value(arg0) {
                Ok(v) => v,
                Err(e) => return Some(Err(invalid_params(format!("{command}: {e}")))),
            };
            execute_run_test(&args, sink).and_then(envelope_to_value)
        }
        RUN_STEP_COMMAND => {
            let args: RunStepArgs = match serde_json::from_value(arg0) {
                Ok(v) => v,
                Err(e) => return Some(Err(invalid_params(format!("{command}: {e}")))),
            };
            execute_run_step(&args, sink).and_then(envelope_to_value)
        }
        RUN_LAST_FAILURES_COMMAND => {
            let args: RunLastFailuresArgs = match serde_json::from_value(arg0) {
                Ok(v) => v,
                Err(e) => return Some(Err(invalid_params(format!("{command}: {e}")))),
            };
            execute_run_last_failures(&args, sink).and_then(envelope_to_value)
        }
        _ => return None,
    };
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_file_arg_accepts_plain_path() {
        let p = parse_file_arg("/tmp/foo.tarn.yaml").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/foo.tarn.yaml"));
    }

    #[test]
    fn parse_file_arg_accepts_file_url() {
        // Build the URL from a platform-valid absolute path so this works
        // on Windows (where `file:///tmp/foo.tarn.yaml` is not a valid
        // file URL — `Url::to_file_path` requires a drive letter).
        let path = std::env::temp_dir().join("foo.tarn.yaml");
        let url = lsp_types::Url::from_file_path(&path).expect("path is absolute");
        let p = parse_file_arg(url.as_str()).unwrap();
        assert_eq!(p, path);
    }

    #[test]
    fn step_value_to_string_handles_int_and_string() {
        assert_eq!(step_value_to_string(&json!(2)).unwrap(), "2");
        assert_eq!(step_value_to_string(&json!("my step")).unwrap(), "my step");
        assert!(step_value_to_string(&json!(true)).is_err());
    }

    #[test]
    fn extract_failures_filters_by_status() {
        let artifact = LastRunArtifact {
            env_name: Some("staging".into()),
            files: vec![LastRunFile {
                file: "tests/users.tarn.yaml".into(),
                tests: vec![
                    LastRunTest {
                        name: "login".into(),
                        status: "PASSED".into(),
                    },
                    LastRunTest {
                        name: "logout".into(),
                        status: "FAILED".into(),
                    },
                    LastRunTest {
                        name: "admin".into(),
                        status: "FAILED".into(),
                    },
                ],
            }],
        };
        let fails = extract_failures(&artifact);
        assert_eq!(fails.len(), 2);
        assert_eq!(fails[0].test, "logout");
        assert_eq!(fails[1].test, "admin");
    }

    #[test]
    fn extract_failures_is_case_insensitive() {
        let artifact = LastRunArtifact {
            env_name: None,
            files: vec![LastRunFile {
                file: "f.tarn.yaml".into(),
                tests: vec![LastRunTest {
                    name: "t".into(),
                    status: "failed".into(),
                }],
            }],
        };
        assert_eq!(extract_failures(&artifact).len(), 1);
    }

    #[test]
    fn dispatch_returns_none_for_unknown_command() {
        let sink = CapturingSink::new();
        let res = dispatch("tarn.unknown", &[json!({})], &sink);
        assert!(res.is_none());
    }

    #[test]
    fn dispatch_requires_argument_object() {
        let sink = CapturingSink::new();
        let res = dispatch(RUN_FILE_COMMAND, &[], &sink).expect("should dispatch");
        let err = res.expect_err("missing args should error");
        assert_eq!(err.code, ErrorCode::InvalidParams as i32);
        assert!(err.message.contains("requires one argument object"));
    }

    #[test]
    fn capturing_sink_collects_notifications_in_order() {
        let sink = CapturingSink::new();
        sink.send(Notification {
            method: "a".into(),
            params: json!({}),
        })
        .unwrap();
        sink.send(Notification {
            method: "b".into(),
            params: json!({}),
        })
        .unwrap();
        let recorded = sink.notifications();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].method, "a");
        assert_eq!(recorded[1].method, "b");
    }
}
