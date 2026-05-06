//! Append-only NDJSON stream of run lifecycle events (NAZ-413).
//!
//! A running `tarn run` invocation writes one JSON object per line to
//! `<workspace>/.tarn/runs/<run_id>/events.jsonl`. Agents that consume
//! long runs do not need to wait for `report.json` to land — they tail
//! the events file and parse line-by-line, reacting to failures as they
//! happen.
//!
//! ## Schema
//!
//! Every event carries a fixed header:
//! - `schema_version: 1` — bump on any incompatible change
//! - `run_id: "<id>"` — the same id used for the enclosing `.tarn/runs/<id>/` directory
//! - `ts: "<UTC ISO-8601 with milliseconds>"`
//! - `seq: N` — monotonic 0-based per run; readers use this to order /
//!   detect drops. Ordering across threads is loose — `seq` gives a total
//!   order, `ts` is for human inspection.
//! - `event: "<kind>"`
//!
//! ## Event kinds
//!
//! - `run_started` — `files: [path]`, `parallel: bool`, `run_args: [string]`
//! - `file_started` — `file`, `file_id`
//! - `file_completed` — `file`, `file_id`, `passed`, `duration_ms`, `test_count`, `failed_test_count`
//! - `test_started` — `file`, `file_id`, `test`, `test_id`
//! - `test_completed` — identifiers + `passed`, `duration_ms`, `step_count`, `failed_step_count`
//! - `step_started` — identifiers + `step`, `step_index`, `method`, `url` (redacted)
//! - `step_completed` — identifiers + `passed`, `status` (null when never sent), `failure_category` (snake_case, matches `failures.json`) or null, `duration_ms`, `assertion_count`, `failed_assertion_count`
//! - `capture_failure` — identifiers + `message`, `missing: [name]`
//! - `polling_timeout` — identifiers + `elapsed_ms`, `attempts`, `last_status` (u16 or null)
//! - `run_completed` — `passed`, `exit_code`, `duration_ms`, `summary: { files, tests, steps, failed_files, failed_tests, failed_steps }`
//!
//! Bodies are intentionally absent. The archive's `report.json` holds
//! the full payloads — the stream is a correlation spine, not a
//! transport for responses.
//!
//! ## Writer
//!
//! The file is opened once in append mode and wrapped in a `Mutex<BufWriter<File>>`
//! so concurrent workers (parallel runs) serialize through one mutex.
//! Every `emit()` call writes one line and then `flush()`es the buffer,
//! so an early crash still produces a correct line-bounded prefix.

use crate::assert::types::{FailureCategory, FileResult, StepResult, TestResult};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// The stream schema version. Bumped only on breaking changes.
pub const SCHEMA_VERSION: u32 = 1;

/// All the kinds of events that can appear in `events.jsonl`. The
/// string form is what consumers match against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    RunStarted,
    FileStarted,
    FileCompleted,
    TestStarted,
    TestCompleted,
    StepStarted,
    StepCompleted,
    CaptureFailure,
    PollingTimeout,
    RunCompleted,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::RunStarted => "run_started",
            EventKind::FileStarted => "file_started",
            EventKind::FileCompleted => "file_completed",
            EventKind::TestStarted => "test_started",
            EventKind::TestCompleted => "test_completed",
            EventKind::StepStarted => "step_started",
            EventKind::StepCompleted => "step_completed",
            EventKind::CaptureFailure => "capture_failure",
            EventKind::PollingTimeout => "polling_timeout",
            EventKind::RunCompleted => "run_completed",
        }
    }
}

/// Append-only NDJSON event stream. Callers share one `Arc<EventStream>`
/// across threads; internal state is protected by a `Mutex`.
pub struct EventStream {
    writer: Mutex<BufWriter<File>>,
    path: PathBuf,
    run_id: String,
    seq: AtomicU64,
}

impl EventStream {
    /// Open (or create) `path` in append mode. Parents are created if
    /// missing. The file descriptor is owned for the life of this struct.
    pub fn new(path: PathBuf, run_id: impl Into<String>) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            writer: Mutex::new(BufWriter::new(file)),
            path,
            run_id: run_id.into(),
            seq: AtomicU64::new(0),
        })
    }

    /// Absolute path to the open events file. Used by `main` to copy the
    /// completed stream over to the `.tarn/events.jsonl` pointer.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run id the stream is stamping into every envelope.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Serialize `payload` (a JSON object whose fields become the event
    /// body), inject the common envelope (`schema_version`, `run_id`,
    /// `ts`, `seq`, `event`), write one line, flush, release the lock.
    ///
    /// Errors are swallowed: the events file is a best-effort artifact,
    /// not a correctness signal. A write failure never flips the run's
    /// exit code.
    pub fn emit(&self, kind: EventKind, mut payload: Value) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let envelope = json!({
            "schema_version": SCHEMA_VERSION,
            "run_id": self.run_id,
            "ts": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            "seq": seq,
            "event": kind.as_str(),
        });
        // Merge envelope + payload so consumers always see envelope fields
        // first on the line. `payload` may legally override nothing here —
        // duplicate keys would be a caller bug.
        if let Value::Object(env_map) = envelope {
            if let Value::Object(ref mut payload_map) = payload {
                let mut merged = serde_json::Map::with_capacity(env_map.len() + payload_map.len());
                for (k, v) in env_map {
                    merged.insert(k, v);
                }
                for (k, v) in payload_map.iter() {
                    merged.insert(k.clone(), v.clone());
                }
                payload = Value::Object(merged);
            }
        }
        let line = match serde_json::to_string(&payload) {
            Ok(s) => s,
            Err(_) => return,
        };
        if let Ok(mut guard) = self.writer.lock() {
            let _ = writeln!(guard, "{}", line);
            let _ = guard.flush();
        }
    }

    // --------------------------------------------------------------
    // Typed emit helpers. Callers construct the payload by passing the
    // domain-level fields; the helpers assemble the JSON and call emit.
    // Centralising the field names here keeps the schema documented in
    // one place and prevents divergence between the runner's many hook
    // sites.
    // --------------------------------------------------------------

    pub fn emit_run_started(&self, files: &[String], parallel: bool, run_args: &[String]) {
        self.emit(
            EventKind::RunStarted,
            json!({
                "files": files,
                "parallel": parallel,
                "run_args": run_args,
            }),
        );
    }

    pub fn emit_file_started(&self, file_path: &str) {
        self.emit(
            EventKind::FileStarted,
            json!({
                "file": file_path,
                "file_id": file_id(file_path),
            }),
        );
    }

    pub fn emit_file_completed(&self, file: &FileResult) {
        let failed_test_count = file.test_results.iter().filter(|t| !t.passed).count();
        self.emit(
            EventKind::FileCompleted,
            json!({
                "file": file.file,
                "file_id": file_id(&file.file),
                "passed": file.passed,
                "duration_ms": file.duration_ms,
                "test_count": file.test_results.len(),
                "failed_test_count": failed_test_count,
            }),
        );
    }

    pub fn emit_test_started(&self, file_path: &str, test_name: &str) {
        self.emit(
            EventKind::TestStarted,
            json!({
                "file": file_path,
                "file_id": file_id(file_path),
                "test": test_name,
                "test_id": test_id(file_path, test_name),
            }),
        );
    }

    pub fn emit_test_completed(&self, file_path: &str, test: &TestResult) {
        let failed_step_count = test.step_results.iter().filter(|s| !s.passed).count();
        self.emit(
            EventKind::TestCompleted,
            json!({
                "file": file_path,
                "file_id": file_id(file_path),
                "test": test.name,
                "test_id": test_id(file_path, &test.name),
                "passed": test.passed,
                "duration_ms": test.duration_ms,
                "step_count": test.step_results.len(),
                "failed_step_count": failed_step_count,
            }),
        );
    }

    pub fn emit_step_started(
        &self,
        file_path: &str,
        test_name: &str,
        step_index: usize,
        step_name: &str,
        method: &str,
        url: &str,
    ) {
        self.emit(
            EventKind::StepStarted,
            json!({
                "file": file_path,
                "file_id": file_id(file_path),
                "test": test_name,
                "test_id": test_id(file_path, test_name),
                "step": step_name,
                "step_index": step_index,
                "method": method,
                "url": url,
            }),
        );
    }

    pub fn emit_step_completed(
        &self,
        file_path: &str,
        test_name: &str,
        step_index: usize,
        step: &StepResult,
    ) {
        let failure_category: Value = match step.error_category {
            Some(c) => Value::String(failure_category_name(c).to_string()),
            None => Value::Null,
        };
        let status: Value = match step.response_status {
            Some(s) => Value::from(s),
            None => Value::Null,
        };
        let failed_assertion_count = step.assertion_results.iter().filter(|a| !a.passed).count();
        self.emit(
            EventKind::StepCompleted,
            json!({
                "file": file_path,
                "file_id": file_id(file_path),
                "test": test_name,
                "test_id": test_id(file_path, test_name),
                "step": step.name,
                "step_index": step_index,
                "passed": step.passed,
                "status": status,
                "failure_category": failure_category,
                "duration_ms": step.duration_ms,
                "assertion_count": step.assertion_results.len(),
                "failed_assertion_count": failed_assertion_count,
            }),
        );
    }

    /// Emit a `capture_failure` event. `missing` is the list of capture
    /// *names* (or JSONPath expressions) that did not resolve — the same
    /// names downstream steps will cascade-skip on.
    pub fn emit_capture_failure(
        &self,
        file_path: &str,
        test_name: &str,
        step_index: usize,
        step_name: &str,
        message: &str,
        missing: &[String],
    ) {
        self.emit(
            EventKind::CaptureFailure,
            json!({
                "file": file_path,
                "file_id": file_id(file_path),
                "test": test_name,
                "test_id": test_id(file_path, test_name),
                "step": step_name,
                "step_index": step_index,
                "message": message,
                "missing": missing,
            }),
        );
    }

    pub fn emit_polling_timeout(&self, coords: StepCoords<'_>, timeout: PollingTimeoutInfo) {
        self.emit(
            EventKind::PollingTimeout,
            json!({
                "file": coords.file,
                "file_id": file_id(coords.file),
                "test": coords.test,
                "test_id": test_id(coords.file, coords.test),
                "step": coords.step,
                "step_index": coords.step_index,
                "elapsed_ms": timeout.elapsed_ms,
                "attempts": timeout.attempts,
                "last_status": timeout.last_status,
            }),
        );
    }

    pub fn emit_run_completed(&self, outcome: RunOutcome) {
        self.emit(
            EventKind::RunCompleted,
            json!({
                "passed": outcome.passed,
                "exit_code": outcome.exit_code,
                "duration_ms": outcome.duration_ms,
                "summary": {
                    "files": outcome.files,
                    "tests": outcome.tests,
                    "steps": outcome.steps,
                    "failed_files": outcome.failed_files,
                    "failed_tests": outcome.failed_tests,
                    "failed_steps": outcome.failed_steps,
                },
            }),
        );
    }
}

/// File / test / step coordinates for one event site. Groups the five
/// strings the runner always has on hand so the emit helpers do not
/// balloon into 8+ positional parameters.
#[derive(Debug, Clone, Copy)]
pub struct StepCoords<'a> {
    pub file: &'a str,
    pub test: &'a str,
    pub step: &'a str,
    pub step_index: usize,
}

/// Polling timeout metadata. Carried alongside [`StepCoords`] so
/// `emit_polling_timeout` stays a clean two-argument call.
#[derive(Debug, Clone, Copy)]
pub struct PollingTimeoutInfo {
    pub elapsed_ms: u64,
    pub attempts: u32,
    pub last_status: Option<u16>,
}

/// Summary payload for the terminal `run_completed` event.
#[derive(Debug, Clone, Copy)]
pub struct RunOutcome {
    pub passed: bool,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub files: usize,
    pub tests: usize,
    pub steps: usize,
    pub failed_files: usize,
    pub failed_tests: usize,
    pub failed_steps: usize,
}

/// Derive a stable, short identifier for a file path: `sha256` truncated
/// to 8 hex characters. Stable within a run because file paths are the
/// canonical key the runner uses; consumers correlate across artifacts
/// by joining on `(run_id, file_id)`.
pub fn file_id(file_path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(file_path.as_bytes());
    let digest = hasher.finalize();
    // 8 hex chars = 4 bytes, enough to disambiguate within a single run.
    hex8(&digest[..4])
}

/// Derive a stable identifier for a `(file, test)` pair. Same hashing
/// strategy as [`file_id`] so consumers don't need to know two hash
/// inputs.
pub fn test_id(file_path: &str, test_name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(file_path.as_bytes());
    hasher.update(b"::");
    hasher.update(test_name.as_bytes());
    let digest = hasher.finalize();
    hex8(&digest[..4])
}

fn hex8(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{:02x}", b);
    }
    out
}

/// Canonical snake_case name for a `FailureCategory`. Matches the
/// serialization used by `failures.json` so consumers can join on the
/// string without consulting two enum tables.
fn failure_category_name(c: FailureCategory) -> &'static str {
    match c {
        FailureCategory::AssertionFailed => "assertion_failed",
        FailureCategory::ConnectionError => "connection_error",
        FailureCategory::Timeout => "timeout",
        FailureCategory::ParseError => "parse_error",
        FailureCategory::CaptureError => "capture_error",
        FailureCategory::UnresolvedTemplate => "unresolved_template",
        FailureCategory::ResponseShapeMismatch => "response_shape_mismatch",
        FailureCategory::SkippedDueToFailedCapture => "skipped_due_to_failed_capture",
        FailureCategory::SkippedDueToFailFast => "skipped_due_to_fail_fast",
        FailureCategory::SkippedByCondition => "skipped_by_condition",
        FailureCategory::SkippedByPolicy => "skipped_by_policy",
        FailureCategory::CommandFailed => "command_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::{AssertionResult, StepResult};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn read_lines(path: &Path) -> Vec<Value> {
        let raw = std::fs::read_to_string(path).unwrap();
        raw.lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect()
    }

    fn passing_step(name: &str) -> StepResult {
        StepResult {
            name: name.into(),
            description: None,
            debug: false,
            passed: true,
            duration_ms: 12,
            assertion_results: vec![AssertionResult::pass("status", "200", "200")],
            request_info: None,
            response_info: None,
            error_category: None,
            response_status: Some(200),
            response_summary: Some("200 OK".into()),
            captures_set: vec![],
            location: None,
            response_shape_mismatch: None,
        }
    }

    fn failing_step(name: &str) -> StepResult {
        StepResult {
            name: name.into(),
            description: None,
            debug: false,
            passed: false,
            duration_ms: 7,
            assertion_results: vec![AssertionResult::fail("status", "200", "500", "boom")],
            request_info: None,
            response_info: None,
            error_category: Some(FailureCategory::AssertionFailed),
            response_status: Some(500),
            response_summary: Some("500".into()),
            captures_set: vec![],
            location: None,
            response_shape_mismatch: None,
        }
    }

    fn fixture_stream(tmp: &TempDir, run_id: &str) -> (EventStream, PathBuf) {
        let path = tmp.path().join("events.jsonl");
        let stream = EventStream::new(path.clone(), run_id).unwrap();
        (stream, path)
    }

    #[test]
    fn emit_writes_envelope_and_kind() {
        let tmp = TempDir::new().unwrap();
        let (stream, path) = fixture_stream(&tmp, "r1");
        stream.emit_run_started(&["a.tarn.yaml".into()], false, &["run".into()]);
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1);
        let ev = &lines[0];
        assert_eq!(ev["schema_version"], 1);
        assert_eq!(ev["run_id"], "r1");
        assert_eq!(ev["seq"], 0);
        assert_eq!(ev["event"], "run_started");
        assert_eq!(ev["parallel"], false);
        assert_eq!(ev["files"][0], "a.tarn.yaml");
        assert!(ev["ts"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn seq_counter_is_monotonic_and_zero_based() {
        let tmp = TempDir::new().unwrap();
        let (stream, path) = fixture_stream(&tmp, "r1");
        stream.emit_file_started("a.tarn.yaml");
        stream.emit_file_started("b.tarn.yaml");
        stream.emit_file_started("c.tarn.yaml");
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["seq"], 0);
        assert_eq!(lines[1]["seq"], 1);
        assert_eq!(lines[2]["seq"], 2);
    }

    #[test]
    fn every_write_flushes_so_early_crash_produces_a_correct_prefix() {
        // Emitting one event then reading the file must show that event
        // — the BufWriter must have flushed. Proves the durability
        // guarantee we advertise in the module docstring.
        let tmp = TempDir::new().unwrap();
        let (stream, path) = fixture_stream(&tmp, "r1");
        stream.emit_file_started("a.tarn.yaml");
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["event"], "file_started");
    }

    #[test]
    fn file_started_carries_file_id_of_path() {
        let tmp = TempDir::new().unwrap();
        let (stream, path) = fixture_stream(&tmp, "r1");
        stream.emit_file_started("tests/a.tarn.yaml");
        let lines = read_lines(&path);
        assert_eq!(lines[0]["file"], "tests/a.tarn.yaml");
        assert_eq!(lines[0]["file_id"], file_id("tests/a.tarn.yaml"));
        assert_eq!(lines[0]["file_id"].as_str().unwrap().len(), 8);
    }

    #[test]
    fn step_completed_maps_failure_category_to_snake_case_string() {
        let tmp = TempDir::new().unwrap();
        let (stream, path) = fixture_stream(&tmp, "r1");
        stream.emit_step_completed("a.tarn.yaml", "t1", 0, &failing_step("s1"));
        let lines = read_lines(&path);
        assert_eq!(lines[0]["event"], "step_completed");
        assert_eq!(lines[0]["failure_category"], "assertion_failed");
        assert_eq!(lines[0]["status"], 500);
        assert_eq!(lines[0]["passed"], false);
        assert_eq!(lines[0]["assertion_count"], 1);
        assert_eq!(lines[0]["failed_assertion_count"], 1);
    }

    #[test]
    fn step_completed_on_pass_nulls_failure_category() {
        let tmp = TempDir::new().unwrap();
        let (stream, path) = fixture_stream(&tmp, "r1");
        stream.emit_step_completed("a.tarn.yaml", "t1", 0, &passing_step("s1"));
        let lines = read_lines(&path);
        assert!(lines[0]["failure_category"].is_null());
        assert_eq!(lines[0]["status"], 200);
        assert_eq!(lines[0]["passed"], true);
        assert_eq!(lines[0]["failed_assertion_count"], 0);
    }

    #[test]
    fn capture_failure_carries_message_and_missing_list() {
        let tmp = TempDir::new().unwrap();
        let (stream, path) = fixture_stream(&tmp, "r1");
        stream.emit_capture_failure(
            "a.tarn.yaml",
            "t1",
            2,
            "fetch user",
            "jsonpath $.user.id missed",
            &["user_id".into(), "user_email".into()],
        );
        let lines = read_lines(&path);
        assert_eq!(lines[0]["event"], "capture_failure");
        assert_eq!(lines[0]["message"], "jsonpath $.user.id missed");
        assert_eq!(lines[0]["missing"][0], "user_id");
        assert_eq!(lines[0]["missing"][1], "user_email");
        assert_eq!(lines[0]["step_index"], 2);
    }

    fn coords<'a>() -> StepCoords<'a> {
        StepCoords {
            file: "a.tarn.yaml",
            test: "t1",
            step: "s1",
            step_index: 0,
        }
    }

    #[test]
    fn polling_timeout_carries_elapsed_attempts_and_last_status() {
        let tmp = TempDir::new().unwrap();
        let (stream, path) = fixture_stream(&tmp, "r1");
        stream.emit_polling_timeout(
            coords(),
            PollingTimeoutInfo {
                elapsed_ms: 5_000,
                attempts: 3,
                last_status: Some(503),
            },
        );
        let lines = read_lines(&path);
        assert_eq!(lines[0]["event"], "polling_timeout");
        assert_eq!(lines[0]["elapsed_ms"], 5_000);
        assert_eq!(lines[0]["attempts"], 3);
        assert_eq!(lines[0]["last_status"], 503);
    }

    #[test]
    fn polling_timeout_with_no_status_serializes_null() {
        let tmp = TempDir::new().unwrap();
        let (stream, path) = fixture_stream(&tmp, "r1");
        stream.emit_polling_timeout(
            coords(),
            PollingTimeoutInfo {
                elapsed_ms: 5_000,
                attempts: 3,
                last_status: None,
            },
        );
        let lines = read_lines(&path);
        assert!(lines[0]["last_status"].is_null());
    }

    #[test]
    fn run_completed_carries_summary_counts_and_exit_code() {
        let tmp = TempDir::new().unwrap();
        let (stream, path) = fixture_stream(&tmp, "r1");
        stream.emit_run_completed(RunOutcome {
            passed: false,
            exit_code: 1,
            duration_ms: 1234,
            files: 2,
            tests: 3,
            steps: 7,
            failed_files: 1,
            failed_tests: 1,
            failed_steps: 2,
        });
        let lines = read_lines(&path);
        assert_eq!(lines[0]["event"], "run_completed");
        assert_eq!(lines[0]["passed"], false);
        assert_eq!(lines[0]["exit_code"], 1);
        assert_eq!(lines[0]["duration_ms"], 1234);
        assert_eq!(lines[0]["summary"]["files"], 2);
        assert_eq!(lines[0]["summary"]["tests"], 3);
        assert_eq!(lines[0]["summary"]["steps"], 7);
        assert_eq!(lines[0]["summary"]["failed_files"], 1);
        assert_eq!(lines[0]["summary"]["failed_tests"], 1);
        assert_eq!(lines[0]["summary"]["failed_steps"], 2);
    }

    #[test]
    fn file_id_is_stable_and_differs_per_path() {
        assert_eq!(file_id("a"), file_id("a"));
        assert_ne!(file_id("a"), file_id("b"));
        assert_eq!(file_id("a").len(), 8);
    }

    #[test]
    fn test_id_is_stable_and_distinguishes_file_or_test_name() {
        assert_eq!(test_id("a", "t"), test_id("a", "t"));
        assert_ne!(test_id("a", "t"), test_id("b", "t"));
        assert_ne!(test_id("a", "t"), test_id("a", "u"));
        assert_eq!(test_id("a", "t").len(), 8);
    }

    #[test]
    fn many_sequential_emits_preserve_ordering_under_shared_mutex() {
        // Concurrency proof-by-contradiction is covered in the
        // integration test; here we verify the single-thread invariant
        // that matters for readers: N emits produce N lines in seq
        // order 0..N.
        let tmp = TempDir::new().unwrap();
        let (stream, path) = fixture_stream(&tmp, "r1");
        for i in 0..50 {
            stream.emit_file_started(&format!("f{i}.tarn.yaml"));
        }
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 50);
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(line["seq"], i as u64);
        }
    }

    #[test]
    fn test_started_and_test_completed_share_identifiers() {
        let tmp = TempDir::new().unwrap();
        let (stream, path) = fixture_stream(&tmp, "r1");
        stream.emit_test_started("a.tarn.yaml", "happy");
        let test = TestResult {
            name: "happy".into(),
            description: None,
            passed: true,
            duration_ms: 100,
            step_results: vec![passing_step("s1")],
            captures: HashMap::new(),
        };
        stream.emit_test_completed("a.tarn.yaml", &test);
        let lines = read_lines(&path);
        assert_eq!(lines[0]["test_id"], lines[1]["test_id"]);
        assert_eq!(lines[0]["file_id"], lines[1]["file_id"]);
        assert_eq!(lines[1]["step_count"], 1);
        assert_eq!(lines[1]["failed_step_count"], 0);
    }
}
