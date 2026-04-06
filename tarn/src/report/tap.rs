use crate::assert::types::RunResult;
use crate::report::redaction::sanitize_assertion;

/// Render test results in TAP (Test Anything Protocol) v13 format.
pub fn render(result: &RunResult) -> String {
    let mut output = String::new();
    output.push_str("TAP version 13\n");

    let total = result.total_steps();
    output.push_str(&format!("1..{}\n", total));

    let mut test_num = 0;

    for file in &result.file_results {
        output.push_str(&format!("# {}\n", file.name));

        // Setup
        for step in &file.setup_results {
            test_num += 1;
            render_step(&mut output, test_num, "setup", step, file);
        }

        // Tests
        for test in &file.test_results {
            output.push_str(&format!("# {}\n", test.name));
            for step in &test.step_results {
                test_num += 1;
                render_step(&mut output, test_num, &test.name, step, file);
            }
        }

        // Teardown
        for step in &file.teardown_results {
            test_num += 1;
            render_step(&mut output, test_num, "teardown", step, file);
        }
    }

    output
}

fn render_step(
    output: &mut String,
    num: usize,
    group: &str,
    step: &crate::assert::types::StepResult,
    file: &crate::assert::types::FileResult,
) {
    if step.passed {
        output.push_str(&format!(
            "ok {} - {} > {} ({}ms)\n",
            num, group, step.name, step.duration_ms
        ));
    } else {
        output.push_str(&format!(
            "not ok {} - {} > {} ({}ms)\n",
            num, group, step.name, step.duration_ms
        ));

        // YAML diagnostic block for failures
        output.push_str("  ---\n");
        for failure in step.failures() {
            let failure = sanitize_assertion(failure, &file.redaction, &file.redacted_values);
            output.push_str(&format!("  message: \"{}\"\n", failure.message));
            output.push_str(&format!("  expected: {}\n", failure.expected));
            output.push_str(&format!("  actual: {}\n", failure.actual));
        }
        output.push_str("  ...\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::*;
    use std::collections::HashMap;

    fn make_result(passed: bool) -> RunResult {
        RunResult {
            duration_ms: 100,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Test Suite".into(),
                passed,
                duration_ms: 100,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "my_test".into(),
                    description: None,
                    passed,
                    duration_ms: 100,
                    step_results: vec![StepResult {
                        name: "Check status".into(),
                        passed,
                        duration_ms: 50,
                        assertion_results: if passed {
                            vec![AssertionResult::pass("status", "200", "200")]
                        } else {
                            vec![AssertionResult::fail(
                                "status",
                                "200",
                                "404",
                                "Expected 200, got 404",
                            )]
                        },
                        request_info: None,
                        response_info: None,
                        error_category: None,
                        response_status: None,
                        response_summary: None,
                        captures_set: vec![],
                    }],
                    captures: HashMap::new(),
                }],
                teardown_results: vec![],
            }],
        }
    }

    #[test]
    fn tap_starts_with_version() {
        let output = render(&make_result(true));
        assert!(output.starts_with("TAP version 13\n"));
    }

    #[test]
    fn tap_has_plan_line() {
        let output = render(&make_result(true));
        assert!(output.contains("1..1\n"));
    }

    #[test]
    fn tap_passing_test() {
        let output = render(&make_result(true));
        assert!(output.contains("ok 1 - my_test > Check status"));
    }

    #[test]
    fn tap_failing_test() {
        let output = render(&make_result(false));
        assert!(output.contains("not ok 1 - my_test > Check status"));
    }

    #[test]
    fn tap_failure_diagnostic() {
        let output = render(&make_result(false));
        assert!(output.contains("  ---\n"));
        assert!(output.contains("  message:"));
        assert!(output.contains("Expected 200, got 404"));
        assert!(output.contains("  ...\n"));
    }

    #[test]
    fn tap_includes_duration() {
        let output = render(&make_result(true));
        assert!(output.contains("(50ms)"));
    }

    #[test]
    fn tap_multiple_steps() {
        let result = RunResult {
            duration_ms: 200,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Suite".into(),
                passed: true,
                duration_ms: 200,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "test".into(),
                    description: None,
                    passed: true,
                    duration_ms: 200,
                    step_results: vec![
                        StepResult {
                            name: "step1".into(),
                            passed: true,
                            duration_ms: 100,
                            assertion_results: vec![],
                            request_info: None,
                            response_info: None,
                            error_category: None,
                            response_status: None,
                            response_summary: None,
                            captures_set: vec![],
                        },
                        StepResult {
                            name: "step2".into(),
                            passed: true,
                            duration_ms: 100,
                            assertion_results: vec![],
                            request_info: None,
                            response_info: None,
                            error_category: None,
                            response_status: None,
                            response_summary: None,
                            captures_set: vec![],
                        },
                    ],
                    captures: HashMap::new(),
                }],
                teardown_results: vec![],
            }],
        };
        let output = render(&result);
        assert!(output.contains("1..2\n"));
        assert!(output.contains("ok 1 -"));
        assert!(output.contains("ok 2 -"));
    }

    #[test]
    fn tap_with_comment_headers() {
        let output = render(&make_result(true));
        assert!(output.contains("# Test Suite\n"));
        assert!(output.contains("# my_test\n"));
    }

    #[test]
    fn tap_redacts_secret_values() {
        let result = RunResult {
            duration_ms: 10,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Suite".into(),
                passed: false,
                duration_ms: 10,
                redaction: crate::model::RedactionConfig {
                    replacement: "[hidden]".into(),
                    ..crate::model::RedactionConfig::default()
                },
                redacted_values: vec!["secret-token".into()],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "test".into(),
                    description: None,
                    passed: false,
                    duration_ms: 10,
                    step_results: vec![StepResult {
                        name: "step".into(),
                        passed: false,
                        duration_ms: 10,
                        assertion_results: vec![AssertionResult::fail(
                            "status",
                            "secret-token",
                            "secret-token",
                            "Expected secret-token",
                        )],
                        request_info: None,
                        response_info: None,
                        error_category: None,
                        response_status: None,
                        response_summary: None,
                        captures_set: vec![],
                    }],
                    captures: HashMap::new(),
                }],
                teardown_results: vec![],
            }],
        };

        let output = render(&result);
        assert!(!output.contains("secret-token"));
        assert!(output.contains("[hidden]"));
    }
}
