//! Concise, grep-friendly renderer for persisted run artifacts (NAZ-404).
//!
//! The tarn CLI already has "compact" and "llm" formats, but both
//! render from an in-memory `RunResult` — they require re-running the
//! tests or parsing a full `report.json`. NAZ-401 added the much
//! smaller `summary.json` + `failures.json` artifacts, and NAZ-400
//! archives every run under `.tarn/runs/<run_id>/`. This module is the
//! missing piece: a pure renderer that takes `(SummaryDoc,
//! FailuresDoc, run_id)` and produces either a human string or a
//! machine-readable JSON document. The CLI wires it into `tarn
//! report [--run <id>] [--format concise|json]`, which lets users
//! re-render a compact view of any prior run without re-running it.
//!
//! Root-cause grouping is delegated to
//! [`crate::report::failures_command`] — cascade fallout collapses
//! into `└─ cascades: N skipped` under the primary group rather than
//! inflating the group count, and fingerprints match what `tarn
//! failures` prints so both commands agree.
//!
//! # JSON schema
//!
//! `render_json` produces the following stable shape:
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "run_id": "…",
//!   "exit_code": 1,
//!   "duration_ms": 1234,
//!   "totals":  { "files": N, "tests": N, "steps": N },
//!   "failed":  { "files": N, "tests": N, "steps": N },
//!   "groups": [
//!     {
//!       "fingerprint": "status:200:500:GET:/api/users/:id",
//!       "occurrences": 1,
//!       "cascades":    0,
//!       "primary": {
//!         "file": "…", "test": "…", "step": "…",
//!         "category": "assertion_failed",
//!         "method": "GET", "url": "…", "status": 500,
//!         "message": "first line of the failure message"
//!       }
//!     }
//!   ],
//!   "groups_truncated": false,
//!   "groups_total": 1
//! }
//! ```
//!
//! Keys are stable across runs. New keys may be appended; existing
//! keys will not change shape without a schema_version bump.

use crate::assert::types::FailureCategory;
use crate::report::failures_command::{build_report, FailureGroup};
use crate::report::summary::{FailuresDoc, SummaryDoc};
use serde_json::{json, Value};

/// Bumped on incompatible changes to the concise JSON envelope.
pub const CONCISE_SCHEMA_VERSION: u32 = 1;

/// Maximum number of groups shown in the concise human output before
/// a `…and N more groups` tail line is appended. Chosen to keep the
/// whole view inside the ~20-row terminal budget even when the header
/// and per-group lines are expanded.
pub const MAX_HUMAN_GROUPS: usize = 10;

/// Render the concise human view. `no_color` strips ANSI escapes so
/// the same renderer can drive both TTY and non-TTY output.
pub fn render_concise(
    summary: &SummaryDoc,
    failures: &FailuresDoc,
    run_id: &str,
    no_color: bool,
) -> String {
    let report = build_report(failures, ".tarn/failures.json");
    let mut out = String::new();
    out.push_str(&header_line(summary, run_id, no_color));
    out.push('\n');

    if report.total_failures == 0 {
        // Passing runs intentionally omit the "failures:" section so
        // the common-case output is a single line — exactly what the
        // ticket wants as a replacement for `parse-results.py`.
        return out;
    }

    out.push('\n');
    out.push_str("failures:\n");
    let shown = report.groups.iter().take(MAX_HUMAN_GROUPS);
    for group in shown {
        out.push_str(&render_group_block(group, no_color));
    }
    if report.groups.len() > MAX_HUMAN_GROUPS {
        let extra = report.groups.len() - MAX_HUMAN_GROUPS;
        out.push_str(&format!(
            "…and {} more group{} (run `tarn failures` for full list)\n",
            extra,
            if extra == 1 { "" } else { "s" }
        ));
    }
    out
}

/// Render the concise JSON envelope. The shape is documented at the
/// module level; agents can rely on the listed keys and types.
pub fn render_json(summary: &SummaryDoc, failures: &FailuresDoc, run_id: &str) -> Value {
    let report = build_report(failures, ".tarn/failures.json");
    let groups_total = report.groups.len();
    let truncated = groups_total > MAX_HUMAN_GROUPS;
    let groups: Vec<Value> = report
        .groups
        .iter()
        .take(MAX_HUMAN_GROUPS)
        .map(group_to_json)
        .collect();

    json!({
        "schema_version": CONCISE_SCHEMA_VERSION,
        "run_id": run_id,
        "exit_code": summary.exit_code,
        "duration_ms": summary.duration_ms,
        "totals": {
            "files": summary.totals.files,
            "tests": summary.totals.tests,
            "steps": summary.totals.steps,
        },
        "failed": {
            "files": summary.failed.files,
            "tests": summary.failed.tests,
            "steps": summary.failed.steps,
        },
        "groups": groups,
        "groups_truncated": truncated,
        "groups_total": groups_total,
    })
}

/// Header line: `Run <run_id>   exit N   passed A/B   failed C/B   <duration>`.
/// Uses `failed.steps` + `totals.steps` for pass/fail ratios — they are
/// the most actionable units for a step-oriented tool.
fn header_line(summary: &SummaryDoc, run_id: &str, no_color: bool) -> String {
    let total = summary.totals.steps;
    let failed = summary.failed.steps;
    // Saturating subtraction: an inconsistent artifact with failed >
    // total should not panic, just cap at zero.
    let passed = total.saturating_sub(failed);
    let duration = format_duration(summary.duration_ms);
    let dim_start = if no_color { "" } else { "\x1b[2m" };
    let dim_end = if no_color { "" } else { "\x1b[0m" };
    let verdict = if failed == 0 { "PASS" } else { "FAIL" };
    format!(
        "{dim_start}Run {} {dim_end} {}   exit {}   passed {}/{} steps   failed {}/{} steps   {}",
        run_id,
        verdict,
        summary.exit_code,
        passed,
        total,
        failed,
        total,
        duration,
        dim_start = dim_start,
        dim_end = dim_end,
    )
}

fn render_group_block(group: &FailureGroup, no_color: bool) -> String {
    let bullet = if no_color {
        "●"
    } else {
        "\x1b[31m●\x1b[0m"
    };
    let dim_start = if no_color { "" } else { "\x1b[2m" };
    let dim_end = if no_color { "" } else { "\x1b[0m" };

    let mut out = String::new();
    let exemplar = &group.root_cause;
    out.push_str(&format!(
        "{} {}   {}::{}::{}\n",
        bullet, group.fingerprint, exemplar.file, exemplar.test, exemplar.step
    ));

    // Second line: compact request/response excerpt. We prefer
    // `method url → status "excerpt"` because that trio is what a
    // human wants to see when deciding "is this worth opening?".
    if let Some(req) = &exemplar.request {
        let status_piece = exemplar
            .response
            .as_ref()
            .and_then(|r| r.status)
            .map(|s| format!(" → {}", s))
            .unwrap_or_default();
        let body_piece = exemplar
            .response
            .as_ref()
            .and_then(|r| r.body_excerpt.as_ref())
            .map(|b| format!(" {}", truncate_excerpt(b, 40)))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {}{} {}{}{}{}\n",
            dim_start, req.method, req.url, status_piece, body_piece, dim_end,
        ));
    } else {
        // Non-HTTP failures (parse_error, unresolved_template, etc.)
        // still deserve a one-line hint, so surface the first line of
        // the message instead of the request.
        let first = exemplar
            .message
            .lines()
            .next()
            .unwrap_or(exemplar.message.as_str());
        out.push_str(&format!("  {}{}{}\n", dim_start, first, dim_end));
    }

    if !group.blocked_steps.is_empty() {
        out.push_str(&format!(
            "  └─ cascades: {} skipped\n",
            group.blocked_steps.len()
        ));
    }
    out
}

fn group_to_json(group: &FailureGroup) -> Value {
    let exemplar = &group.root_cause;
    json!({
        "fingerprint": group.fingerprint,
        "occurrences": group.occurrences,
        "cascades": group.blocked_steps.len(),
        "primary": {
            "file": exemplar.file,
            "test": exemplar.test,
            "step": exemplar.step,
            "category": exemplar.category.map(category_label),
            "method": exemplar.request.as_ref().map(|r| r.method.clone()),
            "url":    exemplar.request.as_ref().map(|r| r.url.clone()),
            "status": exemplar.response.as_ref().and_then(|r| r.status),
            "message": exemplar
                .message
                .lines()
                .next()
                .unwrap_or(exemplar.message.as_str())
                .to_string(),
        },
    })
}

fn category_label(cat: FailureCategory) -> &'static str {
    match cat {
        FailureCategory::AssertionFailed => "assertion_failed",
        FailureCategory::ConnectionError => "connection_error",
        FailureCategory::Timeout => "timeout",
        FailureCategory::ParseError => "parse_error",
        FailureCategory::CaptureError => "capture_error",
        FailureCategory::UnresolvedTemplate => "unresolved_template",
        FailureCategory::SkippedDueToFailedCapture => "skipped_due_to_failed_capture",
        FailureCategory::SkippedDueToFailFast => "skipped_due_to_fail_fast",
        FailureCategory::SkippedByCondition => "skipped_by_condition",
    }
}

/// Pretty-print the run duration using the smallest unit that keeps
/// meaningful precision. Matches the compact format's feel so users
/// moving between `tarn run` and `tarn report` see consistent units.
fn format_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        let total_seconds = ms / 1_000;
        let minutes = total_seconds / 60;
        let seconds = total_seconds % 60;
        format!("{}m{:02}s", minutes, seconds)
    }
}

/// Truncate a response body excerpt to `limit` visible characters,
/// appending `…` when clipped. Body excerpts in `failures.json` are
/// already capped at ~500 chars; the concise view tightens that much
/// further so the per-group block stays ~3 lines.
fn truncate_excerpt(input: &str, limit: usize) -> String {
    let trimmed: String = input.chars().filter(|c| *c != '\n').collect();
    if trimmed.chars().count() <= limit {
        return format!("\"{}\"", trimmed);
    }
    let clipped: String = trimmed.chars().take(limit).collect();
    format!("\"{}…\"", clipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::summary::{
        Counts, FailureEntry, FailureRequest, FailureResponse, RootCauseRef, SUMMARY_SCHEMA_VERSION,
    };

    fn make_summary(exit_code: i32, total_steps: usize, failed_steps: usize) -> SummaryDoc {
        SummaryDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid".into()),
            started_at: "2026-01-01T00:00:00Z".into(),
            ended_at: "2026-01-01T00:00:01Z".into(),
            duration_ms: 1234,
            exit_code,
            totals: Counts {
                files: 1,
                tests: 1,
                steps: total_steps,
            },
            failed: Counts {
                files: if failed_steps == 0 { 0 } else { 1 },
                tests: if failed_steps == 0 { 0 } else { 1 },
                steps: failed_steps,
            },
            failed_files: if failed_steps == 0 {
                vec![]
            } else {
                vec!["a.tarn.yaml".into()]
            },
            rerun_source: None,
        }
    }

    /// Builder input for `make_failure`. Bundled into a struct so the
    /// factory takes one argument — avoids a `too_many_arguments` lint
    /// at the root cause rather than silencing it.
    struct FailureFixture<'a> {
        file: &'a str,
        test: &'a str,
        step: &'a str,
        cat: FailureCategory,
        msg: &'a str,
        method: &'a str,
        url: &'a str,
        status: Option<u16>,
    }

    fn make_failure(fx: FailureFixture<'_>) -> FailureEntry {
        FailureEntry {
            file: fx.file.into(),
            test: fx.test.into(),
            step: fx.step.into(),
            failure_category: Some(fx.cat),
            message: fx.msg.into(),
            request: Some(FailureRequest {
                method: fx.method.into(),
                url: fx.url.into(),
            }),
            response: fx.status.map(|s| FailureResponse {
                status: Some(s),
                body_excerpt: Some(r#"{"error":"internal server error"}"#.into()),
            }),
            root_cause: None,
        }
    }

    fn doc(failures: Vec<FailureEntry>) -> FailuresDoc {
        FailuresDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid".into()),
            failures,
        }
    }

    #[test]
    fn passing_run_renders_single_header_line_with_pass_verdict() {
        let summary = make_summary(0, 5, 0);
        let failures = doc(vec![]);
        let out = render_concise(&summary, &failures, "rid", true);
        assert!(out.contains("Run rid"));
        assert!(out.contains("PASS"));
        assert!(out.contains("exit 0"));
        assert!(out.contains("passed 5/5 steps"));
        assert!(out.contains("failed 0/5 steps"));
        assert!(!out.contains("failures:"));
    }

    /// Canonical fixture for the "one HTTP failure" case — used by
    /// multiple tests so each reads as AAA on its own.
    fn basic_http_failure() -> FailureFixture<'static> {
        FailureFixture {
            file: "a.tarn.yaml",
            test: "t",
            step: "s",
            cat: FailureCategory::AssertionFailed,
            msg: "Expected HTTP status 200, got 500",
            method: "GET",
            url: "https://api.test/users",
            status: Some(500),
        }
    }

    #[test]
    fn failing_run_renders_fail_verdict_and_failures_section() {
        let summary = make_summary(1, 3, 1);
        let failures = doc(vec![make_failure(basic_http_failure())]);
        let out = render_concise(&summary, &failures, "rid", true);
        assert!(out.contains("FAIL"));
        assert!(out.contains("failures:"));
        assert!(out.contains("status:200:500:GET:/users"));
        assert!(out.contains("a.tarn.yaml::t::s"));
        assert!(out.contains("GET https://api.test/users → 500"));
    }

    #[test]
    fn cascades_are_listed_as_suffix_not_as_extra_occurrences() {
        let root = make_failure(FailureFixture {
            file: "a.tarn.yaml",
            test: "t",
            step: "create",
            cat: FailureCategory::AssertionFailed,
            msg: "Expected HTTP status 201, got 500",
            method: "POST",
            url: "https://api.test/users",
            status: Some(500),
        });
        let cascade = FailureEntry {
            file: "a.tarn.yaml".into(),
            test: "t".into(),
            step: "delete".into(),
            failure_category: Some(FailureCategory::SkippedDueToFailedCapture),
            message: "Skipped".into(),
            request: None,
            response: None,
            root_cause: Some(RootCauseRef {
                file: "a.tarn.yaml".into(),
                test: "t".into(),
                step: "create".into(),
            }),
        };
        let summary = make_summary(1, 2, 2);
        let out = render_concise(&summary, &doc(vec![root, cascade]), "rid", true);
        // One group, one occurrence (the cascade does not inflate it).
        assert!(out.contains("cascades: 1 skipped"));
        // The cascade itself should not get its own fingerprint block.
        assert_eq!(out.matches("●").count(), 1);
    }

    #[test]
    fn group_list_truncates_past_ten_with_remainder_line() {
        // Twelve distinct status fingerprints → should show 10 + "…and 2 more".
        let urls: Vec<String> = (0..12)
            .map(|i| format!("https://api.test/u{}", i))
            .collect();
        let steps: Vec<String> = (0..12).map(|i| format!("s{}", i)).collect();
        let entries: Vec<FailureEntry> = (0..12)
            .map(|i| {
                make_failure(FailureFixture {
                    file: "a.tarn.yaml",
                    test: "t",
                    step: &steps[i],
                    cat: FailureCategory::AssertionFailed,
                    msg: "Expected HTTP status 200, got 500",
                    method: "GET",
                    // Distinct URL path per entry so each gets a
                    // distinct fingerprint (same status/method would
                    // otherwise collapse them into one group).
                    url: &urls[i],
                    status: Some(500),
                })
            })
            .collect();
        let summary = make_summary(1, 12, 12);
        let out = render_concise(&summary, &doc(entries), "rid", true);
        assert_eq!(
            out.matches("●").count(),
            MAX_HUMAN_GROUPS,
            "expected only the first {} groups rendered, got: {}",
            MAX_HUMAN_GROUPS,
            out
        );
        assert!(out.contains("…and 2 more groups"));
        assert!(out.contains("tarn failures"));
    }

    #[test]
    fn json_mode_emits_documented_schema_keys() {
        let summary = make_summary(1, 2, 1);
        let failures = doc(vec![make_failure(basic_http_failure())]);
        let v = render_json(&summary, &failures, "rid");
        assert_eq!(v["schema_version"], CONCISE_SCHEMA_VERSION);
        assert_eq!(v["run_id"], "rid");
        assert_eq!(v["exit_code"], 1);
        assert_eq!(v["duration_ms"], 1234);
        assert_eq!(v["totals"]["steps"], 2);
        assert_eq!(v["failed"]["steps"], 1);
        assert_eq!(v["groups_truncated"], false);
        assert_eq!(v["groups_total"], 1);
        let group = &v["groups"][0];
        assert_eq!(group["fingerprint"], "status:200:500:GET:/users");
        assert_eq!(group["occurrences"], 1);
        assert_eq!(group["cascades"], 0);
        assert_eq!(group["primary"]["file"], "a.tarn.yaml");
        assert_eq!(group["primary"]["status"], 500);
        assert_eq!(group["primary"]["method"], "GET");
        assert_eq!(group["primary"]["category"], "assertion_failed");
    }

    #[test]
    fn json_mode_reports_groups_truncated_flag_when_over_cap() {
        let total = MAX_HUMAN_GROUPS + 3;
        let urls: Vec<String> = (0..total)
            .map(|i| format!("https://api.test/u{}", i))
            .collect();
        let steps: Vec<String> = (0..total).map(|i| format!("s{}", i)).collect();
        let entries: Vec<FailureEntry> = (0..total)
            .map(|i| {
                make_failure(FailureFixture {
                    file: "a.tarn.yaml",
                    test: "t",
                    step: &steps[i],
                    cat: FailureCategory::AssertionFailed,
                    msg: "Expected HTTP status 200, got 500",
                    method: "GET",
                    url: &urls[i],
                    status: Some(500),
                })
            })
            .collect();
        let summary = make_summary(1, total, total);
        let v = render_json(&summary, &doc(entries), "rid");
        assert_eq!(v["groups_truncated"], true);
        assert_eq!(v["groups_total"], total as u64);
        assert_eq!(v["groups"].as_array().unwrap().len(), MAX_HUMAN_GROUPS);
    }

    #[test]
    fn no_color_mode_strips_ansi_escapes() {
        let summary = make_summary(1, 2, 1);
        let failures = doc(vec![make_failure(basic_http_failure())]);
        let plain = render_concise(&summary, &failures, "rid", true);
        assert!(
            !plain.contains('\x1b'),
            "no-color mode must not emit ANSI escapes; got: {:?}",
            plain
        );
        let colored = render_concise(&summary, &failures, "rid", false);
        assert!(
            colored.contains('\x1b'),
            "color mode must emit ANSI escapes; got: {:?}",
            colored
        );
    }
}
