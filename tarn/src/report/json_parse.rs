//! Reverse direction of `report/json.rs`: parse a previously-emitted
//! Tarn JSON report back into a [`RunResult`] so other formatters can
//! re-render it.
//!
//! Used by `tarn summary <run.json>` to produce compact/llm output
//! from a stored report without re-running the tests. Only the fields
//! the downstream formatters care about are rehydrated; fields that
//! would require re-executing HTTP requests (raw response bytes,
//! timings beyond `duration_ms`) are approximated from what the JSON
//! already contains.

use crate::assert::types::{
    AssertionResult, FailureCategory, FileResult, RequestInfo, ResponseInfo, RunResult, StepResult,
    TestResult,
};
use crate::model::{Location, RedactionConfig};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// Parse a JSON report into a `RunResult`. Returns `Err` when the
/// report isn't shaped like a tarn v1 report (wrong top-level type,
/// missing `files` array, etc.).
pub fn parse(input: &str) -> Result<RunResult, ParseError> {
    let value: Value =
        serde_json::from_str(input).map_err(|e| ParseError(format!("invalid JSON: {}", e)))?;

    let obj = value
        .as_object()
        .ok_or_else(|| ParseError("expected top-level JSON object".into()))?;

    let files_value = obj
        .get("files")
        .ok_or_else(|| ParseError("missing `files` array".into()))?;
    let files_array = files_value
        .as_array()
        .ok_or_else(|| ParseError("`files` must be an array".into()))?;

    let mut file_results = Vec::with_capacity(files_array.len());
    for (idx, file_value) in files_array.iter().enumerate() {
        let parsed = parse_file(file_value)
            .map_err(|e| ParseError(format!("files[{}]: {}", idx, e.0)))?;
        file_results.push(parsed);
    }

    let duration_ms = obj
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    Ok(RunResult {
        file_results,
        duration_ms,
    })
}

fn parse_file(value: &Value) -> Result<FileResult, ParseError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ParseError("expected file object".into()))?;

    let file = string_field(obj, "file").unwrap_or_default();
    let name = string_field(obj, "name").unwrap_or_else(|| file.clone());
    let passed = status_to_passed(obj.get("status"));
    let duration_ms = obj
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let setup_results = parse_step_array(obj.get("setup"))?;
    let teardown_results = parse_step_array(obj.get("teardown"))?;
    let test_results = match obj.get("tests").and_then(Value::as_array) {
        Some(tests) => tests
            .iter()
            .map(parse_test)
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };

    Ok(FileResult {
        file,
        name,
        passed,
        duration_ms,
        redaction: RedactionConfig::default(),
        redacted_values: Vec::new(),
        setup_results,
        test_results,
        teardown_results,
    })
}

fn parse_test(value: &Value) -> Result<TestResult, ParseError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ParseError("expected test object".into()))?;

    let name = string_field(obj, "name").unwrap_or_default();
    let description = string_field(obj, "description");
    let passed = status_to_passed(obj.get("status"));
    let duration_ms = obj
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let captures: HashMap<String, Value> = obj
        .get("captures")
        .and_then(Value::as_object)
        .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    let step_results = parse_step_array(obj.get("steps"))?;

    Ok(TestResult {
        name,
        description,
        passed,
        duration_ms,
        step_results,
        captures,
    })
}

fn parse_step_array(value: Option<&Value>) -> Result<Vec<StepResult>, ParseError> {
    match value.and_then(Value::as_array) {
        Some(array) => array.iter().map(parse_step).collect(),
        None => Ok(Vec::new()),
    }
}

fn parse_step(value: &Value) -> Result<StepResult, ParseError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ParseError("expected step object".into()))?;

    let name = string_field(obj, "name").unwrap_or_default();
    let description = string_field(obj, "description");
    let passed = status_to_passed(obj.get("status"));
    let duration_ms = obj
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let response_status = obj
        .get("response_status")
        .and_then(Value::as_u64)
        .map(|n| n as u16);
    let response_summary = string_field(obj, "response_summary");
    let captures_set = obj
        .get("captures_set")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default();

    let assertion_results = obj
        .get("assertions")
        .and_then(Value::as_object)
        .and_then(|a| a.get("details"))
        .and_then(Value::as_array)
        .map(|details| details.iter().map(parse_assertion).collect::<Result<Vec<_>, _>>())
        .transpose()?
        .unwrap_or_default();

    let request_info = obj.get("request").and_then(parse_request);
    let response_info = obj.get("response").and_then(parse_response);

    let error_category = obj
        .get("failure_category")
        .and_then(Value::as_str)
        .and_then(parse_failure_category);

    let location = obj.get("location").and_then(parse_location);

    Ok(StepResult {
        name,
        description,
        debug: false,
        passed,
        duration_ms,
        assertion_results,
        request_info,
        response_info,
        error_category,
        response_status,
        response_summary,
        captures_set,
        location,
    })
}

fn parse_assertion(value: &Value) -> Result<AssertionResult, ParseError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ParseError("expected assertion object".into()))?;
    let assertion = string_field(obj, "assertion").unwrap_or_default();
    let passed = obj.get("passed").and_then(Value::as_bool).unwrap_or(false);
    let expected = string_field(obj, "expected").unwrap_or_default();
    let actual = string_field(obj, "actual").unwrap_or_default();
    let message = string_field(obj, "message").unwrap_or_default();
    let diff = string_field(obj, "diff");
    let location = obj.get("location").and_then(parse_location);

    Ok(AssertionResult {
        assertion,
        passed,
        expected,
        actual,
        message,
        diff,
        location,
    })
}

fn parse_request(value: &Value) -> Option<RequestInfo> {
    let obj = value.as_object()?;
    let method = string_field(obj, "method")?;
    let url = string_field(obj, "url")?;
    let headers = parse_headers(obj.get("headers"));
    let body = obj.get("body").cloned().filter(|v| !v.is_null());
    Some(RequestInfo {
        method,
        url,
        headers,
        body,
        multipart: None,
    })
}

fn parse_response(value: &Value) -> Option<ResponseInfo> {
    let obj = value.as_object()?;
    let status = obj.get("status").and_then(Value::as_u64)? as u16;
    let headers = parse_headers(obj.get("headers"));
    let body = obj.get("body").cloned().filter(|v| !v.is_null());
    Some(ResponseInfo {
        status,
        headers,
        body,
    })
}

fn parse_headers(value: Option<&Value>) -> HashMap<String, String> {
    let Some(Value::Object(map)) = value else {
        return HashMap::new();
    };
    map.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
}

fn parse_location(value: &Value) -> Option<Location> {
    let obj = value.as_object()?;
    Some(Location {
        file: string_field(obj, "file")?,
        line: obj.get("line").and_then(Value::as_u64)? as usize,
        column: obj.get("column").and_then(Value::as_u64)? as usize,
    })
}

fn parse_failure_category(s: &str) -> Option<FailureCategory> {
    Some(match s {
        "assertion_failed" => FailureCategory::AssertionFailed,
        "connection_error" => FailureCategory::ConnectionError,
        "timeout" => FailureCategory::Timeout,
        "parse_error" => FailureCategory::ParseError,
        "capture_error" => FailureCategory::CaptureError,
        "unresolved_template" => FailureCategory::UnresolvedTemplate,
        "skipped_due_to_failed_capture" => FailureCategory::SkippedDueToFailedCapture,
        "skipped_due_to_fail_fast" => FailureCategory::SkippedDueToFailFast,
        _ => return None,
    })
}

fn status_to_passed(value: Option<&Value>) -> bool {
    matches!(value.and_then(Value::as_str), Some("PASSED"))
}

fn string_field(obj: &serde_json::Map<String, Value>, name: &str) -> Option<String> {
    obj.get(name)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::json::render;

    fn build_run() -> RunResult {
        let mut tr = TestResult {
            name: "t1".into(),
            description: Some("desc".into()),
            passed: false,
            duration_ms: 100,
            step_results: vec![StepResult {
                name: "bad".into(),
                description: None,
                debug: false,
                passed: false,
                duration_ms: 50,
                assertion_results: vec![AssertionResult::fail(
                    "status",
                    "200",
                    "500",
                    "Expected 200, got 500",
                )],
                request_info: Some(RequestInfo {
                    method: "GET".into(),
                    url: "/foo".into(),
                    headers: HashMap::new(),
                    body: None,
                    multipart: None,
                }),
                response_info: Some(ResponseInfo {
                    status: 500,
                    headers: HashMap::new(),
                    body: Some(serde_json::json!({"err": "x"})),
                }),
                error_category: Some(FailureCategory::AssertionFailed),
                response_status: Some(500),
                response_summary: None,
                captures_set: vec!["id".into()],
                location: None,
            }],
            captures: HashMap::new(),
        };
        tr.captures
            .insert("token".into(), serde_json::json!("abc"));

        RunResult {
            duration_ms: 200,
            file_results: vec![FileResult {
                file: "x.tarn.yaml".into(),
                name: "x".into(),
                passed: false,
                duration_ms: 200,
                redaction: RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![tr],
                teardown_results: vec![],
            }],
        }
    }

    #[test]
    fn round_trip_preserves_fail_status_and_duration() {
        let run = build_run();
        let json = render(&run);
        let parsed = parse(&json).unwrap();
        assert_eq!(parsed.duration_ms, run.duration_ms);
        assert_eq!(parsed.total_files(), 1);
        assert_eq!(parsed.failed_steps(), 1);
        assert_eq!(parsed.passed_steps(), 0);
    }

    #[test]
    fn round_trip_preserves_assertion_and_request() {
        let run = build_run();
        let json = render(&run);
        let parsed = parse(&json).unwrap();
        let step = &parsed.file_results[0].test_results[0].step_results[0];
        assert_eq!(step.assertion_results.len(), 1);
        assert_eq!(step.assertion_results[0].assertion, "status");
        assert_eq!(step.request_info.as_ref().unwrap().url, "/foo");
        assert_eq!(step.response_info.as_ref().unwrap().status, 500);
        assert_eq!(step.response_status, Some(500));
        assert_eq!(step.error_category, Some(FailureCategory::AssertionFailed));
    }

    #[test]
    fn round_trip_preserves_captures_and_captures_set() {
        let run = build_run();
        let json = render(&run);
        let parsed = parse(&json).unwrap();
        let test = &parsed.file_results[0].test_results[0];
        assert_eq!(
            test.captures.get("token"),
            Some(&serde_json::json!("abc"))
        );
        assert_eq!(
            test.step_results[0].captures_set,
            vec!["id".to_string()]
        );
    }

    #[test]
    fn parse_rejects_invalid_json() {
        assert!(parse("not json").is_err());
    }

    #[test]
    fn parse_rejects_missing_files_array() {
        let err = parse("{\"duration_ms\":0}").unwrap_err();
        assert!(err.to_string().contains("files"));
    }
}
