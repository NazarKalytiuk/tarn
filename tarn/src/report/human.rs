use crate::assert::types::{FileResult, RunResult, StepResult};
use crate::report::redaction::sanitize_assertion;
use colored::Colorize;

/// Render test results as colored human-readable output.
pub fn render(result: &RunResult) -> String {
    let mut output = String::new();

    for file_result in &result.file_results {
        render_file(&mut output, file_result);
    }

    // Summary line
    let passed = result.passed_steps();
    let failed = result.failed_steps();
    let duration = result.duration_ms;

    output.push('\n');
    if failed == 0 {
        output.push_str(&format!(
            " {} {} passed ({}ms)\n",
            "Results:".bold(),
            passed.to_string().green(),
            duration
        ));
    } else {
        output.push_str(&format!(
            " {} {} passed, {} failed ({}ms)\n",
            "Results:".bold(),
            passed.to_string().green(),
            failed.to_string().red(),
            duration
        ));
    }

    output
}

fn render_file(output: &mut String, file_result: &FileResult) {
    output.push_str(&format!(
        "\n {} Running {}\n\n",
        "TARN".bold().white().on_blue(),
        file_result.file.dimmed()
    ));

    output.push_str(&format!(" {} {}\n", "●".bold(), file_result.name.bold()));

    // Setup
    if !file_result.setup_results.is_empty() {
        output.push_str(&format!("\n   {}\n", "Setup".dimmed()));
        for step in &file_result.setup_results {
            render_step(output, step, file_result);
        }
    }

    // Tests
    for test in &file_result.test_results {
        output.push('\n');
        if let Some(ref desc) = test.description {
            output.push_str(&format!("   {} — {}\n", test.name.bold(), desc.dimmed()));
        } else {
            output.push_str(&format!("   {}\n", test.name.bold()));
        }
        for step in &test.step_results {
            render_step(output, step, file_result);
        }
    }

    // Teardown
    if !file_result.teardown_results.is_empty() {
        output.push_str(&format!("\n   {}\n", "Teardown".dimmed()));
        for step in &file_result.teardown_results {
            render_step(output, step, file_result);
        }
    }
}

fn render_step(output: &mut String, step: &StepResult, file_result: &FileResult) {
    if step.passed {
        output.push_str(&format!(
            "   {} {} ({}ms)\n",
            "✓".green(),
            step.name,
            step.duration_ms
        ));
    } else {
        output.push_str(&format!(
            "   {} {} ({}ms)\n",
            "✗".red(),
            step.name.red(),
            step.duration_ms
        ));
        // Show failure details
        let failures = step.failures();
        for (i, failure) in failures.iter().enumerate() {
            let failure = sanitize_assertion(
                failure,
                &file_result.redaction,
                &file_result.redacted_values,
            );
            let connector = if i == failures.len() - 1 {
                "└─"
            } else {
                "├─"
            };
            output.push_str(&format!(
                "     {} {}\n",
                connector.dimmed(),
                failure.message.red()
            ));
            if let Some(diff) = &failure.diff {
                for line in diff.lines() {
                    let colored = if line.starts_with("---") || line.starts_with("+++") {
                        line.bold().to_string()
                    } else if line.starts_with('+') {
                        line.green().to_string()
                    } else if line.starts_with('-') {
                        line.red().to_string()
                    } else {
                        line.dimmed().to_string()
                    };
                    output.push_str(&format!("       {}\n", colored));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::*;
    use std::collections::HashMap;

    fn make_run_result(passed: bool) -> RunResult {
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
                    description: Some("A test".into()),
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
                                "Expected HTTP status 200, got 404",
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
    fn render_passing_test() {
        let result = make_run_result(true);
        let output = render(&result);
        assert!(output.contains("Test Suite"));
        assert!(output.contains("Check status"));
        assert!(output.contains("1")); // 1 passed
    }

    #[test]
    fn render_failing_test() {
        let result = make_run_result(false);
        let output = render(&result);
        assert!(output.contains("Check status"));
        assert!(output.contains("Expected HTTP status 200, got 404"));
    }

    #[test]
    fn render_with_setup_and_teardown() {
        let result = RunResult {
            duration_ms: 200,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Suite".into(),
                passed: true,
                duration_ms: 200,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![StepResult {
                    name: "Auth".into(),
                    passed: true,
                    duration_ms: 50,
                    assertion_results: vec![],
                    request_info: None,
                    response_info: None,
                    error_category: None,
                    response_status: None,
                    response_summary: None,
                    captures_set: vec![],
                }],
                test_results: vec![],
                teardown_results: vec![StepResult {
                    name: "Cleanup".into(),
                    passed: true,
                    duration_ms: 30,
                    assertion_results: vec![],
                    request_info: None,
                    response_info: None,
                    error_category: None,
                    response_status: None,
                    response_summary: None,
                    captures_set: vec![],
                }],
            }],
        };
        let output = render(&result);
        assert!(output.contains("Setup"));
        assert!(output.contains("Auth"));
        assert!(output.contains("Teardown"));
        assert!(output.contains("Cleanup"));
    }

    #[test]
    fn render_multiple_failures_shows_all() {
        let result = RunResult {
            duration_ms: 100,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Suite".into(),
                passed: false,
                duration_ms: 100,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "test".into(),
                    description: None,
                    passed: false,
                    duration_ms: 100,
                    step_results: vec![StepResult {
                        name: "step".into(),
                        passed: false,
                        duration_ms: 50,
                        assertion_results: vec![
                            AssertionResult::fail("status", "200", "403", "status mismatch"),
                            AssertionResult::fail(
                                "body $.error",
                                "\"ok\"",
                                "\"forbidden\"",
                                "body mismatch",
                            ),
                        ],
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
        assert!(output.contains("status mismatch"));
        assert!(output.contains("body mismatch"));
    }

    #[test]
    fn render_whole_body_diff() {
        let result = RunResult {
            duration_ms: 100,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Suite".into(),
                passed: false,
                duration_ms: 100,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "test".into(),
                    description: None,
                    passed: false,
                    duration_ms: 100,
                    step_results: vec![StepResult {
                        name: "step".into(),
                        passed: false,
                        duration_ms: 50,
                        assertion_results: vec![AssertionResult::fail_with_diff(
                            "body $",
                            "{\"name\":\"Alice\"}",
                            "{\"name\":\"Bob\"}",
                            "whole body mismatch",
                            "--- expected\n+++ actual\n-  \"name\": \"Alice\"\n+  \"name\": \"Bob\"\n",
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
        assert!(output.contains("whole body mismatch"));
        assert!(output.contains("--- expected"));
        assert!(output.contains("+++ actual"));
    }

    #[test]
    fn render_redacts_secret_values_in_messages() {
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
                            "200",
                            "401",
                            "Expected secret-token to be accepted",
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
        assert!(output.contains("Expected [hidden] to be accepted"));
    }
}
