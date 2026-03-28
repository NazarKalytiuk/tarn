use crate::assert::types::{FileResult, RunResult, StepResult};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Render test results as structured JSON per spec.
pub fn render(result: &RunResult) -> String {
    let output = json!({
        "schema_version": 1,
        "version": "1",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "duration_ms": result.duration_ms,
        "files": result.file_results.iter().map(render_file).collect::<Vec<_>>(),
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

fn render_file(file: &FileResult) -> Value {
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
        "setup": file.setup_results.iter().map(render_step).collect::<Vec<_>>(),
        "tests": file.test_results.iter().map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "status": if t.passed { "PASSED" } else { "FAILED" },
                "duration_ms": t.duration_ms,
                "steps": t.step_results.iter().map(render_step).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
        "teardown": file.teardown_results.iter().map(render_step).collect::<Vec<_>>(),
    })
}

fn render_step(step: &StepResult) -> Value {
    // Always include per-assertion details so HTML report can show them
    let details: Vec<Value> = step
        .assertion_results
        .iter()
        .map(|a| {
            json!({
                "assertion": a.assertion,
                "passed": a.passed,
                "expected": a.expected,
                "actual": a.actual,
                "message": a.message,
            })
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

    // Include failures shortcut list
    if !step.passed {
        let failures: Vec<Value> = step
            .failures()
            .iter()
            .map(|f| {
                json!({
                    "assertion": f.assertion,
                    "expected": f.expected,
                    "actual": f.actual,
                    "message": f.message,
                })
            })
            .collect();

        obj["assertions"]["failures"] = json!(failures);

        // Include failure category for structured error taxonomy
        if let Some(category) = &step.error_category {
            obj["failure_category"] = json!(category);
        }

        // Include request/response for failed steps
        if let Some(ref req) = step.request_info {
            obj["request"] = json!({
                "method": req.method,
                "url": req.url,
                "headers": redact_secrets(&req.headers),
                "body": req.body,
            });
        }
        if let Some(ref resp) = step.response_info {
            obj["response"] = json!({
                "status": resp.status,
                "headers": resp.headers,
                "body": resp.body,
            });
        }
    }

    obj
}

/// Redact sensitive header values (Authorization, Cookie, etc.)
fn redact_secrets(headers: &HashMap<String, String>) -> HashMap<String, String> {
    let sensitive_keys = [
        "authorization",
        "cookie",
        "set-cookie",
        "x-api-key",
        "x-auth-token",
    ];

    headers
        .iter()
        .map(|(k, v)| {
            let key_lower = k.to_lowercase();
            if sensitive_keys.contains(&key_lower.as_str()) {
                (k.clone(), "***".to_string())
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::*;

    fn make_passing_run() -> RunResult {
        RunResult {
            duration_ms: 100,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Test".into(),
                passed: true,
                duration_ms: 100,
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
                    }],
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
                        }),
                        response_info: Some(ResponseInfo {
                            status: 404,
                            headers: HashMap::new(),
                            body: Some(json!({"error": "not_found"})),
                        }),
                        error_category: Some(FailureCategory::AssertionFailed),
                    }],
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
        let redacted = redact_secrets(&h);
        assert_eq!(redacted.get("Authorization").unwrap(), "***");
    }

    #[test]
    fn redact_cookie() {
        let mut h = HashMap::new();
        h.insert("Cookie".into(), "session=abc".into());
        let redacted = redact_secrets(&h);
        assert_eq!(redacted.get("Cookie").unwrap(), "***");
    }

    #[test]
    fn no_redaction_for_safe_headers() {
        let mut h = HashMap::new();
        h.insert("Content-Type".into(), "application/json".into());
        let redacted = redact_secrets(&h);
        assert_eq!(redacted.get("Content-Type").unwrap(), "application/json");
    }

    #[test]
    fn redact_case_insensitive() {
        let mut h = HashMap::new();
        h.insert("authorization".into(), "Bearer token".into());
        let redacted = redact_secrets(&h);
        assert_eq!(redacted.get("authorization").unwrap(), "***");
    }

    #[test]
    fn json_includes_failure_category() {
        let output = render(&make_failing_run());
        let parsed: Value = serde_json::from_str(&output).unwrap();
        let step = &parsed["files"][0]["tests"][0]["steps"][0];
        assert_eq!(step["failure_category"], "assertion_failed");
    }
}
