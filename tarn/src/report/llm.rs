//! LLM-friendly output format (NAZ-349).
//!
//! Designed to be piped into an AI assistant or into `grep`:
//!
//! * A single grep-friendly first line (`tarn: PASS 498/500 steps, ...`).
//! * No per-file running output, no boxed headers, no colors when
//!   stdout isn't a TTY (the caller passes `no_color: true` in that
//!   case).
//! * Only failures are expanded; each block is short and stable.
//!
//! The format shares helpers with the compact format via
//! `super::failure`, so any future change to request/response/summary
//! shape lands in both at once.

use crate::assert::types::{FailureCategory, FileResult, RunResult, StepResult};
use crate::model::RedactionConfig;
use crate::report::failure::{
    format_assertion_line, for_each_step, group_failures, request_line, response_body_preview,
    skip_cascade_summary, LLM_BODY_PREVIEW_CHARS,
};
use crate::report::redaction::sanitize_assertion;
use crate::report::RenderOptions;

/// Render with default options (no filtering, no color override).
pub fn render(result: &RunResult) -> String {
    render_with_options(result, RenderOptions::default())
}

/// Render respecting the caller's options. `only_failed` drops passing
/// files from the grep summary line (they are already implicit in the
/// pass/fail count); LLM format never expands passing steps so the
/// other knobs are mostly inherited from `compact`.
pub fn render_with_options(result: &RunResult, _opts: RenderOptions) -> String {
    let mut out = String::new();
    out.push_str(&render_summary_line(result));
    out.push('\n');

    // Stable file ordering. Source order within a file is already
    // preserved by the runner, but we sort files alphabetically so two
    // runs with parallel file scheduling still emit the same text.
    let mut files: Vec<&FileResult> = result.file_results.iter().collect();
    files.sort_by(|a, b| a.file.cmp(&b.file));

    for file in files {
        render_file_failures(&mut out, file);
    }

    let groups = group_failures(result);
    if !groups.is_empty() {
        out.push('\n');
        out.push_str("failure summary:\n");
        for (label, count) in &groups {
            out.push_str(&format!("  {}: {}\n", label, count));
        }
    }
    out
}

/// Render the one-line grep-friendly summary. The exact format is a
/// stable contract with LLM consumers — do not change the prefix,
/// separators, or ordering without bumping the documentation.
pub fn render_summary_line(result: &RunResult) -> String {
    let passed = result.passed_steps();
    let total = result.total_steps();
    let failed = result.failed_steps();
    let files = result.total_files();
    let seconds = format!("{:.1}", result.duration_ms as f64 / 1000.0);
    let status = if result.passed() { "PASS" } else { "FAIL" };
    format!(
        "tarn: {} {}/{} steps, {} failed, {} file{}, {}s",
        status,
        passed,
        total,
        failed,
        files,
        if files == 1 { "" } else { "s" },
        seconds
    )
}

fn render_file_failures(out: &mut String, file: &FileResult) {
    // Walk every step in source order so the output is deterministic.
    for_each_step(file, &mut |test, is_teardown, step| {
        if step.passed {
            return;
        }
        let phase = if test.is_some() {
            None
        } else if is_teardown {
            Some("teardown")
        } else {
            Some("setup")
        };
        render_failure_block(out, file, test.map(|t| t.name.as_str()), phase, step);
    });

    // Cascade summaries are emitted per-test once so readers can see how
    // many steps were suppressed without scrolling through every one.
    for test in &file.test_results {
        for (capture, count) in skip_cascade_summary(test) {
            out.push_str(&format!(
                "skipped: {} step{} (depended on failed capture '{}') in {}::{}\n",
                count,
                if count == 1 { "" } else { "s" },
                capture,
                file.file,
                test.name
            ));
        }
    }
}

fn render_failure_block(
    out: &mut String,
    file: &FileResult,
    test_name: Option<&str>,
    phase: Option<&str>,
    step: &StepResult,
) {
    let label = match (test_name, phase) {
        (Some(name), _) => format!("{}::{}::{}", file.file, name, step.name),
        (None, Some(phase)) => format!("{}::<{}>::{}", file.file, phase, step.name),
        (None, None) => format!("{}::{}", file.file, step.name),
    };

    // Cascade skips use a distinct tag so downstream tools can filter
    // fallout from primary failures without parsing the message body.
    let tag = if matches!(
        step.error_category,
        Some(FailureCategory::SkippedDueToFailedCapture)
            | Some(FailureCategory::SkippedDueToFailFast)
    ) {
        "SKIP"
    } else {
        "FAIL"
    };

    out.push_str(&format!("{} {}\n", tag, label));
    render_request(out, step, &file.redaction, &file.redacted_values);
    render_response(out, step, &file.redaction, &file.redacted_values);
    render_asserts(out, step, &file.redaction, &file.redacted_values);
    render_captures(out, step, &file.redaction, &file.redacted_values);
    out.push('\n');
}

fn render_request(
    out: &mut String,
    step: &StepResult,
    redaction: &RedactionConfig,
    secrets: &[String],
) {
    if step.request_info.is_none() {
        return;
    }
    out.push_str(&format!(
        "  request:  {}\n",
        request_line(step, redaction, secrets)
    ));
}

fn render_response(
    out: &mut String,
    step: &StepResult,
    redaction: &RedactionConfig,
    secrets: &[String],
) {
    let Some(resp) = step.response_info.as_ref() else {
        if let Some(status) = step.response_status {
            out.push_str(&format!("  response: {}\n", status));
        }
        return;
    };
    out.push_str(&format!("  response: {}\n", resp.status));
    if let Some(preview) = response_body_preview(step, redaction, secrets, LLM_BODY_PREVIEW_CHARS) {
        out.push_str(&format!("    {}\n", preview));
    }
}

fn render_asserts(
    out: &mut String,
    step: &StepResult,
    redaction: &RedactionConfig,
    secrets: &[String],
) {
    for assertion in step.assertion_results.iter().filter(|a| !a.passed) {
        // Cascade/skip steps manufacture a single informational
        // assertion; surface the message verbatim instead of shoehorning
        // it into the "expected/got" shape.
        if matches!(
            step.error_category,
            Some(FailureCategory::SkippedDueToFailedCapture)
                | Some(FailureCategory::SkippedDueToFailFast)
        ) {
            let sanitized = sanitize_assertion(assertion, redaction, secrets);
            out.push_str(&format!("  reason:   {}\n", sanitized.message));
            continue;
        }
        out.push_str(&format!(
            "  assert:   {}\n",
            format_assertion_line(assertion, redaction, secrets)
        ));
    }
}

fn render_captures(
    out: &mut String,
    step: &StepResult,
    _redaction: &RedactionConfig,
    _secrets: &[String],
) {
    if step.captures_set.is_empty() {
        return;
    }
    let mut names = step.captures_set.clone();
    names.sort();
    out.push_str(&format!("  captures: {}\n", names.join(", ")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::*;
    use crate::model::RedactionConfig;
    use serde_json::json;
    use std::collections::HashMap;

    fn failing_run() -> RunResult {
        RunResult {
            duration_ms: 16300,
            file_results: vec![FileResult {
                file: "tests/users.tarn.yaml".into(),
                name: "users".into(),
                passed: false,
                duration_ms: 16300,
                redaction: RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "create_user".into(),
                    description: None,
                    passed: false,
                    duration_ms: 16300,
                    step_results: vec![StepResult {
                        name: "Create".into(),
                        description: None,
                        passed: false,
                        duration_ms: 100,
                        assertion_results: vec![AssertionResult::fail(
                            "status",
                            "200",
                            "500",
                            "Expected 200, got 500",
                        )],
                        request_info: Some(RequestInfo {
                            method: "POST".into(),
                            url: "/v1/api/users".into(),
                            headers: HashMap::from([(
                                "Authorization".into(),
                                "Bearer secret".into(),
                            )]),
                            body: None,
                            multipart: None,
                        }),
                        response_info: Some(ResponseInfo {
                            status: 500,
                            headers: HashMap::new(),
                            body: Some(json!({"message": "boom"})),
                        }),
                        error_category: Some(FailureCategory::AssertionFailed),
                        response_status: Some(500),
                        response_summary: None,
                        captures_set: vec!["id".into()],
                        location: None,
                    }],
                    captures: HashMap::new(),
                }],
                teardown_results: vec![],
            }],
        }
    }

    #[test]
    fn first_line_is_grep_friendly() {
        let out = render(&failing_run());
        let first = out.lines().next().unwrap();
        assert!(first.starts_with("tarn: FAIL 0/1 steps, 1 failed, 1 file,"));
    }

    #[test]
    fn passing_run_reports_pass_status() {
        let run = RunResult {
            duration_ms: 120,
            file_results: vec![FileResult {
                file: "a.tarn.yaml".into(),
                name: "a".into(),
                passed: true,
                duration_ms: 120,
                redaction: RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "t".into(),
                    description: None,
                    passed: true,
                    duration_ms: 120,
                    step_results: vec![StepResult {
                        name: "ok".into(),
                        description: None,
                        passed: true,
                        duration_ms: 120,
                        assertion_results: vec![AssertionResult::pass("status", "200", "200")],
                        request_info: None,
                        response_info: None,
                        error_category: None,
                        response_status: Some(200),
                        response_summary: None,
                        captures_set: vec![],
                        location: None,
                    }],
                    captures: HashMap::new(),
                }],
                teardown_results: vec![],
            }],
        };
        let out = render(&run);
        assert!(out.lines().next().unwrap().starts_with("tarn: PASS 1/1"));
        assert!(!out.contains("FAIL "));
    }

    #[test]
    fn failure_block_includes_request_response_assert() {
        let out = render(&failing_run());
        assert!(out.contains("FAIL tests/users.tarn.yaml::create_user::Create"));
        assert!(out.contains("request:  POST /v1/api/users (Authorization: ***)"));
        assert!(out.contains("response: 500"));
        assert!(out.contains("\"message\":\"boom\""));
        assert!(out.contains("assert:   status: expected 200, got 500"));
    }

    #[test]
    fn failure_summary_appears_at_end() {
        let out = render(&failing_run());
        assert!(out.contains("failure summary:"));
        assert!(out.contains("HTTP 500: 1"));
    }

    #[test]
    fn files_render_in_sorted_order() {
        let mut run = failing_run();
        run.file_results.push(FileResult {
            file: "tests/aaa.tarn.yaml".into(),
            name: "aaa".into(),
            passed: false,
            duration_ms: 10,
            redaction: RedactionConfig::default(),
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![TestResult {
                name: "t".into(),
                description: None,
                passed: false,
                duration_ms: 10,
                step_results: vec![StepResult {
                    name: "s".into(),
                    description: None,
                    passed: false,
                    duration_ms: 10,
                    assertion_results: vec![AssertionResult::fail(
                        "status",
                        "200",
                        "404",
                        "Expected 200, got 404",
                    )],
                    request_info: Some(RequestInfo {
                        method: "GET".into(),
                        url: "/x".into(),
                        headers: HashMap::new(),
                        body: None,
                        multipart: None,
                    }),
                    response_info: None,
                    error_category: Some(FailureCategory::AssertionFailed),
                    response_status: Some(404),
                    response_summary: None,
                    captures_set: vec![],
                    location: None,
                }],
                captures: HashMap::new(),
            }],
            teardown_results: vec![],
        });
        let out = render(&run);
        let pos_aaa = out.find("tests/aaa.tarn.yaml").unwrap();
        let pos_users = out.find("tests/users.tarn.yaml").unwrap();
        assert!(
            pos_aaa < pos_users,
            "alphabetical ordering should render aaa before users"
        );
    }

    #[test]
    fn cascade_skip_emits_single_line_per_capture() {
        let mut run = failing_run();
        run.file_results[0].test_results[0]
            .step_results
            .push(StepResult {
                name: "step_dep_a".into(),
                description: None,
                passed: false,
                duration_ms: 0,
                assertion_results: vec![AssertionResult::fail(
                    "runtime",
                    "ok",
                    "skipped",
                    "step skipped: depends on capture 'id' which failed",
                )],
                request_info: None,
                response_info: None,
                error_category: Some(FailureCategory::SkippedDueToFailedCapture),
                response_status: None,
                response_summary: None,
                captures_set: vec![],
                location: None,
            });
        run.file_results[0].test_results[0]
            .step_results
            .push(StepResult {
                name: "step_dep_b".into(),
                description: None,
                passed: false,
                duration_ms: 0,
                assertion_results: vec![AssertionResult::fail(
                    "runtime",
                    "ok",
                    "skipped",
                    "step skipped: depends on capture 'id' which failed",
                )],
                request_info: None,
                response_info: None,
                error_category: Some(FailureCategory::SkippedDueToFailedCapture),
                response_status: None,
                response_summary: None,
                captures_set: vec![],
                location: None,
            });
        let out = render(&run);
        let expected =
            "skipped: 2 steps (depended on failed capture 'id') in \
             tests/users.tarn.yaml::create_user";
        assert!(out.contains(expected));
    }

    #[test]
    fn passing_run_has_no_failure_summary() {
        let run = RunResult {
            duration_ms: 10,
            file_results: vec![FileResult {
                file: "a.tarn.yaml".into(),
                name: "a".into(),
                passed: true,
                duration_ms: 10,
                redaction: RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![],
                teardown_results: vec![],
            }],
        };
        let out = render(&run);
        assert!(!out.contains("failure summary:"));
    }
}
