//! Compact one-file-per-line output format (NAZ-240).
//!
//! The compact format is a middle ground between human and llm: it shows
//! a one-line header for the run, a single line per file with a
//! pass/fail badge and aggregate counts, then expands failures inline
//! with the first failing assertion plus a `METHOD URL -> status`
//! breadcrumb. With `-v` the expanded block also shows captured values
//! for each test. With `--only-failed` every passing file is elided.
//!
//! Colors are honored when the caller did not request `no_color`. The
//! `group_failures` helper powers the trailing `HTTP 500: 3 | JSONPath
//! mismatch: 18`-style tally so operators can see failure shape at a
//! glance without scrolling the whole block.

use crate::assert::types::{FileResult, RunResult, TestResult};
use crate::report::failure::{
    group_failures, primary_failure_message, request_arrow_response, skip_cascade_summary,
    truncate_string, CAPTURE_VALUE_CAP, COMPACT_MESSAGE_CAP,
};
use crate::report::RenderOptions;
use colored::Colorize;

/// Render with default options (no filtering, no verbose captures).
pub fn render(result: &RunResult) -> String {
    render_with_options(result, RenderOptions::default())
}

/// Render respecting the caller's options. `only_failed` hides fully
/// passing files; `verbose` surfaces captured values per test; `no_color`
/// strips every ANSI escape.
pub fn render_with_options(result: &RunResult, opts: RenderOptions) -> String {
    // Centralize the "paint or not?" decision. `colored` normally makes
    // this per-call, but we want every code path in this module to
    // respect the caller's `no_color` preference uniformly.
    if opts.no_color {
        colored::control::set_override(false);
    } else {
        colored::control::unset_override();
    }

    let mut out = String::new();
    out.push_str(&render_header(result));
    out.push('\n');

    for file in &result.file_results {
        if opts.only_failed && file.passed {
            continue;
        }
        render_file(&mut out, file, opts);
    }

    let groups = group_failures(result);
    if !groups.is_empty() {
        out.push('\n');
        out.push_str(&render_group_summary(&groups));
        out.push('\n');
    }

    // Always unset the override on exit so unrelated rendering (e.g.
    // the streaming human progress reporter in the same process) keeps
    // its own color policy.
    colored::control::unset_override();
    out
}

fn render_header(result: &RunResult) -> String {
    let files = result.total_files();
    let tests: usize = result
        .file_results
        .iter()
        .map(|f| f.test_results.len())
        .sum();
    let steps_total = result.total_steps();
    let steps_passed = result.passed_steps();
    let seconds = duration_seconds(result.duration_ms);
    format!(
        "tarn: {} file{}, {} test{}, {}/{} steps passed, {}s",
        files,
        plural(files),
        tests,
        plural(tests),
        steps_passed,
        steps_total,
        seconds
    )
}

fn render_file(out: &mut String, file: &FileResult, opts: RenderOptions) {
    let total = file.total_steps();
    let passed = file.passed_steps();
    if file.passed {
        out.push_str(&format!(
            "{} {}  ({}/{})\n",
            "✓".green(),
            file.file,
            passed,
            total
        ));
    } else {
        out.push_str(&format!(
            "{} {}  ({}/{})\n",
            "✗".red(),
            file.file.bold(),
            passed,
            total
        ));
        render_file_failures(out, file, opts);
    }
}

fn render_file_failures(out: &mut String, file: &FileResult, opts: RenderOptions) {
    // Setup failures use `<setup>` as the test label so readers can
    // tell the phase apart from a real test.
    for step in &file.setup_results {
        if step.passed {
            continue;
        }
        let message = primary_failure_message(
            step,
            &file.redaction,
            &file.redacted_values,
            COMPACT_MESSAGE_CAP,
        );
        out.push_str(&format!(
            "  {} <setup> — {} — {}\n",
            "FAIL:".red().bold(),
            step.name,
            message
        ));
        out.push_str(&format!(
            "    {}\n",
            request_arrow_response(step, &file.redaction, &file.redacted_values)
                .dimmed()
        ));
    }

    for test in &file.test_results {
        if test.passed && !opts.verbose {
            continue;
        }
        render_test_failures(out, file, test, opts);
    }

    for step in &file.teardown_results {
        if step.passed {
            continue;
        }
        let message = primary_failure_message(
            step,
            &file.redaction,
            &file.redacted_values,
            COMPACT_MESSAGE_CAP,
        );
        out.push_str(&format!(
            "  {} <teardown> — {} — {}\n",
            "FAIL:".red().bold(),
            step.name,
            message
        ));
        out.push_str(&format!(
            "    {}\n",
            request_arrow_response(step, &file.redaction, &file.redacted_values)
                .dimmed()
        ));
    }
}

fn render_test_failures(
    out: &mut String,
    file: &FileResult,
    test: &TestResult,
    opts: RenderOptions,
) {
    let mut printed_anything = false;

    for step in &test.step_results {
        if step.passed {
            continue;
        }
        let message = primary_failure_message(
            step,
            &file.redaction,
            &file.redacted_values,
            COMPACT_MESSAGE_CAP,
        );
        out.push_str(&format!(
            "  {} {} — {} — {}\n",
            "FAIL:".red().bold(),
            test.name,
            step.name,
            message
        ));
        out.push_str(&format!(
            "    {}\n",
            request_arrow_response(step, &file.redaction, &file.redacted_values)
                .dimmed()
        ));
        printed_anything = true;
    }

    // Cascade summary: one line per failed-capture dependency group so
    // readers see how much fallout was suppressed.
    for (capture, count) in skip_cascade_summary(test) {
        out.push_str(&format!(
            "    {} {} step{} (depended on failed capture '{}')\n",
            "skipped:".yellow(),
            count,
            plural(count),
            capture
        ));
        printed_anything = true;
    }

    // Verbose: render every captured value (keys are sorted for
    // deterministic output) with a short preview.
    if opts.verbose && !test.captures.is_empty() {
        // Ensure the verbose block is scoped to a test that produced
        // visible content above — we don't want captures on a silent
        // pass to create orphan output unless the caller opts in.
        if printed_anything || test.passed {
            out.push_str(&format!("    {} ({})\n", "captures".dimmed(), test.name));
            let mut keys: Vec<&String> = test.captures.keys().collect();
            keys.sort();
            for key in keys {
                let rendered =
                    serde_json::to_string(&test.captures[key]).unwrap_or_else(|_| "null".into());
                out.push_str(&format!(
                    "      {} = {}\n",
                    key,
                    truncate_string(&rendered, CAPTURE_VALUE_CAP)
                ));
            }
        }
    }
}

fn render_group_summary(groups: &[(String, usize)]) -> String {
    groups
        .iter()
        .map(|(label, count)| format!("{}: {}", label, count))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn duration_seconds(ms: u64) -> String {
    let seconds = ms as f64 / 1000.0;
    if seconds >= 10.0 {
        format!("{:.0}", seconds)
    } else {
        format!("{:.1}", seconds)
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::*;
    use crate::model::RedactionConfig;
    use serde_json::json;
    use std::collections::HashMap;

    fn strip_ansi(s: &str) -> String {
        // `colored` writes ANSI escape sequences; strip them with a
        // minimal regex so assertions stay legible.
        let re = regex::Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap();
        re.replace_all(s, "").into_owned()
    }

    fn base_passed_step(name: &str) -> StepResult {
        StepResult {
            name: name.into(),
            description: None,
            passed: true,
            duration_ms: 20,
            assertion_results: vec![AssertionResult::pass("status", "200", "200")],
            request_info: None,
            response_info: None,
            error_category: None,
            response_status: Some(200),
            response_summary: None,
            captures_set: vec![],
            location: None,
        }
    }

    fn failing_step(name: &str, status: u16) -> StepResult {
        StepResult {
            name: name.into(),
            description: None,
            passed: false,
            duration_ms: 30,
            assertion_results: vec![AssertionResult::fail(
                "status",
                "200",
                status.to_string(),
                format!("Expected 200, got {}", status),
            )],
            request_info: Some(RequestInfo {
                method: "GET".into(),
                url: "/foo".into(),
                headers: HashMap::new(),
                body: None,
                multipart: None,
            }),
            response_info: Some(ResponseInfo {
                status,
                headers: HashMap::new(),
                body: Some(json!({"err": "boom"})),
            }),
            error_category: Some(FailureCategory::AssertionFailed),
            response_status: Some(status),
            response_summary: None,
            captures_set: vec![],
            location: None,
        }
    }

    fn build_run(files: Vec<FileResult>) -> RunResult {
        let duration_ms = files.iter().map(|f| f.duration_ms).sum();
        RunResult {
            file_results: files,
            duration_ms,
        }
    }

    fn file_with(name: &str, passed: bool, steps: Vec<StepResult>) -> FileResult {
        let total_steps = steps.len();
        let passed_steps = steps.iter().filter(|s| s.passed).count();
        let _ = total_steps;
        let _ = passed_steps;
        FileResult {
            file: name.into(),
            name: name.into(),
            passed,
            duration_ms: 10,
            redaction: RedactionConfig::default(),
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![TestResult {
                name: "t".into(),
                description: None,
                passed,
                duration_ms: 10,
                step_results: steps,
                captures: HashMap::new(),
            }],
            teardown_results: vec![],
        }
    }

    #[test]
    fn header_has_counts_and_duration() {
        let run = build_run(vec![file_with(
            "a.tarn.yaml",
            true,
            vec![base_passed_step("s1"), base_passed_step("s2")],
        )]);
        let out = strip_ansi(&render_with_options(
            &run,
            RenderOptions {
                no_color: true,
                ..RenderOptions::default()
            },
        ));
        assert!(out.starts_with("tarn: 1 file, 1 test, 2/2 steps passed,"));
    }

    #[test]
    fn passing_file_renders_checkmark_line() {
        let run = build_run(vec![file_with(
            "a.tarn.yaml",
            true,
            vec![base_passed_step("s1")],
        )]);
        let out = strip_ansi(&render_with_options(
            &run,
            RenderOptions {
                no_color: true,
                ..RenderOptions::default()
            },
        ));
        assert!(out.contains("\u{2713} a.tarn.yaml  (1/1)"));
    }

    #[test]
    fn only_failed_hides_passing_files() {
        let run = build_run(vec![
            file_with("a.tarn.yaml", true, vec![base_passed_step("s1")]),
            file_with("b.tarn.yaml", false, vec![failing_step("bad", 500)]),
        ]);
        let out = strip_ansi(&render_with_options(
            &run,
            RenderOptions {
                only_failed: true,
                no_color: true,
                ..RenderOptions::default()
            },
        ));
        assert!(!out.contains("a.tarn.yaml"));
        assert!(out.contains("b.tarn.yaml"));
    }

    #[test]
    fn failed_file_shows_fail_line_and_arrow() {
        let run = build_run(vec![file_with(
            "b.tarn.yaml",
            false,
            vec![failing_step("bad", 500)],
        )]);
        let out = strip_ansi(&render_with_options(
            &run,
            RenderOptions {
                no_color: true,
                ..RenderOptions::default()
            },
        ));
        assert!(out.contains("\u{2717} b.tarn.yaml  (0/1)"));
        assert!(out.contains("FAIL: t \u{2014} bad \u{2014} Expected 200, got 500"));
        assert!(out.contains("GET /foo -> 500"));
    }

    #[test]
    fn group_summary_lists_categories_by_count() {
        let run = build_run(vec![file_with(
            "b.tarn.yaml",
            false,
            vec![
                failing_step("a", 500),
                failing_step("b", 500),
                failing_step("c", 404),
            ],
        )]);
        let out = strip_ansi(&render_with_options(
            &run,
            RenderOptions {
                no_color: true,
                ..RenderOptions::default()
            },
        ));
        assert!(out.contains("HTTP 500: 2 | HTTP 404: 1"));
    }

    #[test]
    fn verbose_shows_captures_for_test() {
        let mut tr = TestResult {
            name: "cap_test".into(),
            description: None,
            passed: false,
            duration_ms: 10,
            step_results: vec![failing_step("login", 500)],
            captures: HashMap::new(),
        };
        tr.captures.insert("token".into(), json!("abc"));
        tr.captures.insert("user_id".into(), json!(42));
        let file = FileResult {
            file: "b.tarn.yaml".into(),
            name: "b".into(),
            passed: false,
            duration_ms: 10,
            redaction: RedactionConfig::default(),
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![tr],
            teardown_results: vec![],
        };
        let run = build_run(vec![file]);
        let out = strip_ansi(&render_with_options(
            &run,
            RenderOptions {
                verbose: true,
                no_color: true,
                ..RenderOptions::default()
            },
        ));
        assert!(out.contains("captures (cap_test)"));
        assert!(out.contains("token = \"abc\""));
        assert!(out.contains("user_id = 42"));
    }

    #[test]
    fn long_capture_value_is_truncated() {
        let mut tr = TestResult {
            name: "cap".into(),
            description: None,
            passed: false,
            duration_ms: 10,
            step_results: vec![failing_step("login", 500)],
            captures: HashMap::new(),
        };
        tr.captures
            .insert("payload".into(), json!("a".repeat(200)));
        let file = FileResult {
            file: "b.tarn.yaml".into(),
            name: "b".into(),
            passed: false,
            duration_ms: 10,
            redaction: RedactionConfig::default(),
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![tr],
            teardown_results: vec![],
        };
        let run = build_run(vec![file]);
        let out = strip_ansi(&render_with_options(
            &run,
            RenderOptions {
                verbose: true,
                no_color: true,
                ..RenderOptions::default()
            },
        ));
        assert!(out.contains("payload = "));
        assert!(out.contains("..."));
        // The line should not contain the full 200 'a' string uncut.
        assert!(!out.contains(&"a".repeat(150)));
    }
}
