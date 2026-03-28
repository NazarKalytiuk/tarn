use serde_json::Value;
use std::path::Path;
use tarn::assert::types::RunResult;
use tarn::env;
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
    };

    let start = std::time::Instant::now();
    let mut file_results = Vec::new();

    for file_path in &files {
        let fp = Path::new(file_path);
        let test_file = parser::parse_file(fp).map_err(|e| e.to_string())?;

        let base_dir = fp.parent().unwrap_or(Path::new("."));
        let resolved_env = env::resolve_env(&test_file.env, env_name, &cli_vars, base_dir)
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
