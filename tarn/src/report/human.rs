use crate::assert::hints::step_hints;
use crate::assert::types::{FailureCategory, FileResult, RunResult, StepResult, TestResult};
use crate::model::RedactionConfig;
use crate::report::redaction::sanitize_assertion;
use crate::report::RenderOptions;
use colored::Colorize;

/// Render test results as colored human-readable output.
pub fn render(result: &RunResult) -> String {
    render_with_options(result, RenderOptions::default())
}

/// Render test results as colored human-readable output with the given options.
pub fn render_with_options(result: &RunResult, opts: RenderOptions) -> String {
    let mut output = String::new();

    for file_result in &result.file_results {
        if opts.only_failed && file_result.passed {
            continue;
        }
        render_file(&mut output, file_result, opts);
    }

    output.push_str(&render_summary(result));
    output
}

/// Render the trailing summary line (with a leading newline) for a run result.
pub fn render_summary(result: &RunResult) -> String {
    let passed = result.passed_steps();
    let failed = result.failed_steps();
    let duration = result.duration_ms;

    let mut out = String::from("\n");
    if failed == 0 {
        out.push_str(&format!(
            " {} {} passed ({}ms)\n",
            "Results:".bold(),
            passed.to_string().green(),
            duration
        ));
    } else {
        out.push_str(&format!(
            " {} {} passed, {} failed ({}ms)\n",
            "Results:".bold(),
            passed.to_string().green(),
            failed.to_string().red(),
            duration
        ));
    }
    out
}

/// Render the file header line (the `TARN Running <path>` banner plus file name).
pub fn render_file_header(file_result: &FileResult) -> String {
    render_file_header_parts(&file_result.file, &file_result.name)
}

/// Render a file header from its raw parts. Used by progress reporters that
/// don't yet have a `FileResult` struct built (e.g. streaming before tests run).
pub fn render_file_header_parts(file_path: &str, file_name: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "\n {} Running {}\n\n",
        "TARN".bold().white().on_blue(),
        file_path.dimmed()
    ));
    out.push_str(&format!(" {} {}\n", "●".bold(), file_name.bold()));
    out
}

/// Render the setup block if it contains any (filtered) steps to display.
pub fn render_setup_block(
    setup_results: &[StepResult],
    redaction: &RedactionConfig,
    redacted_values: &[String],
    opts: RenderOptions,
) -> String {
    let has_visible = setup_results
        .iter()
        .any(|s| !(opts.only_failed && s.passed));
    if !has_visible {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(&format!("\n   {}\n", "Setup".dimmed()));
    for step in setup_results {
        if opts.only_failed && step.passed {
            continue;
        }
        render_step_into(&mut out, step, redaction, redacted_values);
    }
    out
}

/// Render a single test group block (header + steps).
pub fn render_test_block(
    test: &TestResult,
    redaction: &RedactionConfig,
    redacted_values: &[String],
    opts: RenderOptions,
) -> String {
    if opts.only_failed && test.passed {
        return String::new();
    }
    let mut out = String::new();
    out.push('\n');
    if let Some(ref desc) = test.description {
        out.push_str(&format!("   {} — {}\n", test.name.bold(), desc.dimmed()));
    } else {
        out.push_str(&format!("   {}\n", test.name.bold()));
    }
    for step in &test.step_results {
        if opts.only_failed && step.passed {
            continue;
        }
        render_step_into(&mut out, step, redaction, redacted_values);
    }
    out
}

/// Render the teardown block if it contains any (filtered) steps to display.
pub fn render_teardown_block(
    teardown_results: &[StepResult],
    redaction: &RedactionConfig,
    redacted_values: &[String],
    opts: RenderOptions,
) -> String {
    let has_visible = teardown_results
        .iter()
        .any(|s| !(opts.only_failed && s.passed));
    if !has_visible {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(&format!("\n   {}\n", "Teardown".dimmed()));
    for step in teardown_results {
        if opts.only_failed && step.passed {
            continue;
        }
        render_step_into(&mut out, step, redaction, redacted_values);
    }
    out
}

fn render_file(output: &mut String, file_result: &FileResult, opts: RenderOptions) {
    output.push_str(&render_file_header(file_result));
    output.push_str(&render_setup_block(
        &file_result.setup_results,
        &file_result.redaction,
        &file_result.redacted_values,
        opts,
    ));
    for test in &file_result.test_results {
        output.push_str(&render_test_block(
            test,
            &file_result.redaction,
            &file_result.redacted_values,
            opts,
        ));
    }
    output.push_str(&render_teardown_block(
        &file_result.teardown_results,
        &file_result.redaction,
        &file_result.redacted_values,
        opts,
    ));
}

/// Render a step's optional `description:` underneath its name line.
/// Each description line is indented so it visually nests under the step
/// glyph and dimmed via the `colored` crate to match how setup/teardown
/// headers and the `└─` connector are rendered elsewhere in this module.
/// Multi-line descriptions (from YAML `|` or `>` scalars) are split so
/// every line gets the same dimmed indent — no raw `\n` tokens leak into
/// the human report.
fn render_step_description_into(output: &mut String, step: &StepResult) {
    if let Some(ref description) = step.description {
        for line in description.lines() {
            output.push_str(&format!("     {}\n", line.dimmed()));
        }
    }
}

fn render_step_into(
    output: &mut String,
    step: &StepResult,
    redaction: &RedactionConfig,
    redacted_values: &[String],
) {
    if step.passed {
        output.push_str(&format!(
            "   {} {} ({}ms)\n",
            "✓".green(),
            step.name,
            step.duration_ms
        ));
        render_step_description_into(output, step);
    } else if matches!(
        step.error_category,
        Some(FailureCategory::SkippedDueToFailedCapture)
            | Some(FailureCategory::SkippedDueToFailFast)
    ) {
        // Skipped-cascade steps use a distinct glyph so operators can
        // tell cascade fallout apart from primary failures at a glance.
        output.push_str(&format!(
            "   {} {} (skipped)\n",
            "⊘".yellow(),
            step.name.yellow(),
        ));
        render_step_description_into(output, step);
        if let Some(reason) = step.assertion_results.iter().find(|a| !a.passed) {
            let reason = sanitize_assertion(reason, redaction, redacted_values);
            output.push_str(&format!(
                "     {} {}\n",
                "└─".dimmed(),
                reason.message.yellow()
            ));
        }
    } else {
        output.push_str(&format!(
            "   {} {} ({}ms)\n",
            "✗".red(),
            step.name.red(),
            step.duration_ms
        ));
        render_step_description_into(output, step);
        // Show failure details
        let failures = step.failures();
        let hints = step_hints(step);
        let failure_count = failures.len();
        let hint_count = hints.len();
        for (i, failure) in failures.iter().enumerate() {
            let failure = sanitize_assertion(failure, redaction, redacted_values);
            // Reserve the closing `└─` for the very last line we emit
            // (the final hint if present, otherwise the final failure).
            let is_last_line = i == failure_count - 1 && hint_count == 0;
            let connector = if is_last_line { "└─" } else { "├─" };
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

        // Emit optional diagnostic hints (e.g. route-ordering) after
        // the raw failure messages. Each hint renders as a dimmed
        // `note:` line so it's visibly separate from the failure
        // itself and does not masquerade as a new assertion failure.
        for (i, hint) in hints.iter().enumerate() {
            let is_last_line = i == hint_count - 1;
            let connector = if is_last_line { "└─" } else { "├─" };
            output.push_str(&format!("     {} {}\n", connector.dimmed(), hint.dimmed()));
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
                        description: None,
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
                        location: None,
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
                    description: None,
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
                test_results: vec![],
                teardown_results: vec![StepResult {
                    name: "Cleanup".into(),
                    description: None,
                    passed: true,
                    duration_ms: 30,
                    assertion_results: vec![],
                    request_info: None,
                    response_info: None,
                    error_category: None,
                    response_status: None,
                    response_summary: None,
                    captures_set: vec![],
                    location: None,
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
                        description: None,
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
                        location: None,
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
                        description: None,
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
                        location: None,
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
    fn route_ordering_hint_rendered_example_snapshot() {
        // Snapshot-style test: captures the exact rendered output
        // (without ANSI color codes) so doc examples can't drift from
        // the real formatter.
        colored::control::set_override(false);

        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/json".into());

        let run = RunResult {
            duration_ms: 12,
            file_results: vec![FileResult {
                file: "orders.tarn.yaml".into(),
                name: "Orders API".into(),
                passed: false,
                duration_ms: 12,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "approve_order".into(),
                    description: None,
                    passed: false,
                    duration_ms: 12,
                    step_results: vec![StepResult {
                        name: "POST /orders/approve".into(),
                        description: None,
                        passed: false,
                        duration_ms: 12,
                        assertion_results: vec![AssertionResult::fail(
                            "status",
                            "201",
                            "400",
                            "Expected HTTP status 201, got 400",
                        )],
                        request_info: Some(RequestInfo {
                            method: "POST".into(),
                            url: "http://api.example.com/orders/approve".into(),
                            headers,
                            body: None,
                            multipart: None,
                        }),
                        response_info: Some(ResponseInfo {
                            status: 400,
                            headers: HashMap::new(),
                            body: Some(serde_json::json!({
                                "statusCode": 400,
                                "message": "Validation failed (uuid is expected)",
                                "error": "Bad Request"
                            })),
                        }),
                        error_category: Some(FailureCategory::AssertionFailed),
                        response_status: Some(400),
                        response_summary: None,
                        captures_set: vec![],
                        location: None,
                    }],
                    captures: HashMap::new(),
                }],
                teardown_results: vec![],
            }],
        };

        let output = render(&run);
        // Intentionally match on exact substrings rather than the
        // full banner so we don't break on cosmetic whitespace.
        assert!(output.contains("✗ POST /orders/approve (12ms)"));
        assert!(output.contains("├─ Expected HTTP status 201, got 400"));
        assert!(output.contains(
            "└─ note: the server may have matched this path to a dynamic route (e.g. /foo/:id); check for route ordering conflicts (see docs/TROUBLESHOOTING.md#route-ordering)."
        ));

        colored::control::unset_override();
    }

    #[test]
    fn render_emits_route_ordering_hint_when_body_signals_it() {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/json".into());

        let run = RunResult {
            duration_ms: 5,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Suite".into(),
                passed: false,
                duration_ms: 5,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "approve_order".into(),
                    description: None,
                    passed: false,
                    duration_ms: 5,
                    step_results: vec![StepResult {
                        name: "POST /orders/approve".into(),
                        description: None,
                        passed: false,
                        duration_ms: 5,
                        assertion_results: vec![AssertionResult::fail(
                            "status",
                            "201",
                            "400",
                            "Expected HTTP status 201, got 400",
                        )],
                        request_info: Some(RequestInfo {
                            method: "POST".into(),
                            url: "http://api.example.com/orders/approve".into(),
                            headers,
                            body: None,
                            multipart: None,
                        }),
                        response_info: Some(ResponseInfo {
                            status: 400,
                            headers: HashMap::new(),
                            body: Some(serde_json::json!({
                                "statusCode": 400,
                                "message": "Validation failed (uuid is expected)",
                                "error": "Bad Request"
                            })),
                        }),
                        error_category: Some(FailureCategory::AssertionFailed),
                        response_status: Some(400),
                        response_summary: None,
                        captures_set: vec![],
                        location: None,
                    }],
                    captures: HashMap::new(),
                }],
                teardown_results: vec![],
            }],
        };

        let output = render(&run);
        assert!(
            output.contains("route ordering"),
            "expected route-ordering hint in output, got:\n{}",
            output
        );
        assert!(output.contains("docs/TROUBLESHOOTING.md#route-ordering"));
    }

    #[test]
    fn render_does_not_emit_route_ordering_hint_without_signal() {
        let run = RunResult {
            duration_ms: 5,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Suite".into(),
                passed: false,
                duration_ms: 5,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "approve_order".into(),
                    description: None,
                    passed: false,
                    duration_ms: 5,
                    step_results: vec![StepResult {
                        name: "POST /orders/approve".into(),
                        description: None,
                        passed: false,
                        duration_ms: 5,
                        assertion_results: vec![AssertionResult::fail(
                            "status",
                            "201",
                            "400",
                            "Expected HTTP status 201, got 400",
                        )],
                        request_info: Some(RequestInfo {
                            method: "POST".into(),
                            url: "http://api.example.com/orders/approve".into(),
                            headers: HashMap::new(),
                            body: None,
                            multipart: None,
                        }),
                        response_info: Some(ResponseInfo {
                            status: 400,
                            headers: HashMap::new(),
                            body: Some(serde_json::json!({"message": "Insufficient funds"})),
                        }),
                        error_category: Some(FailureCategory::AssertionFailed),
                        response_status: Some(400),
                        response_summary: None,
                        captures_set: vec![],
                        location: None,
                    }],
                    captures: HashMap::new(),
                }],
                teardown_results: vec![],
            }],
        };

        let output = render(&run);
        assert!(!output.contains("route ordering"));
        assert!(!output.contains("docs/TROUBLESHOOTING.md#route-ordering"));
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
                        description: None,
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
                        location: None,
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

    // --- Step-level descriptions in human report (NAZ-243) ---

    /// Strip the ANSI SGR escape sequences the `colored` crate emits so
    /// assertions can compare raw text regardless of whether another
    /// parallel test flipped the global color override. Only matches the
    /// `\x1b[...m` sequences `colored` produces — the `.dimmed()`,
    /// `.green()`, `.red()`, and `.yellow()` wrappers used in this
    /// module — so we do not pull in an extra dependency just for tests.
    fn strip_ansi(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut chars = input.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // Consume the `[...m` CSI sequence.
                if chars.next() != Some('[') {
                    continue;
                }
                for ch in chars.by_ref() {
                    if ch == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Build a run result whose sole step carries the given pass-state
    /// and optional description so each human-render test touches exactly
    /// one variable. Tests pair this with `strip_ansi` on the render
    /// output so assertions are stable even when another parallel test
    /// is flipping the `colored` crate's global override.
    fn run_with_single_step(passed: bool, description: Option<&str>) -> RunResult {
        RunResult {
            duration_ms: 10,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Suite".into(),
                passed,
                duration_ms: 10,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "group".into(),
                    description: None,
                    passed,
                    duration_ms: 10,
                    step_results: vec![StepResult {
                        name: "Step name".into(),
                        description: description.map(str::to_string),
                        passed,
                        duration_ms: 5,
                        assertion_results: if passed {
                            vec![AssertionResult::pass("status", "200", "200")]
                        } else {
                            vec![AssertionResult::fail(
                                "status",
                                "200",
                                "500",
                                "status mismatch",
                            )]
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
                }],
                teardown_results: vec![],
            }],
        }
    }

    #[test]
    fn human_renders_step_description_for_passing_step() {
        // After stripping ANSI escapes, the description text must appear
        // in the output underneath the step name.
        let result = run_with_single_step(true, Some("Checks /health"));
        let output = strip_ansi(&render(&result));
        assert!(
            output.contains("Step name"),
            "step name must still render, got: {}",
            output
        );
        assert!(
            output.contains("Checks /health"),
            "description must render beneath the step, got: {}",
            output
        );
        // Description should appear AFTER the step name line, not before.
        let name_pos = output.find("Step name").unwrap();
        let desc_pos = output.find("Checks /health").unwrap();
        assert!(
            desc_pos > name_pos,
            "description must render below the step name, got name@{} desc@{}",
            name_pos,
            desc_pos
        );
    }

    #[test]
    fn human_renders_step_description_for_failing_step() {
        // Failures must still surface the description so operators see
        // the author's intent alongside the mismatch.
        let result = run_with_single_step(false, Some("Checks /health"));
        let output = strip_ansi(&render(&result));
        assert!(output.contains("Checks /health"));
        assert!(output.contains("status mismatch"));
    }

    #[test]
    fn human_omits_step_description_line_when_missing() {
        // No description must mean no "Checks /health" line leaks in —
        // the guard ensures the helper is a true no-op when the field is
        // absent rather than emitting an empty indented row.
        let result = run_with_single_step(true, None);
        let output = strip_ansi(&render(&result));
        assert!(output.contains("Step name"));
        assert!(!output.contains("Checks /health"));
    }

    #[test]
    fn human_renders_multi_line_step_description_indented() {
        // Multi-line descriptions (from YAML `|` scalars) must keep each
        // line on its own indented row so the report stays readable.
        let result = run_with_single_step(true, Some("First line\nSecond line"));
        let output = strip_ansi(&render(&result));
        assert!(output.contains("First line"));
        assert!(output.contains("Second line"));
        // Each rendered line should carry the same five-space indent used
        // by the description block so the two lines align under the step.
        assert!(output.contains("     First line"));
        assert!(output.contains("     Second line"));
    }
}
