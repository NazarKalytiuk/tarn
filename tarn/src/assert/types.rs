use crate::model::{Location, MultipartBody, RedactionConfig};
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
    UnresolvedTemplate,
    /// The step was not executed because a capture referenced in its
    /// request failed in an earlier step of the same test. Carrying a
    /// distinct category (rather than re-emitting `UnresolvedTemplate`)
    /// lets reports collapse cascade fallout under the root cause
    /// without misrepresenting the later step as a fresh failure.
    SkippedDueToFailedCapture,
    /// The step was not executed because `fail_fast_within_test` is on
    /// and an earlier step in the same test already failed.
    SkippedDueToFailFast,
    /// The step was not executed because its inline `if:` / `unless:`
    /// predicate evaluated falsy / truthy respectively. Distinct from
    /// the other skipped categories because there is no underlying
    /// failure — the skip is a feature, not fallout. Steps carrying
    /// this category are reported with `passed: true` so tests don't
    /// fail when a conditional branch legitimately skips.
    SkippedByCondition,
}

/// Stable machine-readable failure code for programmatic handling.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    AssertionMismatch,
    CaptureExtractionFailed,
    PollConditionNotMet,
    RequestTimedOut,
    ConnectionRefused,
    DnsResolutionFailed,
    TlsVerificationFailed,
    RedirectLimitExceeded,
    NetworkError,
    InterpolationFailed,
    ValidationFailed,
    ConfigurationError,
    ParseError,
    /// Paired with `SkippedDueToFailedCapture` / `SkippedDueToFailFast`
    /// so consumers can filter cascade fallout from primary failures.
    SkippedDependency,
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
    /// Optional unified diff for whole-body mismatches
    pub diff: Option<String>,
    /// Source location of the assertion operator key in the originating
    /// YAML file. Optional for backwards compatibility and because not
    /// every assertion originates from a YAML node (e.g. the synthetic
    /// `runtime`/`interpolation` assertions manufactured by the runner).
    pub location: Option<Location>,
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
            diff: None,
            location: None,
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
            diff: None,
            location: None,
        }
    }

    pub fn fail_with_diff(
        assertion: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
        message: impl Into<String>,
        diff: impl Into<String>,
    ) -> Self {
        Self {
            assertion: assertion.into(),
            passed: false,
            expected: expected.into(),
            actual: actual.into(),
            message: message.into(),
            diff: Some(diff.into()),
            location: None,
        }
    }

    /// Attach a source location to this assertion result. Used by the
    /// runner to stamp each assertion with the position of its YAML
    /// operator key so downstream consumers can anchor failures on
    /// the exact source range.
    pub fn with_location(mut self, location: Option<Location>) -> Self {
        self.location = location;
        self
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
    /// HTTP response status code (available for all executed steps)
    pub response_status: Option<u16>,
    /// Brief summary of the response (e.g., "200 OK", "Array[3] items")
    pub response_summary: Option<String>,
    /// Names of captures set by this step
    pub captures_set: Vec<String>,
    /// Source location of the step's `name:` node in the originating
    /// YAML file. Optional for backwards compatibility (e.g. steps
    /// expanded from an `include:` directive do not carry locations in
    /// the first iteration of this feature).
    pub location: Option<Location>,
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

    pub fn error_code(&self) -> Option<ErrorCode> {
        let message = self.primary_failure_message().unwrap_or_default();
        let lower = message.to_ascii_lowercase();

        match self.error_category {
            Some(FailureCategory::AssertionFailed) => Some(ErrorCode::AssertionMismatch),
            Some(FailureCategory::CaptureError) => Some(ErrorCode::CaptureExtractionFailed),
            Some(FailureCategory::Timeout) => {
                if self
                    .assertion_results
                    .iter()
                    .any(|assertion| assertion.assertion == "poll")
                {
                    Some(ErrorCode::PollConditionNotMet)
                } else {
                    Some(ErrorCode::RequestTimedOut)
                }
            }
            Some(FailureCategory::UnresolvedTemplate) => Some(ErrorCode::InterpolationFailed),
            Some(FailureCategory::SkippedDueToFailedCapture)
            | Some(FailureCategory::SkippedDueToFailFast) => Some(ErrorCode::SkippedDependency),
            // `SkippedByCondition` is an intentional, non-failure skip.
            // Steps in this state carry `passed: true` and return no
            // error code — downstream consumers should treat them as
            // observational, not as cascade fallout.
            Some(FailureCategory::SkippedByCondition) => None,
            Some(FailureCategory::ParseError) => {
                if lower.contains("interpolation") {
                    Some(ErrorCode::InterpolationFailed)
                } else if lower.contains("validation") {
                    Some(ErrorCode::ValidationFailed)
                } else if lower.contains("config") {
                    Some(ErrorCode::ConfigurationError)
                } else {
                    Some(ErrorCode::ParseError)
                }
            }
            Some(FailureCategory::ConnectionError) => {
                if lower.contains("tls verification failed") {
                    Some(ErrorCode::TlsVerificationFailed)
                } else if lower.contains("too many redirects") {
                    Some(ErrorCode::RedirectLimitExceeded)
                } else if lower.contains("connection refused") {
                    Some(ErrorCode::ConnectionRefused)
                } else if lower.contains("failed to lookup")
                    || lower.contains("dns")
                    || lower.contains("no such host")
                    || lower.contains("name or service not known")
                {
                    Some(ErrorCode::DnsResolutionFailed)
                } else {
                    Some(ErrorCode::NetworkError)
                }
            }
            None => None,
        }
    }

    fn primary_failure_message(&self) -> Option<&str> {
        self.assertion_results
            .iter()
            .find(|assertion| !assertion.passed)
            .map(|assertion| assertion.message.as_str())
    }
}

/// HTTP request info for reporting.
#[derive(Debug, Clone)]
pub struct RequestInfo {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<serde_json::Value>,
    pub multipart: Option<MultipartBody>,
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
    /// All captured values at the end of this test group
    pub captures: HashMap<String, serde_json::Value>,
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
    pub redaction: RedactionConfig,
    pub redacted_values: Vec<String>,
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
        assert_eq!(r.diff, None);
    }

    #[test]
    fn assertion_result_fail() {
        let r = AssertionResult::fail("status", "200", "404", "Expected 200, got 404");
        assert!(!r.passed);
        assert_eq!(r.message, "Expected 200, got 404");
        assert_eq!(r.diff, None);
    }

    #[test]
    fn assertion_result_fail_with_diff() {
        let r = AssertionResult::fail_with_diff("body $", "a", "b", "mismatch", "--- expected");
        assert!(!r.passed);
        assert_eq!(r.diff.as_deref(), Some("--- expected"));
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
            response_status: None,
            response_summary: None,
            captures_set: vec![],
            location: None,
        };
        assert_eq!(sr.total_assertions(), 3);
        assert_eq!(sr.passed_assertions(), 2);
        assert_eq!(sr.failed_assertions(), 1);
        assert_eq!(sr.failures().len(), 1);
        assert_eq!(sr.failures()[0].assertion, "body $.name");
    }

    #[test]
    fn step_result_error_code_uses_timeout_subtypes() {
        let poll_timeout = StepResult {
            name: "poll".into(),
            passed: false,
            duration_ms: 0,
            assertion_results: vec![AssertionResult::fail(
                "poll",
                "condition met",
                "not met",
                "Polling timed out",
            )],
            request_info: None,
            response_info: None,
            error_category: Some(FailureCategory::Timeout),
            response_status: None,
            response_summary: None,
            captures_set: vec![],
            location: None,
        };
        assert_eq!(
            poll_timeout.error_code(),
            Some(ErrorCode::PollConditionNotMet)
        );

        let request_timeout = StepResult {
            name: "http".into(),
            passed: false,
            duration_ms: 0,
            assertion_results: vec![AssertionResult::fail(
                "runtime",
                "ok",
                "timeout",
                "Request to https://example.com timed out",
            )],
            request_info: None,
            response_info: None,
            error_category: Some(FailureCategory::Timeout),
            response_status: None,
            response_summary: None,
            captures_set: vec![],
            location: None,
        };
        assert_eq!(
            request_timeout.error_code(),
            Some(ErrorCode::RequestTimedOut)
        );
    }

    #[test]
    fn step_result_error_code_uses_connection_subtypes() {
        let refused = StepResult {
            name: "refused".into(),
            passed: false,
            duration_ms: 0,
            assertion_results: vec![AssertionResult::fail(
                "runtime",
                "ok",
                "error",
                "Connection refused to http://127.0.0.1:1",
            )],
            request_info: None,
            response_info: None,
            error_category: Some(FailureCategory::ConnectionError),
            response_status: None,
            response_summary: None,
            captures_set: vec![],
            location: None,
        };
        assert_eq!(refused.error_code(), Some(ErrorCode::ConnectionRefused));

        let tls = StepResult {
            name: "tls".into(),
            passed: false,
            duration_ms: 0,
            assertion_results: vec![AssertionResult::fail(
                "runtime",
                "ok",
                "error",
                "TLS verification failed for https://example.com",
            )],
            request_info: None,
            response_info: None,
            error_category: Some(FailureCategory::ConnectionError),
            response_status: None,
            response_summary: None,
            captures_set: vec![],
            location: None,
        };
        assert_eq!(tls.error_code(), Some(ErrorCode::TlsVerificationFailed));
    }

    #[test]
    fn step_result_error_code_unresolved_template() {
        let sr = StepResult {
            name: "unresolved".into(),
            passed: false,
            duration_ms: 0,
            assertion_results: vec![AssertionResult::fail(
                "interpolation",
                "all templates resolved",
                "unresolved: capture.id",
                "Unresolved template variables: capture.id",
            )],
            request_info: None,
            response_info: None,
            error_category: Some(FailureCategory::UnresolvedTemplate),
            response_status: None,
            response_summary: None,
            captures_set: vec![],
            location: None,
        };
        assert_eq!(sr.error_code(), Some(ErrorCode::InterpolationFailed));
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
                    response_status: None,
                    response_summary: None,
                    captures_set: vec![],
                    location: None,
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
                    response_status: None,
                    response_summary: None,
                    captures_set: vec![],
                    location: None,
                },
            ],
            captures: HashMap::new(),
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
            redaction: RedactionConfig::default(),
            redacted_values: vec![],
            setup_results: vec![StepResult {
                name: "setup".into(),
                passed: true,
                duration_ms: 50,
                assertion_results: vec![],
                request_info: None,
                response_info: None,
                error_category: None,
                response_status: None,
                response_summary: None,
                captures_set: vec![],
                location: None,
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
                        response_status: None,
                        response_summary: None,
                        captures_set: vec![],
                        location: None,
                    },
                    StepResult {
                        name: "s2".into(),
                        passed: true,
                        duration_ms: 400,
                        assertion_results: vec![],
                        request_info: None,
                        response_info: None,
                        error_category: None,
                        response_status: None,
                        response_summary: None,
                        captures_set: vec![],
                        location: None,
                    },
                ],
                captures: HashMap::new(),
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
                    redaction: RedactionConfig::default(),
                    redacted_values: vec![],
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
                            response_status: None,
                            response_summary: None,
                            captures_set: vec![],
                            location: None,
                        }],
                        captures: HashMap::new(),
                    }],
                    teardown_results: vec![],
                },
                FileResult {
                    file: "b.tarn.yaml".into(),
                    name: "B".into(),
                    passed: false,
                    duration_ms: 200,
                    redaction: RedactionConfig::default(),
                    redacted_values: vec![],
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
                            response_status: None,
                            response_summary: None,
                            captures_set: vec![],
                            location: None,
                        }],
                        captures: HashMap::new(),
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
