use crate::assert::hints::step_hints;
use crate::assert::types::{ErrorCode, FileResult, RunResult, StepResult};
use crate::model::Location;
use crate::report::redaction::{
    redact_headers, sanitize_assertion, sanitize_json, sanitize_string,
};
use crate::report::RenderOptions;
use serde_json::{json, Value};
use std::str::FromStr;

/// Serialize a `Location` into the `{ file, line, column }` shape that
/// `schemas/v1/report.json` documents and the VS Code extension's
/// `schemaGuards.ts` (NAZ-281) expects.
fn location_json(location: &Location) -> Value {
    json!({
        "file": location.file,
        "line": location.line,
        "column": location.column,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonOutputMode {
    Verbose,
    Compact,
}

impl FromStr for JsonOutputMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "verbose" => Ok(Self::Verbose),
            "compact" => Ok(Self::Compact),
            other => Err(format!("Unknown JSON mode: '{}'", other)),
        }
    }
}

/// Render test results as structured JSON per spec.
pub fn render(result: &RunResult) -> String {
    render_with_options(result, JsonOutputMode::Verbose, RenderOptions::default())
}

pub fn render_with_mode(result: &RunResult, mode: JsonOutputMode) -> String {
    render_with_options(result, mode, RenderOptions::default())
}

pub fn render_with_options(
    result: &RunResult,
    mode: JsonOutputMode,
    opts: RenderOptions,
) -> String {
    let files_json: Vec<Value> = result
        .file_results
        .iter()
        .filter(|file| !(opts.only_failed && file.passed))
        .map(|file| render_file(file, mode, opts))
        .collect();

    let output = json!({
        "schema_version": 1,
        "version": "1",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "duration_ms": result.duration_ms,
        "files": files_json,
        "summary": {
            "files": result.total_files(),
            "tests": result.file_results.iter().map(|f| f.test_results.len()).sum::<usize>(),
            "steps": {
                "total": result.total_steps(),
                "passed": result.passed_steps(),
                "failed": result.failed_steps(),
            },
            "status": if result.passed() { "PASSED" } else { "FAILED" },
        }
    });

    serde_json::to_string_pretty(&output).unwrap()
}

fn render_file(file: &FileResult, mode: JsonOutputMode, opts: RenderOptions) -> Value {
    let setup_json: Vec<Value> = file
        .setup_results
        .iter()
        .filter(|step| !(opts.only_failed && step.passed))
        .map(|step| render_step(step, &file.redaction, &file.redacted_values, mode))
        .collect();

    let tests_json: Vec<Value> = file
        .test_results
        .iter()
        .filter(|t| !(opts.only_failed && t.passed))
        .map(|t| {
            let steps_json: Vec<Value> = t
                .step_results
                .iter()
                .filter(|step| !(opts.only_failed && step.passed))
                .map(|step| render_step(step, &file.redaction, &file.redacted_values, mode))
                .collect();
            let mut test_obj = json!({
                "name": t.name,
                "description": t.description,
                "status": if t.passed { "PASSED" } else { "FAILED" },
                "duration_ms": t.duration_ms,
                "steps": steps_json,
            });
            if !t.captures.is_empty() {
                test_obj["captures"] = json!(t.captures);
            }
            test_obj
        })
        .collect();

    let teardown_json: Vec<Value> = file
        .teardown_results
        .iter()
        .filter(|step| !(opts.only_failed && step.passed))
        .map(|step| render_step(step, &file.redaction, &file.redacted_values, mode))
        .collect();

    json!({
        "file": file.file,
        "name": file.name,
        "status": if file.passed { "PASSED" } else { "FAILED" },
        "duration_ms": file.duration_ms,
        "summary": {
            "total": file.total_steps(),
            "passed": file.passed_steps(),
            "failed": file.failed_steps(),
        },
        "setup": setup_json,
        "tests": tests_json,
        "teardown": teardown_json,
    })
}

fn render_step(
    step: &StepResult,
    redaction: &crate::model::RedactionConfig,
    secret_values: &[String],
    mode: JsonOutputMode,
) -> Value {
    let details_source: Vec<_> = match mode {
        JsonOutputMode::Verbose => step.assertion_results.iter().collect(),
        JsonOutputMode::Compact => {
            if step.passed {
                Vec::new()
            } else {
                step.failures()
            }
        }
    };

    let details: Vec<Value> = details_source
        .into_iter()
        .map(|a| {
            let a = sanitize_assertion(a, redaction, secret_values);
            let mut obj = json!({
                "assertion": a.assertion,
                "passed": a.passed,
                "expected": a.expected,
                "actual": a.actual,
                "message": a.message,
                "diff": a.diff,
            });
            if let Some(location) = &a.location {
                obj["location"] = location_json(location);
            }
            obj
        })
        .collect();

    let mut obj = json!({
        "name": step.name,
        "status": if step.passed { "PASSED" } else { "FAILED" },
        "duration_ms": step.duration_ms,
        "assertions": {
            "total": step.total_assertions(),
            "passed": step.passed_assertions(),
            "failed": step.failed_assertions(),
            "details": details,
        },
    });

    if let Some(location) = &step.location {
        obj["location"] = location_json(location);
    }

    // Always include response_status and response_summary when available
    if let Some(status) = step.response_status {
        obj["response_status"] = json!(status);
    }
    if let Some(ref summary) = step.response_summary {
        obj["response_summary"] = json!(summary);
    }
    if !step.captures_set.is_empty() {
        obj["captures_set"] = json!(step.captures_set);
    }

    // Include failures shortcut list
    if !step.passed {
        // Compute diagnostic hints once per step; we attach them to the
        // failed `status` assertion so consumers see the hint alongside
        // the mismatch that triggered it.
        let diagnostic_hints = step_hints(step);
        let failures: Vec<Value> = step
            .failures()
            .iter()
            .map(|f| {
                let f = sanitize_assertion(f, redaction, secret_values);
                let mut obj = json!({
                    "assertion": f.assertion,
                    "expected": f.expected,
                    "actual": f.actual,
                    "message": f.message,
                    "diff": f.diff,
                });
                if let Some(location) = &f.location {
                    obj["location"] = location_json(location);
                }
                if f.assertion == "status" && !diagnostic_hints.is_empty() {
                    obj["hints"] = json!(diagnostic_hints);
                }
                obj
            })
            .collect();

        obj["assertions"]["failures"] = json!(failures);

        // Include failure category for structured error taxonomy
        if let Some(category) = &step.error_category {
            obj["failure_category"] = json!(category);
        }
        if let Some(code) = step.error_code() {
            obj["error_code"] = json!(code);
        }
        let remediation_hints = remediation_hints(step);
        if !remediation_hints.is_empty() {
            obj["remediation_hints"] = json!(remediation_hints);
        }

        // Include request/response for failed steps
        if let Some(ref req) = step.request_info {
            obj["request"] = json!({
                "method": req.method,
                "url": sanitize_string(&req.url, &redaction.replacement, secret_values),
                "headers": redact_headers(&req.headers, redaction, secret_values),
                "body": req.body.as_ref().map(|body| sanitize_json(body, &redaction.replacement, secret_values)),
            });
        }
        if let Some(ref resp) = step.response_info {
            let body = resp.body.as_ref().map(|body| {
                let sanitized = sanitize_json(body, &redaction.replacement, secret_values);
                if mode == JsonOutputMode::Compact {
                    truncate_json_body(&sanitized, 200)
                } else {
                    sanitized
                }
            });
            obj["response"] = json!({
                "status": resp.status,
                "headers": redact_headers(&resp.headers, redaction, secret_values),
                "body": body,
            });
        }
    }

    obj
}

fn remediation_hints(step: &StepResult) -> Vec<String> {
    let mut hints = Vec::new();

    if request_contains_templates(step.request_info.as_ref()) {
        hints.push(
            "Fix unresolved `{{ ... }}` variables in request URL, headers, or body before rerunning."
                .to_string(),
        );
    }

    match step.error_code() {
        Some(ErrorCode::AssertionMismatch) => {
            hints.push(
                "Inspect `assertions.failures` expected vs actual values and update the DSL or the service response."
                    .to_string(),
            );
            if step.response_info.is_some() {
                hints.push(
                    "Use the recorded `response` payload to realign assertions and captures with the actual API output."
                        .to_string(),
                );
            }
        }
        Some(ErrorCode::CaptureExtractionFailed) => {
            hints.push(
                "Verify that the capture source still exists in the latest response body, headers, cookies, status, or URL."
                    .to_string(),
            );
            hints.push(
                "If the API changed, update the capture JSONPath, header name, cookie name, or regex."
                    .to_string(),
            );
        }
        Some(ErrorCode::PollConditionNotMet) => {
            hints.push(
                "Increase `poll.max_attempts` or `poll.interval` if eventual consistency is expected."
                    .to_string(),
            );
            hints.push(
                "Check that `poll.until` matches the terminal state returned by the endpoint."
                    .to_string(),
            );
        }
        Some(ErrorCode::RequestTimedOut) => {
            hints.push(
                "Increase the step `timeout` if the endpoint is expected to be slow.".to_string(),
            );
            hints.push(
                "Check server latency, retry behavior, and upstream dependencies before rerunning."
                    .to_string(),
            );
        }
        Some(ErrorCode::ConnectionRefused) => {
            hints.push(
                "Confirm the target service is running and reachable from the current environment."
                    .to_string(),
            );
            hints.push(
                "Verify `env.base_url`, port, and proxy settings for this request.".to_string(),
            );
        }
        Some(ErrorCode::DnsResolutionFailed) => {
            hints.push(
                "Check the hostname in the request URL and local DNS or network configuration."
                    .to_string(),
            );
        }
        Some(ErrorCode::TlsVerificationFailed) => {
            hints.push(
                "Provide `cacert`, `cert`/`key`, or use `--insecure` only for local debugging."
                    .to_string(),
            );
        }
        Some(ErrorCode::RedirectLimitExceeded) => {
            hints.push(
                "Check for redirect loops or wrong redirect targets, or adjust `follow_redirects` / `max_redirs`."
                    .to_string(),
            );
        }
        Some(ErrorCode::InterpolationFailed)
        | Some(ErrorCode::ValidationFailed)
        | Some(ErrorCode::ConfigurationError)
        | Some(ErrorCode::ParseError) => {
            hints.push(
                "Fix the test file structure or interpolation inputs, then rerun `tarn validate` before `tarn run`."
                    .to_string(),
            );
        }
        Some(ErrorCode::NetworkError) => {
            hints.push(
                "Inspect base URL, proxy, TLS, and general network connectivity for this request."
                    .to_string(),
            );
        }
        Some(ErrorCode::SkippedDependency) => {
            hints.push(
                "This step did not execute. Fix the root-cause failure (listed in the failing assertion message) — this cascade entry will clear automatically once the upstream step passes."
                    .to_string(),
            );
        }
        None => {}
    }

    hints.dedup();
    hints
}

/// Truncate a JSON body to approximately `max_chars` for compact output.
fn truncate_json_body(value: &Value, max_chars: usize) -> Value {
    let serialized = serde_json::to_string(value).unwrap_or_default();
    if serialized.len() <= max_chars {
        return value.clone();
    }
    // Return a truncated string representation
    let end = serialized
        .char_indices()
        .take(max_chars)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(max_chars);
    Value::String(format!("{}...(truncated)", &serialized[..end]))
}

fn request_contains_templates(request: Option<&crate::assert::types::RequestInfo>) -> bool {
    let Some(request) = request else {
        return false;
    };

    if request.url.contains("{{") {
        return true;
    }
    if request
        .headers
        .iter()
        .any(|(name, value)| name.contains("{{") || value.contains("{{"))
    {
        return true;
    }
    request.body.as_ref().is_some_and(value_contains_templates)
}

fn value_contains_templates(value: &Value) -> bool {
    match value {
        Value::String(s) => s.contains("{{"),
        Value::Array(items) => items.iter().any(value_contains_templates),
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| key.contains("{{") || value_contains_templates(value)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::*;
    use jsonschema::validator_for;
    use std::collections::HashMap;

    #[test]
    fn json_mode_from_str() {
        assert_eq!(
            "verbose".parse::<JsonOutputMode>(),
            Ok(JsonOutputMode::Verbose)
        );
        assert_eq!(
            "compact".parse::<JsonOutputMode>(),
            Ok(JsonOutputMode::Compact)
        );
        assert!("other".parse::<JsonOutputMode>().is_err());
    }

    fn make_passing_run() -> RunResult {
        RunResult {
            duration_ms: 100,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Test".into(),
                passed: true,
                duration_ms: 100,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "my_test".into(),
                    description: Some("desc".into()),
                    passed: true,
                    duration_ms: 100,
                    step_results: vec![StepResult {
                        name: "step1".into(),
                        passed: true,
                        duration_ms: 50,
                        assertion_results: vec![AssertionResult::pass("status", "200", "200")],
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

    fn make_failing_run() -> RunResult {
        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), "Bearer secret-token".into());
        headers.insert("Content-Type".into(), "application/json".into());

        RunResult {
            duration_ms: 200,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Test".into(),
                passed: false,
                duration_ms: 200,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "failing_test".into(),
                    description: None,
                    passed: false,
                    duration_ms: 200,
                    step_results: vec![StepResult {
                        name: "bad_step".into(),
                        passed: false,
                        duration_ms: 100,
                        assertion_results: vec![AssertionResult::fail(
                            "status",
                            "200",
                            "404",
                            "Expected 200, got 404",
                        )],
                        request_info: Some(RequestInfo {
                            method: "GET".into(),
                            url: "http://localhost:3000/users".into(),
                            headers,
                            body: None,
                            multipart: None,
                        }),
                        response_info: Some(ResponseInfo {
                            status: 404,
                            headers: HashMap::new(),
                            body: Some(json!({"error": "not_found"})),
                        }),
                        error_category: Some(FailureCategory::AssertionFailed),
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
    fn json_output_is_valid_json() {
        let output = render(&make_passing_run());
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.is_object());
    }

    #[test]
    fn json_summary_for_passing() {
        let output = render(&make_passing_run());
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["summary"]["status"], "PASSED");
        assert_eq!(parsed["summary"]["steps"]["total"], 1);
        assert_eq!(parsed["summary"]["steps"]["passed"], 1);
        assert_eq!(parsed["summary"]["steps"]["failed"], 0);
    }

    #[test]
    fn json_summary_for_failing() {
        let output = render(&make_failing_run());
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["summary"]["status"], "FAILED");
        assert_eq!(parsed["summary"]["steps"]["failed"], 1);
    }

    #[test]
    fn json_includes_request_response_on_failure() {
        let output = render(&make_failing_run());
        let parsed: Value = serde_json::from_str(&output).unwrap();
        let step = &parsed["files"][0]["tests"][0]["steps"][0];
        assert!(step["request"].is_object());
        assert!(step["response"].is_object());
        assert_eq!(step["response"]["status"], 404);
    }

    #[test]
    fn json_excludes_request_response_on_pass() {
        let output = render(&make_passing_run());
        let parsed: Value = serde_json::from_str(&output).unwrap();
        let step = &parsed["files"][0]["tests"][0]["steps"][0];
        assert!(step.get("request").is_none());
        assert!(step.get("response").is_none());
    }

    #[test]
    fn json_redacts_authorization_header() {
        let output = render(&make_failing_run());
        let parsed: Value = serde_json::from_str(&output).unwrap();
        let headers = &parsed["files"][0]["tests"][0]["steps"][0]["request"]["headers"];
        assert_eq!(headers["Authorization"], "***");
        assert_eq!(headers["Content-Type"], "application/json");
    }

    #[test]
    fn json_includes_failure_details() {
        let output = render(&make_failing_run());
        let parsed: Value = serde_json::from_str(&output).unwrap();
        let failures = &parsed["files"][0]["tests"][0]["steps"][0]["assertions"]["failures"];
        assert!(failures.is_array());
        assert_eq!(failures[0]["assertion"], "status");
        assert_eq!(failures[0]["expected"], "200");
        assert_eq!(failures[0]["actual"], "404");
    }

    #[test]
    fn json_includes_diff_when_present() {
        let run = RunResult {
            duration_ms: 10,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Test".into(),
                passed: false,
                duration_ms: 10,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "diff_test".into(),
                    description: None,
                    passed: false,
                    duration_ms: 10,
                    step_results: vec![StepResult {
                        name: "step".into(),
                        passed: false,
                        duration_ms: 10,
                        assertion_results: vec![AssertionResult::fail_with_diff(
                            "body $",
                            "\"a\"",
                            "\"b\"",
                            "body mismatch",
                            "--- expected\n+++ actual\n-a\n+b\n",
                        )],
                        request_info: None,
                        response_info: None,
                        error_category: Some(FailureCategory::AssertionFailed),
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

        let parsed: Value = serde_json::from_str(&render(&run)).unwrap();
        let detail = &parsed["files"][0]["tests"][0]["steps"][0]["assertions"]["details"][0];
        assert_eq!(detail["diff"], "--- expected\n+++ actual\n-a\n+b\n");
        let failure = &parsed["files"][0]["tests"][0]["steps"][0]["assertions"]["failures"][0];
        assert_eq!(failure["diff"], "--- expected\n+++ actual\n-a\n+b\n");
    }

    #[test]
    fn json_has_version_and_timestamp() {
        let output = render(&make_passing_run());
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["version"], "1");
        assert_eq!(parsed["schema_version"], 1);
        assert!(parsed["timestamp"].is_string());
    }

    // --- Secret redaction ---

    #[test]
    fn redact_authorization() {
        let mut h = HashMap::new();
        h.insert("Authorization".into(), "Bearer token".into());
        let redacted = redact_headers(&h, &crate::model::RedactionConfig::default(), &[]);
        assert_eq!(redacted.get("Authorization").unwrap(), "***");
    }

    #[test]
    fn redact_cookie() {
        let mut h = HashMap::new();
        h.insert("Cookie".into(), "session=abc".into());
        let redacted = redact_headers(&h, &crate::model::RedactionConfig::default(), &[]);
        assert_eq!(redacted.get("Cookie").unwrap(), "***");
    }

    #[test]
    fn no_redaction_for_safe_headers() {
        let mut h = HashMap::new();
        h.insert("Content-Type".into(), "application/json".into());
        let redacted = redact_headers(&h, &crate::model::RedactionConfig::default(), &[]);
        assert_eq!(redacted.get("Content-Type").unwrap(), "application/json");
    }

    #[test]
    fn redact_case_insensitive() {
        let mut h = HashMap::new();
        h.insert("authorization".into(), "Bearer token".into());
        let redacted = redact_headers(&h, &crate::model::RedactionConfig::default(), &[]);
        assert_eq!(redacted.get("authorization").unwrap(), "***");
    }

    #[test]
    fn custom_redaction_policy_overrides_defaults() {
        let mut h = HashMap::new();
        h.insert("Authorization".into(), "Bearer token".into());
        h.insert("X-Session-Token".into(), "secret".into());

        let redacted = redact_headers(
            &h,
            &crate::model::RedactionConfig {
                headers: vec!["x-session-token".into()],
                replacement: "[hidden]".into(),
                env_vars: vec![],
                captures: vec![],
            },
            &[],
        );

        assert_eq!(redacted.get("Authorization").unwrap(), "Bearer token");
        assert_eq!(redacted.get("X-Session-Token").unwrap(), "[hidden]");
    }

    #[test]
    fn json_redacts_configured_env_and_capture_values() {
        let run = RunResult {
            duration_ms: 10,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Secrets".into(),
                passed: false,
                duration_ms: 10,
                redaction: crate::model::RedactionConfig {
                    headers: crate::model::RedactionConfig::default().headers,
                    replacement: "[hidden]".into(),
                    env_vars: vec!["api_token".into()],
                    captures: vec!["session".into()],
                },
                redacted_values: vec!["env-secret".into(), "captured-secret".into()],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "leaks".into(),
                    description: None,
                    passed: false,
                    duration_ms: 10,
                    step_results: vec![StepResult {
                        name: "step".into(),
                        passed: false,
                        duration_ms: 10,
                        assertion_results: vec![AssertionResult::fail(
                            "body $.token",
                            "captured-secret",
                            "env-secret",
                            "Expected captured-secret but got env-secret",
                        )],
                        request_info: Some(RequestInfo {
                            method: "GET".into(),
                            url: "https://example.com?token=env-secret".into(),
                            headers: HashMap::from([("X-Trace".into(), "captured-secret".into())]),
                            body: Some(json!({"token": "captured-secret"})),
                            multipart: None,
                        }),
                        response_info: Some(ResponseInfo {
                            status: 401,
                            headers: HashMap::new(),
                            body: Some(json!({"error": "env-secret"})),
                        }),
                        error_category: Some(FailureCategory::AssertionFailed),
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

        let parsed: Value = serde_json::from_str(&render(&run)).unwrap();
        let step = &parsed["files"][0]["tests"][0]["steps"][0];
        assert_eq!(step["request"]["url"], "https://example.com?token=[hidden]");
        assert_eq!(step["request"]["headers"]["X-Trace"], "[hidden]");
        assert_eq!(step["request"]["body"]["token"], "[hidden]");
        assert_eq!(step["response"]["body"]["error"], "[hidden]");
        assert_eq!(
            step["assertions"]["failures"][0]["message"],
            "Expected [hidden] but got [hidden]"
        );
    }

    #[test]
    fn json_includes_failure_category() {
        let output = render(&make_failing_run());
        let parsed: Value = serde_json::from_str(&output).unwrap();
        let step = &parsed["files"][0]["tests"][0]["steps"][0];
        assert_eq!(step["failure_category"], "assertion_failed");
    }

    #[test]
    fn json_includes_error_code_and_remediation_hints() {
        let output = render(&make_failing_run());
        let parsed: Value = serde_json::from_str(&output).unwrap();
        let step = &parsed["files"][0]["tests"][0]["steps"][0];
        assert_eq!(step["error_code"], "assertion_mismatch");
        assert!(step["remediation_hints"].is_array());
        assert!(step["remediation_hints"][0]
            .as_str()
            .unwrap()
            .contains("assertions.failures"));
    }

    #[test]
    fn json_failure_includes_route_ordering_hint_when_body_signals_it() {
        let run = RunResult {
            duration_ms: 10,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Suite".into(),
                passed: false,
                duration_ms: 10,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "approve".into(),
                    description: None,
                    passed: false,
                    duration_ms: 10,
                    step_results: vec![StepResult {
                        name: "POST /orders/approve".into(),
                        passed: false,
                        duration_ms: 10,
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
                            body: Some(json!({"message": "Validation failed (uuid is expected)"})),
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

        let parsed: Value = serde_json::from_str(&render(&run)).unwrap();
        let failure = &parsed["files"][0]["tests"][0]["steps"][0]["assertions"]["failures"][0];
        let hints = failure["hints"].as_array().expect("hints must be an array");
        assert_eq!(hints.len(), 1);
        assert!(hints[0]
            .as_str()
            .unwrap()
            .contains("docs/TROUBLESHOOTING.md#route-ordering"));
    }

    #[test]
    fn json_failure_omits_hints_when_no_signal() {
        let parsed: Value = serde_json::from_str(&render(&make_failing_run())).unwrap();
        let failure = &parsed["files"][0]["tests"][0]["steps"][0]["assertions"]["failures"][0];
        assert!(failure.get("hints").is_none());
    }

    #[test]
    fn compact_json_omits_passed_assertion_details() {
        let output = render_with_mode(&make_passing_run(), JsonOutputMode::Compact);
        let parsed: Value = serde_json::from_str(&output).unwrap();
        let details = &parsed["files"][0]["tests"][0]["steps"][0]["assertions"]["details"];
        assert_eq!(details.as_array().unwrap().len(), 0);
    }

    #[test]
    fn compact_json_keeps_only_failures_in_details() {
        let output = render_with_mode(&make_failing_run(), JsonOutputMode::Compact);
        let parsed: Value = serde_json::from_str(&output).unwrap();
        let details = &parsed["files"][0]["tests"][0]["steps"][0]["assertions"]["details"];
        assert_eq!(details.as_array().unwrap().len(), 1);
        assert_eq!(details[0]["assertion"], "status");
    }

    #[test]
    fn passing_output_matches_report_schema() {
        validate_against_schema(&render(&make_passing_run()));
    }

    #[test]
    fn failing_output_matches_report_schema() {
        validate_against_schema(&render(&make_failing_run()));
    }

    #[test]
    fn compact_outputs_match_report_schema() {
        validate_against_schema(&render_with_mode(
            &make_passing_run(),
            JsonOutputMode::Compact,
        ));
        validate_against_schema(&render_with_mode(
            &make_failing_run(),
            JsonOutputMode::Compact,
        ));
    }

    fn validate_against_schema(output: &str) {
        let schema: Value =
            serde_json::from_str(include_str!("../../../schemas/v1/report.json")).unwrap();
        let instance: Value = serde_json::from_str(output).unwrap();
        let validator = validator_for(&schema).unwrap();
        let result = validator.validate(&instance);
        assert!(
            result.is_ok(),
            "schema validation failed: {:?}",
            result.err()
        );
    }
}
