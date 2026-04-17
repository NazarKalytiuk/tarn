//! Diagnostic hints for common, hard-to-spot server-side failure modes.
//!
//! These helpers inspect the response body for well-known textual
//! signals and, when confident, return a short single-line hint that
//! report formatters can surface alongside the raw assertion failure.
//!
//! The bar for emitting a hint is "clear textual signal". False
//! positives are worse than silence — a misleading note would
//! consume user attention and erode trust in every other hint.

use crate::assert::types::StepResult;

/// Canonical hint text for the NestJS-style route-ordering trap.
///
/// Exposed as a module constant so report formatters and tests can
/// reference the same string without re-typing it (and drifting).
pub const ROUTE_ORDERING_HINT: &str = "note: the server may have matched this path to a dynamic route (e.g. /foo/:id); check for route ordering conflicts (see docs/TROUBLESHOOTING.md#route-ordering).";

/// Return a route-ordering hint when the response body contains a clear
/// signal that the server rejected a URL segment as a dynamic-route
/// parameter (e.g. `/foo/:id`) rather than matching the specific route
/// the user expected (e.g. `/foo/approve`).
///
/// Only emits when both of these hold:
///
/// * The URL has at least two path segments after the host (so there
///   is plausibly a `/:param` at the end that could have swallowed a
///   specific-route call).
/// * The body contains a textual signal that the failure was a param
///   validation error — phrases like "route not found", "invalid id",
///   "invalid uuid", "cannot parse", "validation failed", or a
///   framework error that mentions `param` together with a URL segment
///   value.
///
/// Returns `None` otherwise.
///
/// The bodies inspected here are framework error payloads (NestJS,
/// FastAPI, Express/Joi, etc.) that tend to use a small, stable set of
/// phrases. When the body doesn't look like any of those, the function
/// stays silent — per project rules, false positives are worse than no
/// hint at all.
pub fn route_ordering_hint(url: &str, response_body: &str) -> Option<String> {
    if response_body.trim().is_empty() {
        return None;
    }

    let segments = extract_path_segments(url);
    if segments.len() < 2 {
        // Need at least /foo/<something> for the trap to be plausible.
        return None;
    }

    let body_lower = response_body.to_ascii_lowercase();

    if has_route_ordering_signal(&body_lower, &segments) {
        Some(ROUTE_ORDERING_HINT.to_string())
    } else {
        None
    }
}

/// Decide whether a body (already lowercased) contains a textual
/// signal consistent with a URL segment being rejected as a dynamic
/// route parameter.
fn has_route_ordering_signal(body_lower: &str, segments: &[String]) -> bool {
    // 1. Direct textual signals. These phrases are stable across NestJS
    //    / FastAPI / Express+Joi / class-validator error payloads and
    //    are rare in success responses, so matching any one of them on
    //    a 4xx response is a high-signal event.
    let direct_signals = [
        "route not found",
        "invalid id",
        "invalid uuid",
        "cannot parse",
        "could not parse",
        "failed to parse",
        "validation failed",
    ];
    if direct_signals.iter().any(|sig| body_lower.contains(sig)) {
        return true;
    }

    // 2. Framework-style "invalid param" message that names one of the
    //    URL path segments. Matches things like:
    //      { "message": "param id must be a UUID", ... }
    //      { "message": "Validation failed (uuid is expected) for param 'approve'" }
    //    We require the body to mention `param` *and* a literal URL
    //    segment to keep the false-positive rate low.
    if body_lower.contains("param") {
        for segment in segments {
            if segment.is_empty() {
                continue;
            }
            // Ignore trivially short segments and pure numerics — they
            // are too likely to collide with unrelated text.
            if segment.len() < 3 {
                continue;
            }
            let segment_lower = segment.to_ascii_lowercase();
            if body_lower.contains(&segment_lower) {
                return true;
            }
        }
    }

    false
}

/// Extract path segments from a URL, handling both absolute URLs and
/// relative paths. Strips the query string and fragment. Returns the
/// non-empty segments in order.
fn extract_path_segments(url: &str) -> Vec<String> {
    let path = strip_scheme_and_authority(url);
    let path = path.split('?').next().unwrap_or("");
    let path = path.split('#').next().unwrap_or("");

    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Strip `scheme://authority` from the front of a URL, returning the
/// remaining path (without query/fragment handling). If there is no
/// scheme, the input is returned unchanged.
fn strip_scheme_and_authority(url: &str) -> &str {
    if let Some(idx) = url.find("://") {
        let after_scheme = &url[idx + 3..];
        // Find the first '/' that starts the path.
        if let Some(path_idx) = after_scheme.find('/') {
            &after_scheme[path_idx..]
        } else {
            ""
        }
    } else {
        url
    }
}

/// Compute all diagnostic hints for a failed step. Returns an empty
/// vector when no hint applies (including on passing steps).
///
/// Today this only emits the route-ordering hint, but the signature is
/// shaped so additional diagnostics can be layered in without having
/// to thread new return values through every report formatter.
///
/// Preconditions for the route-ordering hint:
///   * The step failed.
///   * A status assertion failed with a 2xx-expected / 4xx-actual
///     shape. 5xx responses are excluded because route-ordering
///     traps surface as client errors, not server errors.
///   * Both `request_info` and `response_info` are present (we need
///     the URL and response body to inspect).
pub fn step_hints(step: &StepResult) -> Vec<String> {
    if step.passed {
        return Vec::new();
    }

    let Some(request) = step.request_info.as_ref() else {
        return Vec::new();
    };
    let Some(response) = step.response_info.as_ref() else {
        return Vec::new();
    };

    if !is_2xx_expected_4xx_actual(step, response.status) {
        return Vec::new();
    }

    let body_text = response
        .body
        .as_ref()
        .map(|body| match body {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default();

    let mut hints = Vec::new();
    if let Some(hint) = route_ordering_hint(&request.url, &body_text) {
        hints.push(hint);
    }
    hints
}

/// Return true when the step has a failing `status` assertion whose
/// expected value implies a 2xx success while the actual response was
/// a 4xx client error.
fn is_2xx_expected_4xx_actual(step: &StepResult, actual_status: u16) -> bool {
    if !(400..500).contains(&actual_status) {
        return false;
    }

    step.assertion_results
        .iter()
        .filter(|a| !a.passed && a.assertion == "status")
        .any(|a| status_expectation_implies_2xx(&a.expected))
}

/// Heuristic: does the `expected` string we display for a status
/// assertion describe a success (2xx) outcome?
///
/// The `expected` field is rendered by `assert_status` itself, so the
/// shapes are narrow: an exact code (`"200"`), a shorthand (`"2xx"`),
/// or a joined set of range conditions (`"in [200, 201]"`, `">= 200, < 300"`).
fn status_expectation_implies_2xx(expected: &str) -> bool {
    let trimmed = expected.trim();
    if trimmed.is_empty() {
        return false;
    }

    // Exact code, e.g. "200".
    if let Ok(code) = trimmed.parse::<u16>() {
        return (200..300).contains(&code);
    }

    // Shorthand like "2xx" (case-insensitive).
    let lower = trimmed.to_ascii_lowercase();
    if lower == "2xx" {
        return true;
    }

    // Set form: `in [200, 201, 204]`. Accept when all listed codes are
    // 2xx and no inequality operators are mixed in.
    if lower.starts_with("in [") && !lower.contains("<") && !lower.contains(">") {
        let digit_runs = extract_u16_runs(trimmed);
        if !digit_runs.is_empty() {
            return digit_runs.iter().all(|code| (200..300).contains(code));
        }
    }

    // Range form: the renderer joins conditions with ", ". Only treat
    // the spec as 2xx-expected when it is fully *bounded* inside the
    // 2xx band. An open-ended lower or upper bound is ambiguous (it
    // could legitimately include 3xx/4xx) and must not fire the hint.
    let (lower_bound, upper_bound) = collect_range_bounds(trimmed);
    if let (Some(lo), Some(hi)) = (lower_bound, upper_bound) {
        return lo >= 200 && hi <= 299;
    }

    false
}

/// Parse a range-form status expectation like `">= 200, < 300"` into
/// an inclusive `(lower, upper)` pair of 2xx-band-comparable u16s.
/// Returns `None` for whichever side is missing or unparseable.
fn collect_range_bounds(input: &str) -> (Option<u16>, Option<u16>) {
    let mut lower: Option<u16> = None;
    let mut upper: Option<u16> = None;

    for part in input.split(',').map(|s| s.trim()) {
        if let Some(rest) = part.strip_prefix(">=") {
            if let Ok(v) = rest.trim().parse::<u16>() {
                lower = Some(lower.map_or(v, |cur| cur.max(v)));
            }
        } else if let Some(rest) = part.strip_prefix('>') {
            if let Ok(v) = rest.trim().parse::<u16>() {
                let v_inclusive = v.saturating_add(1);
                lower = Some(lower.map_or(v_inclusive, |cur| cur.max(v_inclusive)));
            }
        } else if let Some(rest) = part.strip_prefix("<=") {
            if let Ok(v) = rest.trim().parse::<u16>() {
                upper = Some(upper.map_or(v, |cur| cur.min(v)));
            }
        } else if let Some(rest) = part.strip_prefix('<') {
            if let Ok(v) = rest.trim().parse::<u16>() {
                let v_inclusive = v.saturating_sub(1);
                upper = Some(upper.map_or(v_inclusive, |cur| cur.min(v_inclusive)));
            }
        }
    }

    (lower, upper)
}

/// Extract every contiguous run of ASCII digits in `input` and parse
/// them as `u16`, silently skipping values that don't fit.
fn extract_u16_runs(input: &str) -> Vec<u16> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in input.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            if let Ok(v) = current.parse::<u16>() {
                out.push(v);
            }
            current.clear();
        }
    }
    if !current.is_empty() {
        if let Ok(v) = current.parse::<u16>() {
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Positive cases ---

    #[test]
    fn emits_hint_on_invalid_uuid_body() {
        let url = "http://api.example.com/orders/approve";
        let body = r#"{"statusCode":400,"message":"Validation failed (uuid is expected)","error":"Bad Request"}"#;
        let hint = route_ordering_hint(url, body).expect("hint should fire");
        assert_eq!(hint, ROUTE_ORDERING_HINT);
    }

    #[test]
    fn emits_hint_on_invalid_id_body() {
        let url = "http://api.example.com/users/me";
        let body = r#"{"error":"Invalid id"}"#;
        assert_eq!(
            route_ordering_hint(url, body),
            Some(ROUTE_ORDERING_HINT.to_string()),
        );
    }

    #[test]
    fn emits_hint_on_route_not_found_body() {
        let url = "http://api.example.com/foo/approve";
        let body = r#"{"message":"Route not found"}"#;
        assert!(route_ordering_hint(url, body).is_some());
    }

    #[test]
    fn emits_hint_on_cannot_parse_body() {
        let url = "http://api.example.com/foo/approve";
        let body = "cannot parse value as integer";
        assert!(route_ordering_hint(url, body).is_some());
    }

    #[test]
    fn emits_hint_on_failed_to_parse_body() {
        let url = "http://api.example.com/foo/approve";
        let body = r#"{"detail":"failed to parse path parameter"}"#;
        assert!(route_ordering_hint(url, body).is_some());
    }

    #[test]
    fn emits_hint_on_param_error_mentioning_segment() {
        let url = "http://api.example.com/orders/approve";
        let body =
            r#"{"message":"param 'approve' must be a valid ObjectId","error":"Bad Request"}"#;
        assert!(route_ordering_hint(url, body).is_some());
    }

    #[test]
    fn emits_hint_case_insensitive() {
        let url = "http://api.example.com/orders/approve";
        let body = r#"{"Error":"INVALID UUID"}"#;
        assert!(route_ordering_hint(url, body).is_some());
    }

    #[test]
    fn works_with_relative_url() {
        let url = "/orders/approve";
        let body = r#"{"message":"Validation failed"}"#;
        assert!(route_ordering_hint(url, body).is_some());
    }

    #[test]
    fn works_with_query_string_and_fragment() {
        let url = "http://api.example.com/orders/approve?tenant=1#frag";
        let body = r#"{"message":"invalid uuid"}"#;
        assert!(route_ordering_hint(url, body).is_some());
    }

    // --- Negative cases ---

    #[test]
    fn no_hint_on_generic_not_found_body() {
        // "Not Found" alone is ambiguous: could be a genuine missing
        // resource, not a route-ordering collision. Stay silent.
        let url = "http://api.example.com/orders/approve";
        let body = r#"{"statusCode":404,"message":"Not Found"}"#;
        assert_eq!(route_ordering_hint(url, body), None);
    }

    #[test]
    fn no_hint_on_empty_body() {
        let url = "http://api.example.com/orders/approve";
        assert_eq!(route_ordering_hint(url, ""), None);
        assert_eq!(route_ordering_hint(url, "   \n "), None);
    }

    #[test]
    fn no_hint_on_single_segment_path() {
        // A single-segment path cannot have a trailing `/:param` trap.
        let url = "http://api.example.com/health";
        let body = r#"{"message":"invalid uuid"}"#;
        assert_eq!(route_ordering_hint(url, body), None);
    }

    #[test]
    fn no_hint_on_root_path() {
        let url = "http://api.example.com/";
        let body = r#"{"message":"invalid uuid"}"#;
        assert_eq!(route_ordering_hint(url, body), None);
    }

    #[test]
    fn no_hint_on_success_body_shape() {
        let url = "http://api.example.com/orders/approve";
        let body = r#"{"status":"ok","id":"abc"}"#;
        assert_eq!(route_ordering_hint(url, body), None);
    }

    #[test]
    fn no_hint_on_param_without_segment_match() {
        // Body mentions "param" but not any URL segment — too weak a
        // signal to risk a false positive.
        let url = "http://api.example.com/orders/approve";
        let body = r#"{"message":"missing required param 'tenant'"}"#;
        assert_eq!(route_ordering_hint(url, body), None);
    }

    #[test]
    fn no_hint_on_unrelated_error_body() {
        let url = "http://api.example.com/orders/approve";
        let body = r#"{"message":"Insufficient permissions"}"#;
        assert_eq!(route_ordering_hint(url, body), None);
    }

    #[test]
    fn no_hint_on_very_short_segment_coincidence() {
        // A short 2-char segment like "me" must not trigger the param
        // signal just because the body happens to contain the letters.
        let url = "http://api.example.com/users/me";
        let body = r#"{"message":"missing required param 'tenant' for endpoint"}"#;
        // "me" appears inside "endpoint" but we require segment.len() >= 3.
        assert_eq!(route_ordering_hint(url, body), None);
    }

    // --- Helper unit tests ---

    #[test]
    fn extract_segments_absolute_url() {
        let segs = extract_path_segments("http://api.example.com/foo/bar");
        assert_eq!(segs, vec!["foo".to_string(), "bar".to_string()]);
    }

    #[test]
    fn extract_segments_strips_query() {
        let segs = extract_path_segments("http://api.example.com/foo/bar?x=1&y=2");
        assert_eq!(segs, vec!["foo".to_string(), "bar".to_string()]);
    }

    #[test]
    fn extract_segments_strips_fragment() {
        let segs = extract_path_segments("http://api.example.com/foo/bar#section");
        assert_eq!(segs, vec!["foo".to_string(), "bar".to_string()]);
    }

    #[test]
    fn extract_segments_relative_path() {
        let segs = extract_path_segments("/foo/bar");
        assert_eq!(segs, vec!["foo".to_string(), "bar".to_string()]);
    }

    #[test]
    fn extract_segments_empty_for_bare_host() {
        assert!(extract_path_segments("http://api.example.com").is_empty());
        assert!(extract_path_segments("http://api.example.com/").is_empty());
    }

    // --- Status-expectation heuristic ---

    #[test]
    fn status_expectation_2xx_forms() {
        assert!(status_expectation_implies_2xx("200"));
        assert!(status_expectation_implies_2xx("201"));
        assert!(status_expectation_implies_2xx("204"));
        assert!(status_expectation_implies_2xx("2xx"));
        assert!(status_expectation_implies_2xx("2XX"));
        assert!(status_expectation_implies_2xx("in [200, 201, 204]"));
        // Bounded 2xx range — same shape the complex spec renderer
        // emits for `gte: 200, lt: 300`.
        assert!(status_expectation_implies_2xx(">= 200, < 300"));
        assert!(status_expectation_implies_2xx("> 199, <= 299"));
    }

    #[test]
    fn status_expectation_non_2xx_forms() {
        assert!(!status_expectation_implies_2xx(""));
        assert!(!status_expectation_implies_2xx("404"));
        assert!(!status_expectation_implies_2xx("4xx"));
        assert!(!status_expectation_implies_2xx("in [200, 500]"));
        // Open-ended range — deliberately excluded because it may
        // genuinely accept 3xx/4xx/5xx.
        assert!(!status_expectation_implies_2xx(">= 200"));
        assert!(!status_expectation_implies_2xx("< 500"));
        // Bounded range that reaches beyond 2xx.
        assert!(!status_expectation_implies_2xx(">= 200, < 500"));
        // Bounded range outside 2xx entirely.
        assert!(!status_expectation_implies_2xx(">= 400, < 500"));
    }

    // --- step_hints integration ---

    use crate::assert::types::{
        AssertionResult, FailureCategory, RequestInfo, ResponseInfo, StepResult,
    };
    use std::collections::HashMap;

    fn failing_step(
        expected: &str,
        actual_status: u16,
        url: &str,
        body: Option<serde_json::Value>,
    ) -> StepResult {
        StepResult {
            name: "call".into(),
            description: None,
            passed: false,
            duration_ms: 10,
            assertion_results: vec![AssertionResult::fail(
                "status",
                expected,
                actual_status.to_string(),
                format!("Expected HTTP status {}, got {}", expected, actual_status),
            )],
            request_info: Some(RequestInfo {
                method: "POST".into(),
                url: url.into(),
                headers: HashMap::new(),
                body: None,
                multipart: None,
            }),
            response_info: Some(ResponseInfo {
                status: actual_status,
                headers: HashMap::new(),
                body,
            }),
            error_category: Some(FailureCategory::AssertionFailed),
            response_status: Some(actual_status),
            response_summary: None,
            captures_set: vec![],
            location: None,
        }
    }

    #[test]
    fn step_hints_emitted_for_2xx_expected_4xx_actual_with_signal() {
        let body = serde_json::json!({"message": "Validation failed (uuid is expected)"});
        let step = failing_step("201", 400, "http://api/orders/approve", Some(body));
        let hints = step_hints(&step);
        assert_eq!(hints, vec![ROUTE_ORDERING_HINT.to_string()]);
    }

    #[test]
    fn step_hints_skipped_when_step_passed() {
        let body = serde_json::json!({"message": "invalid uuid"});
        let mut step = failing_step("201", 400, "http://api/orders/approve", Some(body));
        step.passed = true;
        assert!(step_hints(&step).is_empty());
    }

    #[test]
    fn step_hints_skipped_when_no_request_info() {
        let body = serde_json::json!({"message": "invalid uuid"});
        let mut step = failing_step("201", 400, "http://api/orders/approve", Some(body));
        step.request_info = None;
        assert!(step_hints(&step).is_empty());
    }

    #[test]
    fn step_hints_skipped_when_no_response_info() {
        let mut step = failing_step("201", 400, "http://api/orders/approve", None);
        step.response_info = None;
        assert!(step_hints(&step).is_empty());
    }

    #[test]
    fn step_hints_skipped_when_expected_non_2xx() {
        let body = serde_json::json!({"message": "invalid uuid"});
        let step = failing_step("404", 400, "http://api/orders/approve", Some(body));
        assert!(step_hints(&step).is_empty());
    }

    #[test]
    fn step_hints_skipped_when_actual_is_5xx() {
        let body = serde_json::json!({"message": "invalid uuid"});
        let step = failing_step("200", 500, "http://api/orders/approve", Some(body));
        assert!(step_hints(&step).is_empty());
    }

    #[test]
    fn step_hints_skipped_when_body_lacks_signal() {
        let body = serde_json::json!({"message": "Not Found"});
        let step = failing_step("200", 404, "http://api/orders/approve", Some(body));
        assert!(step_hints(&step).is_empty());
    }

    #[test]
    fn step_hints_handles_string_body() {
        // Some responses are stored as a JSON string (e.g. plain text).
        let body = serde_json::Value::String("Validation failed".into());
        let step = failing_step("200", 400, "http://api/orders/approve", Some(body));
        let hints = step_hints(&step);
        assert_eq!(hints, vec![ROUTE_ORDERING_HINT.to_string()]);
    }

    #[test]
    fn step_hints_ignores_passing_status_assertion() {
        let body = serde_json::json!({"message": "invalid uuid"});
        let mut step = failing_step("200", 400, "http://api/orders/approve", Some(body));
        // Replace with a passing status assertion plus a failing body
        // assertion — the hint should not fire because no *status*
        // assertion is failing.
        step.assertion_results = vec![
            AssertionResult::pass("status", "2xx", "200"),
            AssertionResult::fail("body $.name", "Alice", "Bob", "body mismatch"),
        ];
        assert!(step_hints(&step).is_empty());
    }
}
