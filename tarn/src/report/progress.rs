//! Streaming progress reporters for `tarn run`.
//!
//! A [`ProgressReporter`] is invoked by the runner as files, setups, tests, and
//! teardowns finish, so the user sees live feedback instead of a single dump
//! once all tests have completed. The sequential path streams per-test; the
//! parallel path streams per-file (atomically under a mutex) so interleaved
//! output from different files never gets scrambled.

use crate::assert::types::{AssertionResult, FileResult, RunResult, StepResult, TestResult};
use crate::model::RedactionConfig;
use crate::report::{human, RenderOptions};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::Mutex;

/// Redaction context passed alongside intermediate results so the reporter can
/// apply the same masking the final report would.
#[derive(Debug)]
pub struct ReportContext<'a> {
    pub redaction: &'a RedactionConfig,
    pub redacted_values: &'a [String],
}

/// Whether the progress reporter is running under sequential or parallel
/// execution. Sequential fires per-test hooks; parallel only fires `file_finished`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressMode {
    Sequential,
    Parallel,
}

/// Event sink invoked by the runner as tests progress.
pub trait ProgressReporter: Send + Sync {
    fn file_started(&self, file_path: &str, file_name: &str);
    fn setup_finished(&self, results: &[StepResult], ctx: &ReportContext);
    fn test_finished(&self, test: &TestResult, ctx: &ReportContext);
    fn teardown_finished(&self, results: &[StepResult], ctx: &ReportContext);
    fn file_finished(&self, file: &FileResult);

    /// Emitted once after the whole run completes. Reporters that produce
    /// batch output (human) don't need this hook; machine-readable streams
    /// (NDJSON) use it to emit a final `done` event with the aggregated
    /// summary. Default implementation is empty.
    fn run_finished(&self, _result: &RunResult) {}
}

/// Human-readable progress reporter that writes to a shared writer.
pub struct HumanProgress {
    writer: Mutex<Box<dyn Write + Send>>,
    opts: RenderOptions,
    mode: ProgressMode,
}

impl HumanProgress {
    pub fn new(writer: Box<dyn Write + Send>, opts: RenderOptions, mode: ProgressMode) -> Self {
        Self {
            writer: Mutex::new(writer),
            opts,
            mode,
        }
    }

    fn write(&self, content: &str) {
        if content.is_empty() {
            return;
        }
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(content.as_bytes());
            let _ = w.flush();
        }
    }
}

impl ProgressReporter for HumanProgress {
    fn file_started(&self, file_path: &str, file_name: &str) {
        if self.mode != ProgressMode::Sequential {
            return;
        }
        self.write(&human::render_file_header_parts(file_path, file_name));
    }

    fn setup_finished(&self, results: &[StepResult], ctx: &ReportContext) {
        if self.mode != ProgressMode::Sequential {
            return;
        }
        self.write(&human::render_setup_block(
            results,
            ctx.redaction,
            ctx.redacted_values,
            self.opts,
        ));
    }

    fn test_finished(&self, test: &TestResult, ctx: &ReportContext) {
        if self.mode != ProgressMode::Sequential {
            return;
        }
        self.write(&human::render_test_block(
            test,
            ctx.redaction,
            ctx.redacted_values,
            self.opts,
        ));
    }

    fn teardown_finished(&self, results: &[StepResult], ctx: &ReportContext) {
        if self.mode != ProgressMode::Sequential {
            return;
        }
        self.write(&human::render_teardown_block(
            results,
            ctx.redaction,
            ctx.redacted_values,
            self.opts,
        ));
    }

    fn file_finished(&self, file: &FileResult) {
        match self.mode {
            ProgressMode::Sequential => {
                // Sequential streams setup/tests/teardown as they complete; the
                // trailing newline separates successive files visually.
                if self.opts.only_failed && file.passed {
                    return;
                }
                self.write("\n");
            }
            ProgressMode::Parallel => {
                if self.opts.only_failed && file.passed {
                    return;
                }
                let mut block = String::new();
                block.push_str(&human::render_file_header(file));
                block.push_str(&human::render_setup_block(
                    &file.setup_results,
                    &file.redaction,
                    &file.redacted_values,
                    self.opts,
                ));
                for test in &file.test_results {
                    block.push_str(&human::render_test_block(
                        test,
                        &file.redaction,
                        &file.redacted_values,
                        self.opts,
                    ));
                }
                block.push_str(&human::render_teardown_block(
                    &file.teardown_results,
                    &file.redaction,
                    &file.redacted_values,
                    self.opts,
                ));
                block.push('\n');
                self.write(&block);
            }
        }
    }
}

/// Machine-readable streaming reporter that writes one JSON object per
/// line to a shared writer. Designed for editor integrations, MCP
/// consumers, and structured CI pipelines that want live progress.
///
/// Event schema:
///
/// - `{"event":"file_started","file":"...","file_name":"..."}`
/// - `{"event":"step_finished","file":"...","phase":"setup|test|teardown","test":"<test_name>","step":"...","step_index":N,"status":"PASSED|FAILED","duration_ms":N,...}`
/// - `{"event":"test_finished","file":"...","test":"...","status":"...","duration_ms":N,"steps":{"total":N,"passed":N,"failed":N}}`
/// - `{"event":"file_finished","file":"...","status":"...","duration_ms":N,"summary":{"total":N,"passed":N,"failed":N}}`
/// - `{"event":"done","duration_ms":N,"summary":{"files":N,"tests":N,"steps":{"total":N,"passed":N,"failed":N},"status":"..."}}`
///
/// Failing `step_finished` events include `failure_category` and any
/// `error_code` / `remediation_hints` attached to the step so a consumer
/// can diagnose without waiting for the final report.
pub struct NdjsonProgress {
    writer: Mutex<Box<dyn Write + Send>>,
    mode: ProgressMode,
    /// Tracks the file currently being streamed in sequential mode so step
    /// events can carry the `file` field. In parallel mode this stays empty
    /// because per-test hooks are suppressed — parallel emits the whole
    /// file atomically inside `file_finished`, carrying the file name on
    /// every line.
    current_file: Mutex<Option<String>>,
}

impl NdjsonProgress {
    pub fn new(writer: Box<dyn Write + Send>, mode: ProgressMode) -> Self {
        Self {
            writer: Mutex::new(writer),
            mode,
            current_file: Mutex::new(None),
        }
    }

    fn emit(&self, value: Value) {
        let line = match serde_json::to_string(&value) {
            Ok(s) => s,
            Err(_) => return,
        };
        if let Ok(mut w) = self.writer.lock() {
            let _ = writeln!(w, "{}", line);
            let _ = w.flush();
        }
    }

    fn step_event(
        &self,
        file_path: &str,
        phase: &str,
        test: &str,
        step_index: usize,
        step: &StepResult,
    ) {
        let mut obj = json!({
            "event": "step_finished",
            "file": file_path,
            "phase": phase,
            "test": test,
            "step": step.name,
            "step_index": step_index,
            "status": if step.passed { "PASSED" } else { "FAILED" },
            "duration_ms": step.duration_ms,
        });

        if !step.passed {
            if let Some(category) = step.error_category {
                obj["failure_category"] = serde_json::to_value(category).unwrap_or(Value::Null);
            }
            if let Some(code) = step.error_code() {
                obj["error_code"] = serde_json::to_value(code).unwrap_or(Value::Null);
            }
            let assertion_failures: Vec<&AssertionResult> = step
                .assertion_results
                .iter()
                .filter(|a| !a.passed)
                .collect();
            if !assertion_failures.is_empty() {
                let details: Vec<Value> = assertion_failures
                    .iter()
                    .map(|a| {
                        let mut entry = json!({
                            "assertion": a.assertion,
                            "expected": a.expected,
                            "actual": a.actual,
                        });
                        if !a.message.is_empty() {
                            entry["message"] = json!(a.message);
                        }
                        if let Some(diff) = &a.diff {
                            entry["diff"] = json!(diff);
                        }
                        entry
                    })
                    .collect();
                obj["assertion_failures"] = json!(details);
            }
        }

        self.emit(obj);
    }

    fn step_pass_fail_counts(steps: &[StepResult]) -> (usize, usize) {
        let passed = steps.iter().filter(|s| s.passed).count();
        let failed = steps.len() - passed;
        (passed, failed)
    }
}

impl NdjsonProgress {
    fn current_file_path(&self) -> String {
        self.current_file
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_default()
    }

    fn emit_file_started(&self, file_path: &str, file_name: &str) {
        self.emit(json!({
            "event": "file_started",
            "file": file_path,
            "file_name": file_name,
        }));
    }

    fn emit_test_finished(&self, file_path: &str, test: &TestResult) {
        let (passed, failed) = Self::step_pass_fail_counts(&test.step_results);
        self.emit(json!({
            "event": "test_finished",
            "file": file_path,
            "test": test.name,
            "status": if test.passed { "PASSED" } else { "FAILED" },
            "duration_ms": test.duration_ms,
            "steps": {
                "total": test.step_results.len(),
                "passed": passed,
                "failed": failed,
            },
        }));
    }

    fn emit_file_finished(&self, file: &FileResult) {
        self.emit(json!({
            "event": "file_finished",
            "file": file.file,
            "file_name": file.name,
            "status": if file.passed { "PASSED" } else { "FAILED" },
            "duration_ms": file.duration_ms,
            "summary": {
                "total": file.total_steps(),
                "passed": file.passed_steps(),
                "failed": file.failed_steps(),
            },
        }));
    }
}

impl ProgressReporter for NdjsonProgress {
    fn file_started(&self, file_path: &str, file_name: &str) {
        if self.mode == ProgressMode::Sequential {
            if let Ok(mut guard) = self.current_file.lock() {
                *guard = Some(file_path.to_string());
            }
            self.emit_file_started(file_path, file_name);
        }
    }

    fn setup_finished(&self, results: &[StepResult], _ctx: &ReportContext) {
        if self.mode != ProgressMode::Sequential {
            return;
        }
        let file_path = self.current_file_path();
        for (idx, step) in results.iter().enumerate() {
            self.step_event(&file_path, "setup", "", idx, step);
        }
    }

    fn test_finished(&self, test: &TestResult, _ctx: &ReportContext) {
        if self.mode != ProgressMode::Sequential {
            return;
        }
        let file_path = self.current_file_path();
        for (idx, step) in test.step_results.iter().enumerate() {
            self.step_event(&file_path, "test", &test.name, idx, step);
        }
        self.emit_test_finished(&file_path, test);
    }

    fn teardown_finished(&self, results: &[StepResult], _ctx: &ReportContext) {
        if self.mode != ProgressMode::Sequential {
            return;
        }
        let file_path = self.current_file_path();
        for (idx, step) in results.iter().enumerate() {
            self.step_event(&file_path, "teardown", "", idx, step);
        }
    }

    fn file_finished(&self, file: &FileResult) {
        match self.mode {
            ProgressMode::Sequential => {
                self.emit_file_finished(file);
            }
            ProgressMode::Parallel => {
                // In parallel mode we didn't fire any per-step events, so
                // unroll the entire file here atomically under the writer
                // mutex (each emit() acquires the lock independently, but
                // because there is no interleaving with other threads'
                // hooks for this file, the block stays contiguous).
                self.emit_file_started(&file.file, &file.name);
                for (idx, step) in file.setup_results.iter().enumerate() {
                    self.step_event(&file.file, "setup", "", idx, step);
                }
                for test in &file.test_results {
                    for (idx, step) in test.step_results.iter().enumerate() {
                        self.step_event(&file.file, "test", &test.name, idx, step);
                    }
                    self.emit_test_finished(&file.file, test);
                }
                for (idx, step) in file.teardown_results.iter().enumerate() {
                    self.step_event(&file.file, "teardown", "", idx, step);
                }
                self.emit_file_finished(file);
            }
        }
    }

    fn run_finished(&self, result: &RunResult) {
        let total_steps = result.total_steps();
        let passed_steps = result.passed_steps();
        let failed_steps = result.failed_steps();
        let total_tests: usize = result
            .file_results
            .iter()
            .map(|f| f.test_results.len())
            .sum();
        self.emit(json!({
            "event": "done",
            "duration_ms": result.duration_ms,
            "summary": {
                "files": result.total_files(),
                "tests": total_tests,
                "steps": {
                    "total": total_steps,
                    "passed": passed_steps,
                    "failed": failed_steps,
                },
                "status": if result.passed() { "PASSED" } else { "FAILED" },
            },
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::{AssertionResult, FileResult, StepResult, TestResult};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex as StdMutex};

    struct SharedWriter(Arc<StdMutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn snapshot(buf: &Arc<StdMutex<Vec<u8>>>) -> String {
        String::from_utf8(buf.lock().unwrap().clone()).unwrap()
    }

    fn make_test(name: &str, passed: bool) -> TestResult {
        TestResult {
            name: name.into(),
            description: None,
            passed,
            duration_ms: 10,
            step_results: vec![StepResult {
                name: format!("{}/step", name),
                description: None,
                debug: false,
                passed,
                duration_ms: 10,
                assertion_results: if passed {
                    vec![AssertionResult::pass("status", "200", "200")]
                } else {
                    vec![AssertionResult::fail("status", "200", "500", "boom")]
                },
                request_info: None,
                response_info: None,
                error_category: None,
                response_status: None,
                response_summary: None,
                captures_set: vec![],
                location: None,
            }],
            captures: HashMap::new(),
        }
    }

    fn make_file(name: &str, passed: bool) -> FileResult {
        FileResult {
            file: format!("{}.tarn.yaml", name),
            name: name.into(),
            passed,
            duration_ms: 10,
            redaction: RedactionConfig::default(),
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![make_test("t1", passed)],
            teardown_results: vec![],
        }
    }

    #[test]
    fn sequential_streams_per_test() {
        let buf = Arc::new(StdMutex::new(Vec::new()));
        let progress = HumanProgress::new(
            Box::new(SharedWriter(buf.clone())),
            RenderOptions::default(),
            ProgressMode::Sequential,
        );
        let ctx = ReportContext {
            redaction: &RedactionConfig::default(),
            redacted_values: &[],
        };
        progress.file_started("f.tarn.yaml", "F");
        progress.test_finished(&make_test("t1", true), &ctx);
        progress.test_finished(&make_test("t2", false), &ctx);
        let out = snapshot(&buf);
        assert!(out.contains("Running"));
        assert!(out.contains("f.tarn.yaml"));
        assert!(out.contains("t1"));
        assert!(out.contains("t2"));
        assert!(out.contains("boom"));
    }

    #[test]
    fn sequential_only_failed_skips_passing_tests() {
        let buf = Arc::new(StdMutex::new(Vec::new()));
        let progress = HumanProgress::new(
            Box::new(SharedWriter(buf.clone())),
            RenderOptions {
                only_failed: true,
                ..RenderOptions::default()
            },
            ProgressMode::Sequential,
        );
        let ctx = ReportContext {
            redaction: &RedactionConfig::default(),
            redacted_values: &[],
        };
        progress.test_finished(&make_test("happy", true), &ctx);
        progress.test_finished(&make_test("sad", false), &ctx);
        let out = snapshot(&buf);
        assert!(!out.contains("happy"));
        assert!(out.contains("sad"));
        assert!(out.contains("boom"));
    }

    #[test]
    fn parallel_emits_full_file_atomically_on_file_finished() {
        let buf = Arc::new(StdMutex::new(Vec::new()));
        let progress = HumanProgress::new(
            Box::new(SharedWriter(buf.clone())),
            RenderOptions::default(),
            ProgressMode::Parallel,
        );
        progress.file_finished(&make_file("a", true));
        let out = snapshot(&buf);
        assert!(out.contains("Running"));
        assert!(out.contains("a.tarn.yaml"));
        assert!(out.contains("t1"));
    }

    #[test]
    fn parallel_only_failed_skips_passing_files() {
        let buf = Arc::new(StdMutex::new(Vec::new()));
        let progress = HumanProgress::new(
            Box::new(SharedWriter(buf.clone())),
            RenderOptions {
                only_failed: true,
                ..RenderOptions::default()
            },
            ProgressMode::Parallel,
        );
        progress.file_finished(&make_file("ok", true));
        progress.file_finished(&make_file("broken", false));
        let out = snapshot(&buf);
        assert!(!out.contains("ok.tarn.yaml"));
        assert!(out.contains("broken.tarn.yaml"));
    }

    #[test]
    fn parallel_ignores_per_test_hooks() {
        let buf = Arc::new(StdMutex::new(Vec::new()));
        let progress = HumanProgress::new(
            Box::new(SharedWriter(buf.clone())),
            RenderOptions::default(),
            ProgressMode::Parallel,
        );
        let ctx = ReportContext {
            redaction: &RedactionConfig::default(),
            redacted_values: &[],
        };
        progress.file_started("f.tarn.yaml", "F");
        progress.test_finished(&make_test("t1", true), &ctx);
        let out = snapshot(&buf);
        assert!(out.is_empty());
    }

    // -----------------------------------------------------------------
    // NdjsonProgress
    // -----------------------------------------------------------------

    fn collect_ndjson_events(raw: &str) -> Vec<serde_json::Value> {
        raw.lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .collect()
    }

    fn make_run_result(files: Vec<FileResult>) -> RunResult {
        RunResult {
            duration_ms: files.iter().map(|f| f.duration_ms).sum(),
            file_results: files,
        }
    }

    #[test]
    fn ndjson_sequential_emits_file_and_step_and_test_and_file_events() {
        let buf = Arc::new(StdMutex::new(Vec::new()));
        let progress = NdjsonProgress::new(
            Box::new(SharedWriter(buf.clone())),
            ProgressMode::Sequential,
        );
        let ctx = ReportContext {
            redaction: &RedactionConfig::default(),
            redacted_values: &[],
        };

        progress.file_started("tests/f.tarn.yaml", "F");
        progress.test_finished(&make_test("t1", true), &ctx);
        progress.test_finished(&make_test("t2", false), &ctx);
        let file = FileResult {
            file: "tests/f.tarn.yaml".into(),
            name: "F".into(),
            passed: false,
            duration_ms: 20,
            redaction: RedactionConfig::default(),
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![make_test("t1", true), make_test("t2", false)],
            teardown_results: vec![],
        };
        progress.file_finished(&file);

        let out = snapshot(&buf);
        let events = collect_ndjson_events(&out);

        assert_eq!(events[0]["event"], "file_started");
        assert_eq!(events[0]["file"], "tests/f.tarn.yaml");

        // Per-step events for t1
        assert_eq!(events[1]["event"], "step_finished");
        assert_eq!(events[1]["file"], "tests/f.tarn.yaml");
        assert_eq!(events[1]["test"], "t1");
        assert_eq!(events[1]["step"], "t1/step");
        assert_eq!(events[1]["status"], "PASSED");

        // test_finished for t1
        assert_eq!(events[2]["event"], "test_finished");
        assert_eq!(events[2]["test"], "t1");
        assert_eq!(events[2]["status"], "PASSED");

        // step_finished (failure) for t2
        assert_eq!(events[3]["event"], "step_finished");
        assert_eq!(events[3]["test"], "t2");
        assert_eq!(events[3]["status"], "FAILED");
        let fail_details = events[3]["assertion_failures"].as_array().unwrap();
        assert_eq!(fail_details[0]["assertion"], "status");
        assert_eq!(fail_details[0]["expected"], "200");
        assert_eq!(fail_details[0]["actual"], "500");

        // test_finished (failure) for t2
        assert_eq!(events[4]["event"], "test_finished");
        assert_eq!(events[4]["test"], "t2");
        assert_eq!(events[4]["status"], "FAILED");

        // file_finished
        assert_eq!(events[5]["event"], "file_finished");
        assert_eq!(events[5]["status"], "FAILED");
    }

    #[test]
    fn ndjson_parallel_emits_atomic_per_file_stream_on_file_finished() {
        let buf = Arc::new(StdMutex::new(Vec::new()));
        let progress =
            NdjsonProgress::new(Box::new(SharedWriter(buf.clone())), ProgressMode::Parallel);
        let ctx = ReportContext {
            redaction: &RedactionConfig::default(),
            redacted_values: &[],
        };
        // Per-test hooks should be ignored in parallel mode.
        progress.file_started("a.tarn.yaml", "A");
        progress.test_finished(&make_test("ignored", true), &ctx);

        let out = snapshot(&buf);
        assert!(
            out.is_empty(),
            "parallel mode must not emit from per-test hooks: {}",
            out
        );

        progress.file_finished(&make_file("a", true));
        let out = snapshot(&buf);
        let events = collect_ndjson_events(&out);

        // Expect file_started, step_finished for the embedded test, test_finished, file_finished.
        let names: Vec<&str> = events
            .iter()
            .map(|e| e["event"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "file_started",
                "step_finished",
                "test_finished",
                "file_finished"
            ]
        );
        // Every event must carry the file path.
        for e in &events {
            assert_eq!(e["file"], "a.tarn.yaml");
        }
    }

    #[test]
    fn ndjson_run_finished_emits_done_event_with_summary() {
        let buf = Arc::new(StdMutex::new(Vec::new()));
        let progress = NdjsonProgress::new(
            Box::new(SharedWriter(buf.clone())),
            ProgressMode::Sequential,
        );
        let result = make_run_result(vec![make_file("a", true), make_file("b", false)]);
        progress.run_finished(&result);
        let out = snapshot(&buf);
        let events = collect_ndjson_events(&out);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event"], "done");
        assert_eq!(events[0]["summary"]["files"], 2);
        assert_eq!(events[0]["summary"]["status"], "FAILED");
        assert_eq!(events[0]["summary"]["steps"]["total"], 2);
        assert_eq!(events[0]["summary"]["steps"]["passed"], 1);
        assert_eq!(events[0]["summary"]["steps"]["failed"], 1);
    }

    #[test]
    fn ndjson_default_run_finished_is_a_noop_for_human_progress() {
        let buf = Arc::new(StdMutex::new(Vec::new()));
        let progress = HumanProgress::new(
            Box::new(SharedWriter(buf.clone())),
            RenderOptions::default(),
            ProgressMode::Sequential,
        );
        progress.run_finished(&make_run_result(vec![make_file("a", true)]));
        let out = snapshot(&buf);
        assert!(out.is_empty(), "HumanProgress should ignore run_finished");
    }
}
