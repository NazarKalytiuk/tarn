use serde_json::Value;
use std::path::Path;
use tarn::assert::types::RunResult;
use tarn::config;
use tarn::env;
use tarn::fix_plan::generate_fix_plan_from_report;
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
        cookie_jar_per_test: false,
        fail_fast_within_test: false,
        // Fixture writing is a CLI-facing feature (NAZ-252). The MCP
        // tool path drives runs non-interactively against arbitrary
        // files and has no stable workspace root to anchor fixtures
        // under, so we leave the writer disabled by default.
        fixtures: tarn::report::fixture_writer::FixtureWriteConfig::default(),
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

/// Backing handler for the MCP `tarn_fix_plan` tool.
///
/// Since NAZ-305 (Phase L3.4), the actual report-to-plan lowering lives
/// in `tarn::fix_plan::generate_fix_plan_from_report` so the MCP surface
/// and the LSP Quick Fix code action share one source of truth. This
/// wrapper keeps the MCP tool's original JSON output shape — the one
/// pinned by `tests/golden/fix-plan.json.golden` — and just delegates
/// the heavy lifting to the library.
pub fn tarn_fix_plan(params: &Value) -> Result<Value, String> {
    let report = if let Some(report) = params.get("report") {
        report.clone()
    } else {
        tarn_run(params)?
    };

    if report
        .get("files")
        .and_then(|value| value.as_array())
        .is_none()
    {
        return Err("Invalid Tarn report: missing `files` array".to_string());
    }

    let max_items = params
        .get("max_items")
        .and_then(|value| value.as_u64())
        .unwrap_or(10) as usize;

    let items = generate_fix_plan_from_report(&report, max_items);
    let items_json: Vec<Value> = items.iter().map(|item| item.to_json()).collect();

    let next_action = items
        .first()
        .map(|item| item.summary.clone())
        .unwrap_or_else(|| "No failing steps. The report is already passing.".to_string());

    Ok(serde_json::json!({
        "status": report
            .get("summary")
            .and_then(|summary| summary.get("status"))
            .cloned()
            .unwrap_or(Value::String("UNKNOWN".into())),
        "failed_steps": items.len(),
        "next_action": next_action,
        "items": items_json,
    }))
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
