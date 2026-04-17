use serde_json::Value;
use std::path::{Path, PathBuf};
use tarn::assert::types::RunResult;
use tarn::config;
use tarn::env;
use tarn::fix_plan::generate_fix_plan_from_report;
use tarn::model::HttpTransportConfig;
use tarn::parser;
use tarn::report;
use tarn::runner::{self, RunOptions};

/// Resolved working directory for an MCP tool call, plus a flag telling
/// downstream code whether the caller pinned it explicitly.
///
/// The distinction matters for error handling: when the caller passes
/// `cwd` we refuse to silently fall back to the process cwd if the
/// directory does not contain `tarn.config.yaml` (requirement #4). When
/// the caller did *not* pass `cwd`, we use whatever workspace root the
/// MCP client announced during `initialize` (if any), or finally the
/// process cwd — and a missing `tarn.config.yaml` is tolerated there
/// because the default behaviour must stay backwards-compatible with
/// how `tarn-mcp` worked before NAZ-248.
#[derive(Debug)]
pub struct ResolvedCwd {
    pub path: PathBuf,
    pub explicit: bool,
}

/// Resolve the working directory for an MCP tool call.
///
/// Priority (highest first):
///   1. `params.cwd` — must be an absolute path that exists
///   2. the workspace root captured from the MCP `initialize` handshake
///   3. the process `current_dir()`
///
/// Returns `Err` for a malformed explicit `cwd` (relative, non-string,
/// non-directory). A relative-path rejection keeps the contract
/// predictable: MCP clients run anywhere on disk, and resolving a
/// relative `cwd` against the *server's* process cwd would defeat the
/// whole point of this parameter.
pub fn resolve_cwd(params: &Value) -> Result<ResolvedCwd, String> {
    if let Some(raw) = params.get("cwd") {
        let cwd_str = raw
            .as_str()
            .ok_or_else(|| "Parameter `cwd` must be a string".to_string())?;
        let path = PathBuf::from(cwd_str);
        if !path.is_absolute() {
            return Err(format!(
                "Parameter `cwd` must be an absolute path, got: {}",
                cwd_str
            ));
        }
        if !path.exists() {
            return Err(format!("`cwd` does not exist: {}", path.display()));
        }
        if !path.is_dir() {
            return Err(format!("`cwd` is not a directory: {}", path.display()));
        }
        return Ok(ResolvedCwd {
            path,
            explicit: true,
        });
    }

    if let Some(root) = crate::protocol::workspace_root() {
        return Ok(ResolvedCwd {
            path: root,
            explicit: false,
        });
    }

    let cwd = std::env::current_dir()
        .map_err(|e| format!("Failed to read process current_dir: {}", e))?;
    Ok(ResolvedCwd {
        path: cwd,
        explicit: false,
    })
}

/// Resolve a user-supplied path (which may be relative) against the
/// resolved working directory. Absolute paths are returned unchanged so
/// callers can always pass one when the MCP client has no workspace
/// context.
fn resolve_path_against_cwd(path_str: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(path_str);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// When the caller passed an explicit `cwd`, refuse to run unless that
/// directory actually contains `tarn.config.yaml`. This is requirement
/// #4 of NAZ-248: never silently default if the user pinned the
/// workspace, and always name the path we looked at so the agent can
/// correct its tool call without extra round-trips.
fn require_config_for_explicit_cwd(resolved: &ResolvedCwd) -> Result<(), String> {
    if !resolved.explicit {
        return Ok(());
    }
    let config_path = resolved.path.join("tarn.config.yaml");
    if config_path.exists() {
        Ok(())
    } else {
        Err(format!(
            "tarn.config.yaml not found at {} (resolved from explicit `cwd` parameter). \
             Pass the workspace root that contains tarn.config.yaml.",
            config_path.display()
        ))
    }
}

/// Expand a user-supplied `path` into a list of `.tarn.yaml` files.
/// Relative paths are resolved against `cwd`. When `path` is a bare
/// directory and relative, discovery is rooted at `cwd.join(path)` so
/// MCP clients can ask for `"tests"` without knowing the filesystem
/// layout the server was launched from.
fn expand_test_files(path_str: &str, cwd: &Path) -> Result<Vec<String>, String> {
    let resolved = resolve_path_against_cwd(path_str, cwd);
    if resolved.is_file() {
        Ok(vec![resolved.to_string_lossy().into_owned()])
    } else if resolved.is_dir() {
        runner::discover_test_files(&resolved).map_err(|e| e.to_string())
    } else {
        Err(format!("Path not found: {}", resolved.display()))
    }
}

/// Execute tarn_run: parse, resolve env, run tests, return JSON results.
pub fn tarn_run(params: &Value) -> Result<Value, String> {
    let cwd = resolve_cwd(params)?;
    require_config_for_explicit_cwd(&cwd)?;

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

    let files = expand_test_files(path_str, &cwd.path)?;

    if files.is_empty() {
        return Err("No .tarn.yaml files found".to_string());
    }

    let opts = RunOptions {
        verbose: false,
        dry_run: false,
        http: HttpTransportConfig::default(),
        cookie_jar_per_test: false,
        fail_fast_within_test: false,
        verbose_responses: false,
        max_body_bytes: runner::DEFAULT_MAX_BODY_BYTES,
    };

    let start = std::time::Instant::now();
    let mut file_results = Vec::new();

    for file_path in &files {
        let fp = Path::new(file_path);
        let test_file = parser::parse_file(fp).map_err(|e| e.to_string())?;

        // Use the resolved MCP cwd as the root for config + env lookup.
        // Previously this walked up from the test file's parent, which
        // broke when the MCP server was launched outside the project
        // (no `tarn.config.yaml` / `tarn.env.yaml` in sight). With an
        // explicit cwd we honour the caller; without one we still call
        // `find_project_root` so the ergonomic default (run from inside
        // the project) keeps working.
        let root_dir = if cwd.explicit {
            cwd.path.clone()
        } else {
            config::find_project_root(&cwd.path).unwrap_or_else(|| cwd.path.clone())
        };
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
    let cwd = resolve_cwd(params)?;
    require_config_for_explicit_cwd(&cwd)?;

    let path_str = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: path")?;

    let files = expand_test_files(path_str, &cwd.path)?;

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
    let cwd = resolve_cwd(params)?;
    require_config_for_explicit_cwd(&cwd)?;

    let path_str = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");

    let files = expand_test_files(path_str, &cwd.path)?;

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
        // NAZ-248: `cwd` is forwarded to `tarn_run` through the raw
        // params object, so we do not need a second resolution pass
        // here. The validation still runs inside `tarn_run` and will
        // surface an error before any files are touched.
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

    #[test]
    fn resolve_cwd_rejects_relative_path() {
        let params = serde_json::json!({ "cwd": "relative/path" });
        let err = resolve_cwd(&params).unwrap_err();
        assert!(err.contains("absolute"), "got: {}", err);
    }

    #[test]
    fn resolve_cwd_rejects_non_string() {
        let params = serde_json::json!({ "cwd": 42 });
        let err = resolve_cwd(&params).unwrap_err();
        assert!(err.contains("must be a string"), "got: {}", err);
    }

    #[test]
    fn resolve_cwd_rejects_missing_directory() {
        let params = serde_json::json!({ "cwd": "/definitely/not/a/real/tarn/cwd/xyzzy" });
        let err = resolve_cwd(&params).unwrap_err();
        assert!(err.contains("does not exist"), "got: {}", err);
    }

    #[test]
    fn resolve_cwd_accepts_explicit_existing_absolute_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let params = serde_json::json!({ "cwd": tmp.path().to_string_lossy() });
        let resolved = resolve_cwd(&params).unwrap();
        assert!(resolved.explicit);
        assert_eq!(resolved.path, tmp.path());
    }

    #[test]
    fn resolve_cwd_defaults_to_process_cwd_when_absent() {
        let params = serde_json::json!({});
        let resolved = resolve_cwd(&params).unwrap();
        assert!(!resolved.explicit);
        // Must be absolute and exist.
        assert!(resolved.path.is_absolute());
        assert!(resolved.path.exists());
    }

    #[test]
    fn require_config_for_explicit_cwd_errors_when_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let resolved = ResolvedCwd {
            path: tmp.path().to_path_buf(),
            explicit: true,
        };
        let err = require_config_for_explicit_cwd(&resolved).unwrap_err();
        assert!(err.contains("tarn.config.yaml"), "got: {}", err);
        assert!(err.contains(&tmp.path().display().to_string()), "got: {}", err);
    }

    #[test]
    fn require_config_for_explicit_cwd_ok_when_present() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("tarn.config.yaml"), "test_dir: tests\n").unwrap();
        let resolved = ResolvedCwd {
            path: tmp.path().to_path_buf(),
            explicit: true,
        };
        require_config_for_explicit_cwd(&resolved).unwrap();
    }

    #[test]
    fn require_config_for_explicit_cwd_skips_check_when_default() {
        let tmp = tempfile::TempDir::new().unwrap();
        let resolved = ResolvedCwd {
            path: tmp.path().to_path_buf(),
            explicit: false,
        };
        // No config present, but this is a default cwd — must pass.
        require_config_for_explicit_cwd(&resolved).unwrap();
    }

    #[test]
    fn resolve_path_against_cwd_joins_relative() {
        let cwd = std::path::Path::new("/tmp/workspace");
        let joined = resolve_path_against_cwd("tests/x.tarn.yaml", cwd);
        assert_eq!(joined, std::path::PathBuf::from("/tmp/workspace/tests/x.tarn.yaml"));
    }

    #[test]
    fn resolve_path_against_cwd_preserves_absolute() {
        let cwd = std::path::Path::new("/tmp/workspace");
        let joined = resolve_path_against_cwd("/other/file.tarn.yaml", cwd);
        assert_eq!(joined, std::path::PathBuf::from("/other/file.tarn.yaml"));
    }
}
