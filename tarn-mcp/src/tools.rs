use serde_json::Value;
use std::path::Path;
use tarn::assert::types::RunResult;
use tarn::config;
use tarn::env;
use tarn::model::HttpTransportConfig;
use tarn::parser;
use tarn::report;
use tarn::runner::{self, RunOptions};

/// Execute tarn_run: parse, resolve env, run tests, return JSON results.
pub fn tarn_run(params: &Value) -> Result<Value, String> {
    let path_str = params
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("tests");

    let env_name = params.get("env").and_then(|v| v.as_str());
    let tag_str = params.get("tag").and_then(|v| v.as_str()).unwrap_or("");

    // Parse vars from object
    let cli_vars: Vec<(String, String)> = params
        .get("vars")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    let tag_filter = if tag_str.is_empty() {
        vec![]
    } else {
        runner::parse_tag_filter(tag_str)
    };

    let path = Path::new(path_str);
    let files = if path.is_file() {
        vec![path_str.to_string()]
    } else if path.is_dir() {
        runner::discover_test_files(path).map_err(|e| e.to_string())?
    } else {
        return Err(format!("Path not found: {}", path_str));
    };

    if files.is_empty() {
        return Err("No .tarn.yaml files found".to_string());
    }

    let opts = RunOptions {
        verbose: false,
        dry_run: false,
        http: HttpTransportConfig::default(),
    };

    let start = std::time::Instant::now();
    let mut file_results = Vec::new();

    for file_path in &files {
        let fp = Path::new(file_path);
        let test_file = parser::parse_file(fp).map_err(|e| e.to_string())?;

        let start_dir = fp.parent().unwrap_or(Path::new("."));
        let abs_start = if start_dir.is_absolute() {
            start_dir.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(start_dir)
        };
        let root_dir = config::find_project_root(&abs_start).unwrap_or(abs_start);
        let project_config = config::load_config(&root_dir).map_err(|e| e.to_string())?;
        let resolved_env = env::resolve_env_with_profiles(
            &test_file.env,
            env_name,
            &cli_vars,
            &root_dir,
            &project_config.env_file,
            &project_config.environments,
        )
        .map_err(|e| e.to_string())?;

        let result = runner::run_file(&test_file, file_path, &resolved_env, &tag_filter, &opts)
            .map_err(|e| e.to_string())?;

        file_results.push(result);
    }

    let run_result = RunResult {
        file_results,
        duration_ms: start.elapsed().as_millis() as u64,
    };

    let json_output = report::render(&run_result, report::OutputFormat::Json);
    let parsed: Value = serde_json::from_str(&json_output)
        .map_err(|e| format!("Failed to parse JSON output: {}", e))?;

    Ok(parsed)
}

/// Validate .tarn.yaml files without running them.
pub fn tarn_validate(params: &Value) -> Result<Value, String> {
    let path_str = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: path")?;

    let path = Path::new(path_str);
    let files = if path.is_file() {
        vec![path_str.to_string()]
    } else if path.is_dir() {
        runner::discover_test_files(path).map_err(|e| e.to_string())?
    } else {
        return Err(format!("Path not found: {}", path_str));
    };

    let mut results = Vec::new();

    for file_path in &files {
        let fp = Path::new(file_path);
        match parser::parse_file(fp) {
            Ok(_) => {
                results.push(serde_json::json!({
                    "file": file_path,
                    "valid": true,
                }));
            }
            Err(e) => {
                results.push(serde_json::json!({
                    "file": file_path,
                    "valid": false,
                    "error": e.to_string(),
                }));
            }
        }
    }

    let all_valid = results.iter().all(|r| r["valid"] == true);

    Ok(serde_json::json!({
        "valid": all_valid,
        "files": results,
    }))
}

/// List available tests from .tarn.yaml files.
pub fn tarn_list(params: &Value) -> Result<Value, String> {
    let path_str = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");

    let path = Path::new(path_str);
    let files = if path.is_file() {
        vec![path_str.to_string()]
    } else if path.is_dir() {
        runner::discover_test_files(path).map_err(|e| e.to_string())?
    } else {
        return Err(format!("Path not found: {}", path_str));
    };

    let mut file_list = Vec::new();

    for file_path in &files {
        let fp = Path::new(file_path);
        match parser::parse_file(fp) {
            Ok(tf) => {
                let mut tests = Vec::new();

                if !tf.steps.is_empty() {
                    tests.push(serde_json::json!({
                        "name": "(flat steps)",
                        "steps": tf.steps.iter().map(|s| &s.name).collect::<Vec<_>>(),
                    }));
                }

                for (name, group) in &tf.tests {
                    tests.push(serde_json::json!({
                        "name": name,
                        "description": group.description,
                        "tags": group.tags,
                        "steps": group.steps.iter().map(|s| &s.name).collect::<Vec<_>>(),
                    }));
                }

                file_list.push(serde_json::json!({
                    "file": file_path,
                    "name": tf.name,
                    "tags": tf.tags,
                    "tests": tests,
                }));
            }
            Err(e) => {
                file_list.push(serde_json::json!({
                    "file": file_path,
                    "error": e.to_string(),
                }));
            }
        }
    }

    Ok(serde_json::json!({
        "files": file_list,
    }))
}

pub fn tarn_fix_plan(params: &Value) -> Result<Value, String> {
    let report = if let Some(report) = params.get("report") {
        report.clone()
    } else {
        tarn_run(params)?
    };

    let files = report
        .get("files")
        .and_then(|value| value.as_array())
        .ok_or("Invalid Tarn report: missing `files` array")?;
    let max_items = params
        .get("max_items")
        .and_then(|value| value.as_u64())
        .unwrap_or(10) as usize;

    let mut items = Vec::new();

    for file in files {
        let file_name = file
            .get("file")
            .and_then(|value| value.as_str())
            .unwrap_or("");

        for step in file
            .get("setup")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
        {
            if let Some(item) = plan_item(file_name, "setup", step) {
                items.push(item);
            }
        }

        for test in file
            .get("tests")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
        {
            let test_name = test
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("test");
            for step in test
                .get("steps")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
            {
                if let Some(item) = plan_item(file_name, test_name, step) {
                    items.push(item);
                }
            }
        }

        for step in file
            .get("teardown")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
        {
            if let Some(item) = plan_item(file_name, "teardown", step) {
                items.push(item);
            }
        }
    }

    items.sort_by_key(|item| {
        (
            item.get("priority_rank")
                .and_then(|value| value.as_u64())
                .unwrap_or(99),
            item.get("file")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            item.get("step")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
        )
    });
    items.truncate(max_items);

    let next_action = items
        .first()
        .and_then(|item| item.get("summary"))
        .and_then(|value| value.as_str())
        .unwrap_or("No failing steps. The report is already passing.")
        .to_string();

    Ok(serde_json::json!({
        "status": report
            .get("summary")
            .and_then(|summary| summary.get("status"))
            .cloned()
            .unwrap_or(Value::String("UNKNOWN".into())),
        "failed_steps": items.len(),
        "next_action": next_action,
        "items": items,
    }))
}

fn plan_item(file_name: &str, scope: &str, step: &Value) -> Option<Value> {
    if step.get("status")?.as_str()? != "FAILED" {
        return None;
    }

    let failure_category = step
        .get("failure_category")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let error_code = step
        .get("error_code")
        .and_then(|value| value.as_str())
        .unwrap_or(failure_category);
    let failed_assertions = step
        .get("assertions")
        .and_then(|value| value.get("failures"))
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let remediation_hints = step
        .get("remediation_hints")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    Some(serde_json::json!({
        "file": file_name,
        "scope": scope,
        "step": step.get("name").cloned().unwrap_or(Value::String("unknown".into())),
        "failure_category": failure_category,
        "error_code": error_code,
        "priority": priority_label(failure_category),
        "priority_rank": priority_rank(failure_category),
        "summary": summary_text(failure_category, error_code, &failed_assertions),
        "actions": remediation_hints,
        "evidence": {
            "request_url": step.get("request").and_then(|request| request.get("url")).cloned(),
            "response_status": step.get("response").and_then(|response| response.get("status")).cloned(),
            "failed_assertions": failed_assertions,
        }
    }))
}

fn priority_rank(category: &str) -> u64 {
    match category {
        "parse_error" => 1,
        "connection_error" => 2,
        "timeout" => 3,
        "capture_error" => 4,
        "assertion_failed" => 5,
        _ => 9,
    }
}

fn priority_label(category: &str) -> &'static str {
    match priority_rank(category) {
        1 | 2 => "high",
        3 | 4 => "medium",
        _ => "normal",
    }
}

fn summary_text(category: &str, error_code: &str, failed_assertions: &[Value]) -> String {
    if let Some(message) = failed_assertions
        .first()
        .and_then(|failure| failure.get("message"))
        .and_then(|value| value.as_str())
    {
        return message.to_string();
    }

    match category {
        "connection_error" => format!("Connectivity issue detected ({error_code})."),
        "timeout" => format!("Operation timed out ({error_code})."),
        "capture_error" => format!("Capture extraction failed ({error_code})."),
        "parse_error" => format!("Test definition or interpolation issue detected ({error_code})."),
        _ => format!("Test step failed ({error_code})."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tarn_fix_plan_matches_golden_contract() {
        let report: Value = serde_json::json!({
            "summary": { "status": "FAILED" },
            "files": [{
                "file": "tests/users.tarn.yaml",
                "setup": [],
                "tests": [{
                    "name": "smoke",
                    "steps": [{
                        "name": "Create user",
                        "status": "FAILED",
                        "failure_category": "assertion_failed",
                        "error_code": "assertion_mismatch",
                        "remediation_hints": [
                            "Inspect `assertions.failures` expected vs actual values and update the DSL or the service response.",
                            "Use the recorded `response` payload to realign assertions and captures with the actual API output."
                        ],
                        "assertions": {
                            "failures": [{
                                "assertion": "status",
                                "expected": "201",
                                "actual": "400",
                                "message": "Expected HTTP status 201, got 400"
                            }]
                        },
                        "request": {
                            "url": "https://api.example.test/users"
                        },
                        "response": {
                            "status": 400
                        }
                    }]
                }],
                "teardown": []
            }]
        });

        let actual = tarn_fix_plan(&serde_json::json!({ "report": report })).unwrap();
        let expected: Value =
            serde_json::from_str(include_str!("../tests/golden/fix-plan.json.golden")).unwrap();
        assert_eq!(actual, expected);
    }
}
