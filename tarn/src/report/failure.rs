//! Shared rendering helpers for compact and llm formats.
//!
//! Both formats lean on the same primitives: a one-line request summary,
//! a short response body preview, and a grouped category+count summary of
//! every failure in the run. Extracting these keeps compact.rs and
//! llm.rs from drifting apart — if the failure signal changes, both
//! formats pick up the new shape automatically.

use crate::assert::types::{
    AssertionResult, ErrorCode, FailureCategory, FileResult, RunResult, StepResult, TestResult,
};
use crate::model::RedactionConfig;
use crate::report::redaction::{redact_headers, sanitize_assertion, sanitize_json, sanitize_string};

/// Maximum characters rendered for an assertion message in the compact
/// summary. LLM format uses a different cap via `format_assertion_line`.
pub const COMPACT_MESSAGE_CAP: usize = 200;

/// Maximum characters rendered for a captured value in `compact -v`.
pub const CAPTURE_VALUE_CAP: usize = 80;

/// Default body preview size for the llm format (characters of the
/// serialized JSON, not bytes). Keeps the block readable.
pub const LLM_BODY_PREVIEW_CHARS: usize = 4 * 1024;

/// One-line HTTP request summary such as `GET /v1/users`. Includes a
/// `(Authorization: ***)` trailer if any redacted header is present so
/// the caller can tell auth context apart without leaking the token.
pub fn request_line(step: &StepResult, redaction: &RedactionConfig, secrets: &[String]) -> String {
    let Some(req) = step.request_info.as_ref() else {
        return "(no request recorded)".to_string();
    };
    let sanitized_url = sanitize_string(&req.url, &redaction.replacement, secrets);
    let mut out = format!("{} {}", req.method, sanitized_url);
    let redacted_headers = redact_headers(&req.headers, redaction, secrets);
    let mut trailer: Vec<String> = redacted_headers
        .into_iter()
        .filter(|(_, v)| v == &redaction.replacement)
        .map(|(k, v)| format!("{}: {}", k, v))
        .collect();
    trailer.sort();
    if !trailer.is_empty() {
        out.push_str(&format!(" ({})", trailer.join(", ")));
    }
    out
}

/// Shorter variant of [`request_line`] used in the compact format's
/// single-line failure breadcrumb: `GET /foo -> 500`.
pub fn request_arrow_response(
    step: &StepResult,
    redaction: &RedactionConfig,
    secrets: &[String],
) -> String {
    let base = match step.request_info.as_ref() {
        Some(req) => {
            let sanitized_url = sanitize_string(&req.url, &redaction.replacement, secrets);
            format!("{} {}", req.method, sanitized_url)
        }
        None => "(no request)".to_string(),
    };
    match step.response_status {
        Some(code) => format!("{} -> {}", base, code),
        None => match step.response_info.as_ref() {
            Some(resp) => format!("{} -> {}", base, resp.status),
            None => format!("{} -> (no response)", base),
        },
    }
}

/// Format the first failing assertion message for a step. Falls back to
/// "(no failing assertion recorded)" for cascade/skip entries that still
/// carry an informational message.
pub fn primary_failure_message(
    step: &StepResult,
    redaction: &RedactionConfig,
    secrets: &[String],
    cap: usize,
) -> String {
    match step.assertion_results.iter().find(|a| !a.passed) {
        Some(a) => {
            let a = sanitize_assertion(a, redaction, secrets);
            truncate_string(&a.message, cap)
        }
        None => "(no failing assertion recorded)".to_string(),
    }
}

/// Format `assertion, got actual` line used by llm format. Truncates
/// very long actual values so the block stays readable.
pub fn format_assertion_line(
    assertion: &AssertionResult,
    redaction: &RedactionConfig,
    secrets: &[String],
) -> String {
    let a = sanitize_assertion(assertion, redaction, secrets);
    let expected = truncate_string(&a.expected, 200);
    let actual = truncate_string(&a.actual, 200);
    format!("{}: expected {}, got {}", a.assertion, expected, actual)
}

/// Render a short preview of the response body, truncating once the
/// serialized form exceeds `max_chars`. A final marker communicates how
/// large the untruncated body was, so LLM consumers can decide whether
/// to rerun with a larger `--max-body`.
pub fn response_body_preview(
    step: &StepResult,
    redaction: &RedactionConfig,
    secrets: &[String],
    max_chars: usize,
) -> Option<String> {
    let resp = step.response_info.as_ref()?;
    let body = resp.body.as_ref()?;
    let sanitized = sanitize_json(body, &redaction.replacement, secrets);
    let serialized = match serde_json::to_string(&sanitized) {
        Ok(s) => s,
        Err(_) => return None,
    };
    if serialized.is_empty() {
        return None;
    }
    if serialized.len() <= max_chars {
        return Some(serialized);
    }
    let end = serialized
        .char_indices()
        .take_while(|(idx, _)| *idx < max_chars)
        .last()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    Some(format!(
        "{}...<truncated: {} bytes>",
        &serialized[..end],
        serialized.len()
    ))
}

/// Truncate a display string to at most `cap` characters, appending an
/// ellipsis marker when truncation happened. Zero `cap` is treated as
/// "no limit" so callers can disable the behavior.
pub fn truncate_string(s: &str, cap: usize) -> String {
    if cap == 0 || s.chars().count() <= cap {
        return s.to_string();
    }
    let end = s
        .char_indices()
        .take(cap)
        .last()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    format!("{}...", &s[..end])
}

/// Aggregate failures across the run and return a sorted `(label, count)`
/// list suitable for `HTTP 500: 3 | JSONPath mismatch: 18`-style
/// summaries. The label is a human-readable category chosen from the
/// failure's `ErrorCode` (or HTTP status for assertion failures against
/// the status code). Highest-count first, then alphabetical.
pub fn group_failures(run: &RunResult) -> Vec<(String, usize)> {
    let mut groups: std::collections::BTreeMap<String, usize> = Default::default();
    for file in &run.file_results {
        for_each_step(file, &mut |_, _, step| {
            if step.passed {
                return;
            }
            let label = failure_label(step);
            *groups.entry(label).or_insert(0) += 1;
        });
    }
    let mut entries: Vec<(String, usize)> = groups.into_iter().collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries
}

/// Visit every step (setup, tests, teardown) in a file in source order.
/// The closure receives the owning test name when applicable (setup and
/// teardown pass `None`).
pub fn for_each_step<'a>(
    file: &'a FileResult,
    visit: &mut dyn FnMut(Option<&'a TestResult>, bool, &'a StepResult),
) {
    for step in &file.setup_results {
        visit(None, false, step);
    }
    for test in &file.test_results {
        for step in &test.step_results {
            visit(Some(test), false, step);
        }
    }
    for step in &file.teardown_results {
        visit(None, true, step);
    }
}

/// Pick a stable, human-readable label to bucket the failure under. The
/// heuristic walks from the most-specific signal (HTTP status assertion
/// mismatch) to the generic `ErrorCode`. Keep in sync with the
/// expectations baked into `compact.rs` / `llm.rs` test fixtures.
fn failure_label(step: &StepResult) -> String {
    if matches!(
        step.error_category,
        Some(FailureCategory::SkippedDueToFailedCapture)
            | Some(FailureCategory::SkippedDueToFailFast)
    ) {
        return "Skipped (cascade)".to_string();
    }

    if let Some(ErrorCode::AssertionMismatch) = step.error_code() {
        if let Some(status_assertion) = step
            .assertion_results
            .iter()
            .find(|a| !a.passed && a.assertion == "status")
        {
            if let Ok(actual) = status_assertion.actual.trim().parse::<u16>() {
                return format!("HTTP {}", actual);
            }
        }
        if let Some(a) = step
            .assertion_results
            .iter()
            .find(|a| !a.passed && a.assertion.starts_with("body "))
        {
            let _ = a;
            return "JSONPath mismatch".to_string();
        }
        return "Assertion mismatch".to_string();
    }

    match step.error_code() {
        Some(ErrorCode::CaptureExtractionFailed) => "Capture failed".to_string(),
        Some(ErrorCode::RequestTimedOut) => "Request timed out".to_string(),
        Some(ErrorCode::ConnectionRefused) => "Connection refused".to_string(),
        Some(ErrorCode::DnsResolutionFailed) => "DNS resolution failed".to_string(),
        Some(ErrorCode::TlsVerificationFailed) => "TLS verification failed".to_string(),
        Some(ErrorCode::RedirectLimitExceeded) => "Redirect limit exceeded".to_string(),
        Some(ErrorCode::NetworkError) => "Network error".to_string(),
        Some(ErrorCode::InterpolationFailed) => "Interpolation failed".to_string(),
        Some(ErrorCode::ValidationFailed) => "Validation failed".to_string(),
        Some(ErrorCode::ConfigurationError) => "Configuration error".to_string(),
        Some(ErrorCode::ParseError) => "Parse error".to_string(),
        Some(ErrorCode::PollConditionNotMet) => "Poll condition not met".to_string(),
        Some(ErrorCode::SkippedDependency) => "Skipped (cascade)".to_string(),
        Some(ErrorCode::AssertionMismatch) => "Assertion mismatch".to_string(),
        None => "Unknown failure".to_string(),
    }
}

/// Count cascade-skipped steps in a test, grouped by the failed capture
/// name they depended on. Returns `(cascade_capture_name, skipped_count)`
/// entries so both compact and llm formats can render `skipped: N steps
/// (depended on failed capture 'id')` lines.
pub fn skip_cascade_summary(test: &TestResult) -> Vec<(String, usize)> {
    let mut groups: std::collections::BTreeMap<String, usize> = Default::default();
    for step in &test.step_results {
        if !matches!(
            step.error_category,
            Some(FailureCategory::SkippedDueToFailedCapture)
        ) {
            continue;
        }
        let dep = step
            .assertion_results
            .iter()
            .find(|a| !a.passed)
            .and_then(|a| extract_capture_name(&a.message))
            .unwrap_or_else(|| "?".to_string());
        *groups.entry(dep).or_insert(0) += 1;
    }
    groups.into_iter().collect()
}

/// Extract the capture name mentioned in a cascade skip message. The
/// runner emits messages like "depends on capture 'id' which failed"
/// — we look for the first single-quoted substring.
fn extract_capture_name(message: &str) -> Option<String> {
    let start = message.find('\'')?;
    let rest = &message[start + 1..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::{
        AssertionResult, FailureCategory, FileResult, RequestInfo, ResponseInfo, RunResult,
        StepResult, TestResult,
    };
    use serde_json::json;
    use std::collections::HashMap;

    fn failing_step_with_status(name: &str, actual: u16) -> StepResult {
        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), "Bearer secret".into());
        StepResult {
            name: name.into(),
            description: None,
            passed: false,
            duration_ms: 10,
            assertion_results: vec![AssertionResult::fail(
                "status",
                "200",
                format!("{}", actual),
                format!("Expected 200, got {}", actual),
            )],
            request_info: Some(RequestInfo {
                method: "GET".into(),
                url: "http://x/y".into(),
                headers,
                body: None,
                multipart: None,
            }),
            response_info: Some(ResponseInfo {
                status: actual,
                headers: HashMap::new(),
                body: Some(json!({"err": "boom"})),
            }),
            error_category: Some(FailureCategory::AssertionFailed),
            response_status: Some(actual),
            response_summary: None,
            captures_set: vec![],
            location: None,
        }
    }

    fn run_with_mixed() -> RunResult {
        RunResult {
            duration_ms: 50,
            file_results: vec![FileResult {
                file: "a.tarn.yaml".into(),
                name: "A".into(),
                passed: false,
                duration_ms: 50,
                redaction: RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "t1".into(),
                    description: None,
                    passed: false,
                    duration_ms: 50,
                    step_results: vec![
                        failing_step_with_status("s500", 500),
                        failing_step_with_status("s500b", 500),
                        failing_step_with_status("s404", 404),
                    ],
                    captures: HashMap::new(),
                }],
                teardown_results: vec![],
            }],
        }
    }

    #[test]
    fn group_failures_sorts_by_count_then_label() {
        let groups = group_failures(&run_with_mixed());
        assert_eq!(groups[0], ("HTTP 500".to_string(), 2));
        assert_eq!(groups[1], ("HTTP 404".to_string(), 1));
    }

    #[test]
    fn request_line_trails_redacted_headers() {
        let step = failing_step_with_status("s", 500);
        let line = request_line(&step, &RedactionConfig::default(), &[]);
        assert!(line.starts_with("GET http://x/y"));
        assert!(line.contains("Authorization: ***"));
    }

    #[test]
    fn response_body_preview_truncates_over_cap() {
        let mut step = failing_step_with_status("s", 500);
        let big = serde_json::Value::String("x".repeat(64));
        step.response_info.as_mut().unwrap().body = Some(big);
        let preview =
            response_body_preview(&step, &RedactionConfig::default(), &[], 20).unwrap();
        assert!(preview.contains("<truncated:"));
        assert!(preview.len() < 100);
    }

    #[test]
    fn truncate_string_preserves_short_strings() {
        assert_eq!(truncate_string("short", 10), "short");
        assert_eq!(truncate_string("123456789012", 5), "12345...");
    }

    #[test]
    fn extract_capture_name_pulls_quoted_identifier() {
        assert_eq!(
            extract_capture_name("step skipped: depends on 'id' which failed"),
            Some("id".into())
        );
        assert_eq!(extract_capture_name("no quotes here"), None);
    }
}
