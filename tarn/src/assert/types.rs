use serde::Serialize;
use std::collections::HashMap;

/// Category of failure for structured error reporting.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    AssertionFailed,
    ConnectionError,
    Timeout,
    ParseError,
    CaptureError,
}

/// Result of a single assertion check.
#[derive(Debug, Clone)]
pub struct AssertionResult {
    /// What was asserted (e.g., "status", "body $.name", "header content-type")
    pub assertion: String,
    /// Whether it passed
    pub passed: bool,
    /// Expected value (for display)
    pub expected: String,
    /// Actual value (for display)
    pub actual: String,
    /// Human-readable message
    pub message: String,
}

impl AssertionResult {
    pub fn pass(
        assertion: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        let assertion = assertion.into();
        let expected = expected.into();
        let actual = actual.into();
        Self {
            message: format!("{}: OK", assertion),
            assertion,
            passed: true,
            expected,
            actual,
        }
    }

    pub fn fail(
        assertion: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            assertion: assertion.into(),
            passed: false,
            expected: expected.into(),
            actual: actual.into(),
            message: message.into(),
        }
    }
}

/// Result of executing a single step.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub name: String,
    pub passed: bool,
    pub duration_ms: u64,
    pub assertion_results: Vec<AssertionResult>,
    /// HTTP request details (included in JSON output for failed steps)
    pub request_info: Option<RequestInfo>,
    /// HTTP response details (included in JSON output for failed steps)
    pub response_info: Option<ResponseInfo>,
    /// Category of failure (for structured error taxonomy in JSON output)
    pub error_category: Option<FailureCategory>,
}

impl StepResult {
    pub fn total_assertions(&self) -> usize {
        self.assertion_results.len()
    }

    pub fn passed_assertions(&self) -> usize {
        self.assertion_results.iter().filter(|a| a.passed).count()
    }

    pub fn failed_assertions(&self) -> usize {
        self.assertion_results.iter().filter(|a| !a.passed).count()
    }

    pub fn failures(&self) -> Vec<&AssertionResult> {
        self.assertion_results
            .iter()
            .filter(|a| !a.passed)
            .collect()
    }
}

/// HTTP request info for reporting.
#[derive(Debug, Clone)]
pub struct RequestInfo {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<serde_json::Value>,
}

/// HTTP response info for reporting.
#[derive(Debug, Clone)]
pub struct ResponseInfo {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Option<serde_json::Value>,
}

/// Result of a named test group.
#[derive(Debug, Clone)]
pub struct TestResult {
    pub name: String,
    pub description: Option<String>,
    pub passed: bool,
    pub duration_ms: u64,
    pub step_results: Vec<StepResult>,
}

impl TestResult {
    pub fn total_steps(&self) -> usize {
        self.step_results.len()
    }

    pub fn passed_steps(&self) -> usize {
        self.step_results.iter().filter(|s| s.passed).count()
    }

    pub fn failed_steps(&self) -> usize {
        self.step_results.iter().filter(|s| !s.passed).count()
    }
}

/// Result of running an entire test file.
#[derive(Debug, Clone)]
pub struct FileResult {
    pub file: String,
    pub name: String,
    pub passed: bool,
    pub duration_ms: u64,
    pub setup_results: Vec<StepResult>,
    pub test_results: Vec<TestResult>,
    pub teardown_results: Vec<StepResult>,
}

impl FileResult {
    pub fn total_steps(&self) -> usize {
        self.setup_results.len()
            + self
                .test_results
                .iter()
                .map(|t| t.total_steps())
                .sum::<usize>()
            + self.teardown_results.len()
    }

    pub fn passed_steps(&self) -> usize {
        self.setup_results.iter().filter(|s| s.passed).count()
            + self
                .test_results
                .iter()
                .map(|t| t.passed_steps())
                .sum::<usize>()
            + self.teardown_results.iter().filter(|s| s.passed).count()
    }

    pub fn failed_steps(&self) -> usize {
        self.total_steps() - self.passed_steps()
    }
}

/// Top-level run result.
#[derive(Debug, Clone)]
pub struct RunResult {
    pub file_results: Vec<FileResult>,
    pub duration_ms: u64,
}

impl RunResult {
    pub fn passed(&self) -> bool {
        self.file_results.iter().all(|f| f.passed)
    }

    pub fn total_files(&self) -> usize {
        self.file_results.len()
    }

    pub fn total_steps(&self) -> usize {
        self.file_results.iter().map(|f| f.total_steps()).sum()
    }

    pub fn passed_steps(&self) -> usize {
        self.file_results.iter().map(|f| f.passed_steps()).sum()
    }

    pub fn failed_steps(&self) -> usize {
        self.file_results.iter().map(|f| f.failed_steps()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assertion_result_pass() {
        let r = AssertionResult::pass("status", "200", "200");
        assert!(r.passed);
        assert_eq!(r.assertion, "status");
        assert_eq!(r.expected, "200");
        assert_eq!(r.actual, "200");
    }

    #[test]
    fn assertion_result_fail() {
        let r = AssertionResult::fail("status", "200", "404", "Expected 200, got 404");
        assert!(!r.passed);
        assert_eq!(r.message, "Expected 200, got 404");
    }

    #[test]
    fn step_result_counts() {
        let sr = StepResult {
            name: "test".into(),
            passed: false,
            duration_ms: 100,
            assertion_results: vec![
                AssertionResult::pass("status", "200", "200"),
                AssertionResult::fail("body $.name", "\"Alice\"", "\"Bob\"", "mismatch"),
                AssertionResult::pass("body $.age", "30", "30"),
            ],
            request_info: None,
            response_info: None,
            error_category: None,
        };
        assert_eq!(sr.total_assertions(), 3);
        assert_eq!(sr.passed_assertions(), 2);
        assert_eq!(sr.failed_assertions(), 1);
        assert_eq!(sr.failures().len(), 1);
        assert_eq!(sr.failures()[0].assertion, "body $.name");
    }

    #[test]
    fn test_result_counts() {
        let tr = TestResult {
            name: "crud".into(),
            description: Some("CRUD test".into()),
            passed: false,
            duration_ms: 500,
            step_results: vec![
                StepResult {
                    name: "create".into(),
                    passed: true,
                    duration_ms: 200,
                    assertion_results: vec![AssertionResult::pass("status", "201", "201")],
                    request_info: None,
                    response_info: None,
                    error_category: None,
                },
                StepResult {
                    name: "verify".into(),
                    passed: false,
                    duration_ms: 100,
                    assertion_results: vec![AssertionResult::fail(
                        "status",
                        "200",
                        "404",
                        "not found",
                    )],
                    request_info: None,
                    response_info: None,
                    error_category: None,
                },
            ],
        };
        assert_eq!(tr.total_steps(), 2);
        assert_eq!(tr.passed_steps(), 1);
        assert_eq!(tr.failed_steps(), 1);
    }

    #[test]
    fn file_result_counts() {
        let fr = FileResult {
            file: "test.tarn.yaml".into(),
            name: "Test".into(),
            passed: true,
            duration_ms: 1000,
            setup_results: vec![StepResult {
                name: "setup".into(),
                passed: true,
                duration_ms: 50,
                assertion_results: vec![],
                request_info: None,
                response_info: None,
                error_category: None,
            }],
            test_results: vec![TestResult {
                name: "t1".into(),
                description: None,
                passed: true,
                duration_ms: 800,
                step_results: vec![
                    StepResult {
                        name: "s1".into(),
                        passed: true,
                        duration_ms: 400,
                        assertion_results: vec![],
                        request_info: None,
                        response_info: None,
                        error_category: None,
                    },
                    StepResult {
                        name: "s2".into(),
                        passed: true,
                        duration_ms: 400,
                        assertion_results: vec![],
                        request_info: None,
                        response_info: None,
                        error_category: None,
                    },
                ],
            }],
            teardown_results: vec![],
        };
        assert_eq!(fr.total_steps(), 3);
        assert_eq!(fr.passed_steps(), 3);
        assert_eq!(fr.failed_steps(), 0);
    }

    #[test]
    fn run_result_aggregation() {
        let rr = RunResult {
            file_results: vec![
                FileResult {
                    file: "a.tarn.yaml".into(),
                    name: "A".into(),
                    passed: true,
                    duration_ms: 100,
                    setup_results: vec![],
                    test_results: vec![TestResult {
                        name: "t".into(),
                        description: None,
                        passed: true,
                        duration_ms: 100,
                        step_results: vec![StepResult {
                            name: "s".into(),
                            passed: true,
                            duration_ms: 100,
                            assertion_results: vec![],
                            request_info: None,
                            response_info: None,
                            error_category: None,
                        }],
                    }],
                    teardown_results: vec![],
                },
                FileResult {
                    file: "b.tarn.yaml".into(),
                    name: "B".into(),
                    passed: false,
                    duration_ms: 200,
                    setup_results: vec![],
                    test_results: vec![TestResult {
                        name: "t".into(),
                        description: None,
                        passed: false,
                        duration_ms: 200,
                        step_results: vec![StepResult {
                            name: "s".into(),
                            passed: false,
                            duration_ms: 200,
                            assertion_results: vec![],
                            request_info: None,
                            response_info: None,
                            error_category: None,
                        }],
                    }],
                    teardown_results: vec![],
                },
            ],
            duration_ms: 300,
        };
        assert!(!rr.passed());
        assert_eq!(rr.total_files(), 2);
        assert_eq!(rr.total_steps(), 2);
        assert_eq!(rr.passed_steps(), 1);
        assert_eq!(rr.failed_steps(), 1);
    }
}
