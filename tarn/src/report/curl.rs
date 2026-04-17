use crate::assert::types::{FileResult, RequestInfo, RunResult, StepResult};
use crate::http;
use crate::report::redaction::{redact_headers, sanitize_json, sanitize_string};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn render_failures(result: &RunResult) -> String {
    render(result, false)
}

pub fn render_all(result: &RunResult) -> String {
    render(result, true)
}

fn render(result: &RunResult, include_passed: bool) -> String {
    let mut output = String::from("#!/bin/sh\nset -eu\n");
    let mut exported = 0usize;

    for file in &result.file_results {
        for step in &file.setup_results {
            exported += render_step(&mut output, file, "setup", step, include_passed);
        }
        for test in &file.test_results {
            for step in &test.step_results {
                exported += render_step(&mut output, file, &test.name, step, include_passed);
            }
        }
        for step in &file.teardown_results {
            exported += render_step(&mut output, file, "teardown", step, include_passed);
        }
    }

    if exported == 0 {
        output.push_str("\n# No matching requests to export.\n");
    }

    output
}

fn render_step(
    output: &mut String,
    file: &FileResult,
    scope: &str,
    step: &StepResult,
    include_passed: bool,
) -> usize {
    if !include_passed && step.passed {
        return 0;
    }

    let Some(request) = step.request_info.as_ref() else {
        return 0;
    };

    output.push_str("\n# File: ");
    output.push_str(&file.file);
    output.push_str("\n# Scope: ");
    output.push_str(scope);
    output.push_str("\n# Step: ");
    output.push_str(&step.name);
    output.push_str("\n# Status: ");
    output.push_str(if step.passed { "PASSED" } else { "FAILED" });
    if let Some(category) = step.error_category {
        output.push_str("\n# Failure category: ");
        output.push_str(&format!("{category:?}").to_ascii_lowercase());
    }
    output.push('\n');
    output.push_str(&request_to_curl(
        request,
        &file.redaction,
        &file.redacted_values,
    ));
    output.push('\n');
    1
}

pub fn request_to_curl(
    request: &RequestInfo,
    redaction: &crate::model::RedactionConfig,
    secret_values: &[String],
) -> String {
    let mut lines = vec![
        "curl".to_string(),
        format!("  -X {}", shell_quote(&request.method)),
        format!(
            "  {}",
            shell_quote(&sanitize_string(
                &request.url,
                &redaction.replacement,
                secret_values,
            ))
        ),
    ];

    let headers = redact_headers(&request.headers, redaction, secret_values);
    append_headers(&mut lines, &headers);

    if let Some(multipart) = &request.multipart {
        for field in &multipart.fields {
            lines.push(format!(
                "  -F {}",
                shell_quote(&format!(
                    "{}={}",
                    field.name,
                    sanitize_string(&field.value, &redaction.replacement, secret_values)
                ))
            ));
        }
        for file in &multipart.files {
            let mut part = format!("{}=@{}", file.name, file.path);
            if let Some(filename) = &file.filename {
                part.push_str(&format!(";filename={}", filename));
            }
            if let Some(content_type) = &file.content_type {
                part.push_str(&format!(";type={}", content_type));
            }
            lines.push(format!("  -F {}", shell_quote(&part)));
        }
    } else if let Some(body) = &request.body {
        let body = render_body(body, &headers, redaction, secret_values);
        lines.push(format!("  --data-raw {}", shell_quote(&body)));
    }

    lines.join(" \\\n")
}

fn append_headers(lines: &mut Vec<String>, headers: &BTreeMap<String, String>) {
    for (name, value) in headers {
        lines.push(format!("  -H {}", shell_quote(&format!("{name}: {value}"))));
    }
}

fn render_body(
    body: &Value,
    headers: &BTreeMap<String, String>,
    redaction: &crate::model::RedactionConfig,
    secret_values: &[String],
) -> String {
    if is_form_urlencoded(headers) {
        if let Some(form) = body.as_object() {
            let form = form
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value
                            .as_str()
                            .map(|s| sanitize_string(s, &redaction.replacement, secret_values))
                            .unwrap_or_else(|| value.to_string()),
                    )
                })
                .collect();
            if let Ok(encoded) = http::encode_form_body(&form) {
                return encoded;
            }
        }
    }

    match sanitize_json(body, &redaction.replacement, secret_values) {
        Value::String(text) => text,
        other => serde_json::to_string(&other).unwrap_or_else(|_| other.to_string()),
    }
}

fn is_form_urlencoded(headers: &BTreeMap<String, String>) -> bool {
    headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-type")
            && value
                .to_ascii_lowercase()
                .starts_with("application/x-www-form-urlencoded")
    })
}

fn shell_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::{AssertionResult, FileResult, RequestInfo, StepResult, TestResult};
    use crate::model::{FileField, FormField, MultipartBody, RedactionConfig};
    use serde_json::json;
    use std::collections::HashMap;

    fn make_file_result(step: StepResult) -> FileResult {
        FileResult {
            file: "tests/export.tarn.yaml".into(),
            name: "Export".into(),
            passed: step.passed,
            duration_ms: 10,
            redaction: RedactionConfig::default(),
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![TestResult {
                name: "suite".into(),
                description: None,
                passed: step.passed,
                duration_ms: 10,
                step_results: vec![step],
                captures: HashMap::new(),
            }],
            teardown_results: vec![],
        }
    }

    #[test]
    fn render_failures_skips_passing_steps() {
        let run = RunResult {
            duration_ms: 10,
            file_results: vec![make_file_result(StepResult {
                name: "ok".into(),
                description: None,
                debug: false,
                passed: true,
                duration_ms: 5,
                assertion_results: vec![AssertionResult::pass("status", "200", "200")],
                request_info: Some(RequestInfo {
                    method: "GET".into(),
                    url: "https://example.com/ok".into(),
                    headers: HashMap::new(),
                    body: None,
                    multipart: None,
                }),
                response_info: None,
                error_category: None,
                response_status: None,
                response_summary: None,
                captures_set: vec![],
                location: None,
            })],
        };

        let output = render_failures(&run);
        assert!(output.contains("No matching requests"));
        assert!(!output.contains("https://example.com/ok"));
    }

    #[test]
    fn render_all_includes_passing_steps() {
        let run = RunResult {
            duration_ms: 10,
            file_results: vec![make_file_result(StepResult {
                name: "ok".into(),
                description: None,
                debug: false,
                passed: true,
                duration_ms: 5,
                assertion_results: vec![AssertionResult::pass("status", "200", "200")],
                request_info: Some(RequestInfo {
                    method: "GET".into(),
                    url: "https://example.com/ok".into(),
                    headers: HashMap::new(),
                    body: None,
                    multipart: None,
                }),
                response_info: None,
                error_category: None,
                response_status: None,
                response_summary: None,
                captures_set: vec![],
                location: None,
            })],
        };

        let output = render_all(&run);
        assert!(output.contains("https://example.com/ok"));
        assert!(output.contains("# Status: PASSED"));
    }

    #[test]
    fn request_to_curl_renders_json_form_and_multipart() {
        let json_request = RequestInfo {
            method: "POST".into(),
            url: "https://example.com/users".into(),
            headers: HashMap::from([("Content-Type".into(), "application/json".into())]),
            body: Some(json!({"name": "Alice"})),
            multipart: None,
        };
        let json_command = request_to_curl(&json_request, &RedactionConfig::default(), &Vec::new());
        assert!(json_command.contains("--data-raw '{\"name\":\"Alice\"}'"));

        let form_request = RequestInfo {
            method: "POST".into(),
            url: "https://example.com/login".into(),
            headers: HashMap::from([(
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            )]),
            body: Some(json!({"email": "a@example.com", "password": "secret"})),
            multipart: None,
        };
        let form_command = request_to_curl(&form_request, &RedactionConfig::default(), &Vec::new());
        assert!(form_command.contains("--data-raw 'email=a%40example.com&password=secret'"));

        let multipart_request = RequestInfo {
            method: "POST".into(),
            url: "https://example.com/upload".into(),
            headers: HashMap::from([("X-Trace".into(), "trace".into())]),
            body: None,
            multipart: Some(MultipartBody {
                fields: vec![FormField {
                    name: "title".into(),
                    value: "Report".into(),
                }],
                files: vec![FileField {
                    name: "file".into(),
                    path: "/tmp/report.txt".into(),
                    content_type: Some("text/plain".into()),
                    filename: Some("report.txt".into()),
                }],
            }),
        };
        let multipart_command =
            request_to_curl(&multipart_request, &RedactionConfig::default(), &Vec::new());
        assert!(multipart_command.contains("-F 'title=Report'"));
        assert!(multipart_command
            .contains("-F 'file=@/tmp/report.txt;filename=report.txt;type=text/plain'"));
    }

    #[test]
    fn request_to_curl_redacts_headers_urls_and_body() {
        let request = RequestInfo {
            method: "POST".into(),
            url: "https://example.com/?token=secret-token".into(),
            headers: HashMap::from([("Authorization".into(), "Bearer secret-token".into())]),
            body: Some(json!({"token": "secret-token"})),
            multipart: None,
        };
        let redaction = RedactionConfig::default();
        let output = request_to_curl(&request, &redaction, &["secret-token".into()]);
        assert!(output.contains("https://example.com/?token=***"));
        assert!(output.contains("Authorization: ***"));
        assert!(output.contains("{\"token\":\"***\"}"));
    }
}
