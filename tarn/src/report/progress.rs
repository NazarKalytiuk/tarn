//! Streaming progress reporters for `tarn run`.
//!
//! A [`ProgressReporter`] is invoked by the runner as files, setups, tests, and
//! teardowns finish, so the user sees live feedback instead of a single dump
//! once all tests have completed. The sequential path streams per-test; the
//! parallel path streams per-file (atomically under a mutex) so interleaved
//! output from different files never gets scrambled.

use crate::assert::types::{FileResult, StepResult, TestResult};
use crate::model::RedactionConfig;
use crate::report::{human, RenderOptions};
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
            RenderOptions { only_failed: true },
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
            RenderOptions { only_failed: true },
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
}
