use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tarn::assert::types::RunResult;
use tarn::config;
use tarn::env;
use tarn::fix_plan::generate_fix_plan_from_report;
use tarn::model::HttpTransportConfig;
use tarn::parser;
use tarn::report;
use tarn::report::summary::{FailuresDoc, SummaryDoc};
use tarn::runner::{self, RunOptions};

/// Structured MCP tool error.
///
/// Every tool handler returns `Result<Value, ToolError>`; the dispatcher
/// in `main.rs` renders the payload into either an MCP
/// `content: [{text, isError}]` envelope *or* a JSON-RPC `error` record,
/// depending on whether the caller used `tools/call` or a plain method
/// invocation. Carrying the triple `{code, message, data}` through every
/// handler keeps agents out of brittle string-matching on error text.
#[derive(Debug, Clone)]
pub struct ToolError {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

impl ToolError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Render the error as a JSON payload the MCP `tools/call` content
    /// block can embed. Keeps the code/message/data triple visible so
    /// agents parse one shape regardless of transport surface.
    pub fn to_tool_call_json(&self) -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("code".into(), Value::from(self.code));
        obj.insert("message".into(), Value::String(self.message.clone()));
        if let Some(data) = &self.data {
            obj.insert("data".into(), data.clone());
        }
        Value::Object(obj)
    }
}

// MCP error-code allocation (reserved block -32050 .. -32099 for tarn):
// JSON-RPC leaves -32000 .. -32099 for implementation-defined server
// errors, so every code below lives in that window. Keeping codes
// documented in one place avoids drift between handler sites.
pub const ERR_INVALID_CWD: i32 = -32050;
pub const ERR_MISSING_CONFIG: i32 = -32051;
pub const ERR_MISSING_PARAM: i32 = -32052;
pub const ERR_INVALID_PARAM: i32 = -32053;
pub const ERR_PATH_NOT_FOUND: i32 = -32054;
pub const ERR_NO_TESTS: i32 = -32055;
pub const ERR_PARSE: i32 = -32056;
pub const ERR_RUN_FAILED: i32 = -32057;
pub const ERR_ARTIFACT_MISSING: i32 = -32058;
pub const ERR_ARTIFACT_PARSE: i32 = -32059;
pub const ERR_RUN_ID_UNKNOWN: i32 = -32060;
pub const ERR_RERUN_EMPTY: i32 = -32061;
pub const ERR_INVALID_REPORT: i32 = -32062;
pub const ERR_INSPECT_FAILED: i32 = -32063;

/// Resolved working directory for an MCP tool call, plus a flag telling
/// downstream code whether the caller pinned it explicitly.
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
pub fn resolve_cwd(params: &Value) -> Result<ResolvedCwd, ToolError> {
    if let Some(raw) = params.get("cwd") {
        let cwd_str = raw.as_str().ok_or_else(|| {
            ToolError::new(ERR_INVALID_PARAM, "Parameter `cwd` must be a string")
                .with_data(json!({ "param": "cwd", "got": raw }))
        })?;
        let path = PathBuf::from(cwd_str);
        if !path.is_absolute() {
            return Err(ToolError::new(
                ERR_INVALID_CWD,
                format!("Parameter `cwd` must be an absolute path, got: {}", cwd_str),
            )
            .with_data(json!({
                "cwd": cwd_str,
                "expected": "absolute path",
            })));
        }
        if !path.exists() {
            return Err(ToolError::new(
                ERR_INVALID_CWD,
                format!("`cwd` does not exist: {}", path.display()),
            )
            .with_data(json!({
                "cwd": path.display().to_string(),
                "expected": "existing directory",
            })));
        }
        if !path.is_dir() {
            return Err(ToolError::new(
                ERR_INVALID_CWD,
                format!("`cwd` is not a directory: {}", path.display()),
            )
            .with_data(json!({
                "cwd": path.display().to_string(),
                "expected": "directory",
            })));
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

    let cwd = std::env::current_dir().map_err(|e| {
        ToolError::new(
            ERR_INVALID_CWD,
            format!("Failed to read process current_dir: {}", e),
        )
    })?;
    Ok(ResolvedCwd {
        path: cwd,
        explicit: false,
    })
}

/// Resolve a user-supplied path (which may be relative) against the
/// resolved working directory. Absolute paths are returned unchanged.
fn resolve_path_against_cwd(path_str: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(path_str);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// When the caller passed an explicit `cwd`, refuse to run unless that
/// directory actually contains `tarn.config.yaml`. Explicitness signals
/// the agent knows which project it is pointing at; silently falling
/// back would mask a real misconfiguration.
fn require_config_for_explicit_cwd(resolved: &ResolvedCwd) -> Result<(), ToolError> {
    if !resolved.explicit {
        return Ok(());
    }
    let config_path = resolved.path.join("tarn.config.yaml");
    if config_path.exists() {
        Ok(())
    } else {
        Err(ToolError::new(
            ERR_MISSING_CONFIG,
            format!(
                "tarn.config.yaml not found at {} (resolved from explicit `cwd` parameter). \
                 Pass the workspace root that contains tarn.config.yaml.",
                config_path.display()
            ),
        )
        .with_data(json!({
            "cwd": resolved.path.display().to_string(),
            "expected": config_path.display().to_string(),
        })))
    }
}

/// Expand a user-supplied `path` into a list of `.tarn.yaml` files.
fn expand_test_files(path_str: &str, cwd: &Path) -> Result<Vec<String>, ToolError> {
    let resolved = resolve_path_against_cwd(path_str, cwd);
    if resolved.is_file() {
        Ok(vec![resolved.to_string_lossy().into_owned()])
    } else if resolved.is_dir() {
        runner::discover_test_files(&resolved).map_err(|e| {
            ToolError::new(ERR_PATH_NOT_FOUND, e.to_string()).with_data(json!({
                "path": resolved.display().to_string(),
            }))
        })
    } else {
        Err(ToolError::new(
            ERR_PATH_NOT_FOUND,
            format!("Path not found: {}", resolved.display()),
        )
        .with_data(json!({ "path": resolved.display().to_string() })))
    }
}

/// Which slice of the run the caller wants returned inline in the
/// response. Full is the legacy behaviour (entire verbose report);
/// agent is the NAZ-412 compact payload and is the default because it
/// is what the ticket asks us to surface; summary and failures map
/// directly onto the NAZ-401 artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportMode {
    Full,
    Summary,
    Failures,
    Agent,
}

impl ReportMode {
    fn parse(raw: &str) -> Result<Self, ToolError> {
        match raw.to_ascii_lowercase().as_str() {
            "full" => Ok(ReportMode::Full),
            "summary" => Ok(ReportMode::Summary),
            "failures" => Ok(ReportMode::Failures),
            "agent" => Ok(ReportMode::Agent),
            other => Err(ToolError::new(
                ERR_INVALID_PARAM,
                format!(
                    "Unknown report_mode '{}'. Use one of: full, summary, failures, agent.",
                    other
                ),
            )
            .with_data(json!({
                "param": "report_mode",
                "got": other,
                "allowed": ["full", "summary", "failures", "agent"],
            }))),
        }
    }
}

/// Artifact layout for one run. Every field is a filesystem path
/// rendered as a string so the agent can pass the value straight back
/// through MCP without guessing the separator.
fn artifact_paths(run_dir: &Path) -> Value {
    json!({
        "run_dir": run_dir.display().to_string(),
        "report": run_dir.join("report.json").display().to_string(),
        "summary": run_dir.join("summary.json").display().to_string(),
        "failures": run_dir.join("failures.json").display().to_string(),
        "state": run_dir.join("state.json").display().to_string(),
        "events": run_dir.join("events.jsonl").display().to_string(),
    })
}

fn path_exists_bool(p: &Path) -> bool {
    p.is_file()
}

// Execute the tests, write all run artifacts under
// `.tarn/runs/<run_id>/`, and return the raw pieces needed by each
// report_mode branch of `tarn_run`. Factored out so rerun can reuse
// the same artifact-writing pipeline without a second body of code.
struct RunOutputs {
    run_id: String,
    run_dir: PathBuf,
    workspace_root: PathBuf,
    run_result: RunResult,
    started_at: chrono::DateTime<chrono::Utc>,
    ended_at: chrono::DateTime<chrono::Utc>,
    exit_code: i32,
    selectors: Vec<tarn::selector::Selector>,
    executed_files: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn execute_and_persist(
    cwd: &ResolvedCwd,
    files: &[String],
    env_name: Option<&str>,
    cli_vars: &[(String, String)],
    tag_filter: &[String],
    selectors: &[tarn::selector::Selector],
    opts: &RunOptions,
    extra_args: &[String],
) -> Result<RunOutputs, ToolError> {
    let started_at = chrono::Utc::now();
    let start_instant = std::time::Instant::now();

    // Workspace anchor: explicit cwd is authoritative; otherwise walk
    // up from the resolved cwd to find the tarn project root, matching
    // the CLI's `find_project_root` fallback chain.
    let workspace_root = if cwd.explicit {
        cwd.path.clone()
    } else {
        config::find_project_root(&cwd.path).unwrap_or_else(|| cwd.path.clone())
    };

    let run_id = report::run_dir::generate_run_id(started_at);
    let run_dir = report::run_dir::ensure_run_directory(&workspace_root, &run_id).map_err(|e| {
        ToolError::new(
            ERR_RUN_FAILED,
            format!(
                "Failed to create run directory under {}/.tarn/runs: {}",
                workspace_root.display(),
                e
            ),
        )
        .with_data(json!({ "workspace_root": workspace_root.display().to_string() }))
    })?;

    // Open the events stream for the same lifecycle signals the CLI
    // emits so agents tailing `events.jsonl` see identical content.
    let events = match tarn::report::event_stream::EventStream::new(
        run_dir.join("events.jsonl"),
        run_id.clone(),
    ) {
        Ok(s) => Some(std::sync::Arc::new(s)),
        Err(_) => None,
    };

    if let Some(ref ev) = events {
        ev.emit_run_started(files, false, extra_args);
    }

    let mut file_results = Vec::new();
    for file_path in files {
        let fp = Path::new(file_path);
        let test_file = parser::parse_file(fp).map_err(|e| {
            ToolError::new(ERR_PARSE, e.to_string()).with_data(json!({ "file": file_path }))
        })?;

        let project_config = config::load_config(&workspace_root).map_err(|e| {
            ToolError::new(ERR_PARSE, e.to_string()).with_data(json!({
                "workspace_root": workspace_root.display().to_string(),
            }))
        })?;
        let resolved_env = env::resolve_env_with_profiles(
            &test_file.env,
            env_name,
            cli_vars,
            &workspace_root,
            &project_config.env_file,
            &project_config.environments,
        )
        .map_err(|e| {
            ToolError::new(ERR_PARSE, e.to_string()).with_data(json!({ "file": file_path }))
        })?;

        // The runner's `run_file` already emits file/test/step events
        // via its observer hook when one is provided; the agent path
        // only needs the terminal run_completed envelope so tools/list
        // consumers still get a clean stream for --agent-style tailing.
        let result = runner::run_file(&test_file, file_path, &resolved_env, tag_filter, opts)
            .map_err(|e| {
                ToolError::new(ERR_RUN_FAILED, e.to_string())
                    .with_data(json!({ "file": file_path }))
            })?;

        file_results.push(result);
    }

    let run_result = RunResult {
        file_results,
        duration_ms: start_instant.elapsed().as_millis() as u64,
    };
    let ended_at = chrono::Utc::now();
    let exit_code = report::compute_exit_code(&run_result);

    // 1. report.json: full verbose JSON, identical shape to the CLI's
    //    `.tarn/runs/<id>/report.json`.
    let report_json = report::json::render_with_rerun_source(
        &run_result,
        report::json::JsonOutputMode::Verbose,
        report::RenderOptions::default(),
        None,
    );
    if let Err(e) = std::fs::write(run_dir.join("report.json"), &report_json) {
        return Err(ToolError::new(
            ERR_RUN_FAILED,
            format!("Failed to write report.json: {}", e),
        ));
    }

    // 2. state.json / summary.json / failures.json: reuse the same
    //    library builders the CLI calls in `write_state_sidecar`, so
    //    the MCP path produces byte-identical artifacts.
    let state = report::state_writer::build_state_with_run_id(
        &run_result,
        started_at,
        ended_at,
        exit_code,
        extra_args,
        env_name.map(|s| s.to_string()),
        cli_vars
            .iter()
            .find(|(k, _)| k == "base_url")
            .map(|(_, v)| v.clone()),
        Some(run_id.clone()),
    );
    let (summary_doc, failures_doc) = report::summary::build_summary_and_failures(
        &run_result,
        started_at,
        ended_at,
        exit_code,
        Some(run_id.clone()),
        None,
    );
    let _ = report::state_writer::write_state_to_dir(&run_dir, &state);
    let _ = report::summary::write_summary_to_dir(&run_dir, &summary_doc);
    let _ = report::summary::write_failures_to_dir(&run_dir, &failures_doc);

    // 3. Refresh the workspace pointer artifacts so CLI-style readers
    //    (`tarn failures`, `tarn report` without `--run`) keep working
    //    after an MCP-driven run. Mirrors the CLI's `.tarn/…` writes.
    let tarn_dir = workspace_root.join(".tarn");
    let _ = report::state_writer::write_state(&workspace_root, &state);
    let _ = report::summary::write_summary_to_dir(&tarn_dir, &summary_doc);
    let _ = report::summary::write_failures_to_dir(&tarn_dir, &failures_doc);
    let _ = report::run_dir::copy_to_pointer(
        &run_dir.join("report.json"),
        &tarn_dir.join("last-run.json"),
    );

    if let Some(ref ev) = events {
        let failed_files = run_result.file_results.iter().filter(|f| !f.passed).count();
        let total_tests: usize = run_result
            .file_results
            .iter()
            .map(|f| f.test_results.len())
            .sum();
        let failed_tests = run_result
            .file_results
            .iter()
            .flat_map(|f| f.test_results.iter())
            .filter(|t| !t.passed)
            .count();
        let outcome = tarn::report::event_stream::RunOutcome {
            passed: run_result.passed(),
            exit_code,
            duration_ms: run_result.duration_ms,
            files: run_result.total_files(),
            tests: total_tests,
            steps: run_result.total_steps(),
            failed_files,
            failed_tests,
            failed_steps: run_result.failed_steps(),
        };
        ev.emit_run_completed(outcome);
    }

    Ok(RunOutputs {
        run_id,
        run_dir,
        workspace_root,
        run_result,
        started_at,
        ended_at,
        exit_code,
        selectors: selectors.to_vec(),
        executed_files: files.to_vec(),
    })
}

// Assemble the `tarn_run`-style response envelope once the run has
// completed and artifacts are on disk. Shared between `tarn_run` and
// `tarn_rerun_failed` so both produce identical payloads.
fn build_run_response(outputs: &RunOutputs, mode: ReportMode) -> Result<Value, ToolError> {
    let report_body = match mode {
        ReportMode::Full => {
            let text = report::render(&outputs.run_result, report::OutputFormat::Json);
            serde_json::from_str::<Value>(&text).map_err(|e| {
                ToolError::new(
                    ERR_RUN_FAILED,
                    format!("failed to parse rendered report: {}", e),
                )
            })?
        }
        ReportMode::Summary => {
            let (summary_doc, _) = report::summary::build_summary_and_failures(
                &outputs.run_result,
                outputs.started_at,
                outputs.ended_at,
                outputs.exit_code,
                Some(outputs.run_id.clone()),
                None,
            );
            serde_json::to_value(&summary_doc)
                .map_err(|e| ToolError::new(ERR_RUN_FAILED, format!("summary serialize: {}", e)))?
        }
        ReportMode::Failures => {
            let (_, failures_doc) = report::summary::build_summary_and_failures(
                &outputs.run_result,
                outputs.started_at,
                outputs.ended_at,
                outputs.exit_code,
                Some(outputs.run_id.clone()),
                None,
            );
            serde_json::to_value(&failures_doc)
                .map_err(|e| ToolError::new(ERR_RUN_FAILED, format!("failures serialize: {}", e)))?
        }
        ReportMode::Agent => {
            let agent_inputs = report::agent_report::AgentReportInputs {
                run_id: Some(outputs.run_id.clone()),
                exit_code: outputs.exit_code,
                started_at: outputs.started_at,
                ended_at: outputs.ended_at,
                selected_files: &outputs.executed_files,
                selectors: &outputs.selectors,
                run_directory: Some(outputs.run_dir.as_path()),
            };
            let agent = report::agent_report::build(&outputs.run_result, agent_inputs);
            serde_json::to_value(&agent)
                .map_err(|e| ToolError::new(ERR_RUN_FAILED, format!("agent serialize: {}", e)))?
        }
    };

    Ok(json!({
        "run_id": outputs.run_id,
        "exit_code": outputs.exit_code,
        "workspace_root": outputs.workspace_root.display().to_string(),
        "report_mode": match mode {
            ReportMode::Full => "full",
            ReportMode::Summary => "summary",
            ReportMode::Failures => "failures",
            ReportMode::Agent => "agent",
        },
        "report": report_body,
        "artifacts": artifact_paths(&outputs.run_dir),
    }))
}

fn parse_vars(params: &Value) -> Vec<(String, String)> {
    params
        .get("vars")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn build_run_opts() -> RunOptions {
    RunOptions {
        verbose: false,
        dry_run: false,
        http: HttpTransportConfig::default(),
        cookie_jar_per_test: false,
        fail_fast_within_test: false,
        verbose_responses: false,
        max_body_bytes: runner::DEFAULT_MAX_BODY_BYTES,
        // Fixture writing is CLI-facing; the MCP path has no reliable
        // workspace anchor for per-run fixtures, so keep it off.
        fixtures: tarn::report::fixture_writer::FixtureWriteConfig::default(),
    }
}

/// Execute `tarn_run`: parse, resolve env, run tests, write artifacts,
/// return a structured response whose report body matches `report_mode`.
pub fn tarn_run(params: &Value) -> Result<Value, ToolError> {
    let cwd = resolve_cwd(params)?;
    require_config_for_explicit_cwd(&cwd)?;

    let path_str = params
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("tests");
    let env_name = params.get("env").and_then(|v| v.as_str());
    let tag_str = params.get("tag").and_then(|v| v.as_str()).unwrap_or("");
    let cli_vars = parse_vars(params);
    let report_mode = match params.get("report_mode") {
        Some(v) => {
            let s = v.as_str().ok_or_else(|| {
                ToolError::new(ERR_INVALID_PARAM, "report_mode must be a string")
                    .with_data(json!({ "param": "report_mode" }))
            })?;
            ReportMode::parse(s)?
        }
        None => ReportMode::Agent,
    };

    let tag_filter = if tag_str.is_empty() {
        vec![]
    } else {
        runner::parse_tag_filter(tag_str)
    };

    let files = expand_test_files(path_str, &cwd.path)?;
    if files.is_empty() {
        return Err(ToolError::new(ERR_NO_TESTS, "No .tarn.yaml files found")
            .with_data(json!({ "path": path_str })));
    }

    let opts = build_run_opts();
    let outputs = execute_and_persist(
        &cwd,
        &files,
        env_name,
        &cli_vars,
        &tag_filter,
        &[],
        &opts,
        &["tarn-mcp".to_string(), "run".to_string()],
    )?;

    build_run_response(&outputs, report_mode)
}

/// Validate .tarn.yaml files without running them.
pub fn tarn_validate(params: &Value) -> Result<Value, ToolError> {
    let cwd = resolve_cwd(params)?;
    require_config_for_explicit_cwd(&cwd)?;

    let path_str = params.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
        ToolError::new(ERR_MISSING_PARAM, "Missing required parameter: path")
            .with_data(json!({ "param": "path" }))
    })?;

    let files = expand_test_files(path_str, &cwd.path)?;

    let mut results = Vec::new();
    for file_path in &files {
        let fp = Path::new(file_path);
        match parser::parse_file(fp) {
            Ok(_) => {
                results.push(json!({
                    "file": file_path,
                    "valid": true,
                }));
            }
            Err(e) => {
                results.push(json!({
                    "file": file_path,
                    "valid": false,
                    "error": e.to_string(),
                }));
            }
        }
    }

    let all_valid = results.iter().all(|r| r["valid"] == true);

    Ok(json!({
        "valid": all_valid,
        "files": results,
    }))
}

/// List available tests from .tarn.yaml files.
pub fn tarn_list(params: &Value) -> Result<Value, ToolError> {
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
                    tests.push(json!({
                        "name": "(flat steps)",
                        "steps": tf.steps.iter().map(|s| &s.name).collect::<Vec<_>>(),
                    }));
                }

                for (name, group) in &tf.tests {
                    tests.push(json!({
                        "name": name,
                        "description": group.description,
                        "tags": group.tags,
                        "steps": group.steps.iter().map(|s| &s.name).collect::<Vec<_>>(),
                    }));
                }

                file_list.push(json!({
                    "file": file_path,
                    "name": tf.name,
                    "tags": tf.tags,
                    "tests": tests,
                }));
            }
            Err(e) => {
                file_list.push(json!({
                    "file": file_path,
                    "error": e.to_string(),
                }));
            }
        }
    }

    Ok(json!({ "files": file_list }))
}

/// Resolve the workspace root for read-only artifact-inspection tools.
/// These tools do not run tests, so we mirror the CLI's fallback chain:
/// explicit cwd wins, otherwise walk up from the resolved cwd. An
/// explicit cwd without `tarn.config.yaml` still errors (same contract
/// as `tarn_run`).
fn resolve_workspace_root(params: &Value) -> Result<PathBuf, ToolError> {
    let cwd = resolve_cwd(params)?;
    require_config_for_explicit_cwd(&cwd)?;
    Ok(if cwd.explicit {
        cwd.path
    } else {
        config::find_project_root(&cwd.path).unwrap_or(cwd.path)
    })
}

// Resolve either an explicit run_id (including aliases `last`, `prev`,
// `latest`, `@latest`) or fall back to "latest" when absent. Callers
// that accept a bare `last` alias still go through here so the error
// data is uniform.
fn resolve_run_id_param(workspace_root: &Path, params: &Value) -> Result<String, ToolError> {
    let alias = params
        .get("run_id")
        .and_then(|v| v.as_str())
        .unwrap_or("last");
    report::run_dir::resolve_run_id(workspace_root, alias).map_err(|e| {
        ToolError::new(ERR_RUN_ID_UNKNOWN, e.to_string()).with_data(json!({
            "run_id": alias,
            "workspace_root": workspace_root.display().to_string(),
        }))
    })
}

fn read_json_artifact<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ToolError> {
    if !path.is_file() {
        return Err(ToolError::new(
            ERR_ARTIFACT_MISSING,
            format!("artifact missing at {}", path.display()),
        )
        .with_data(json!({ "path": path.display().to_string() })));
    }
    let raw = std::fs::read(path).map_err(|e| {
        ToolError::new(
            ERR_ARTIFACT_MISSING,
            format!("failed to read {}: {}", path.display(), e),
        )
        .with_data(json!({ "path": path.display().to_string() }))
    })?;
    serde_json::from_slice::<T>(&raw).map_err(|e| {
        ToolError::new(
            ERR_ARTIFACT_PARSE,
            format!("failed to parse {}: {}", path.display(), e),
        )
        .with_data(json!({ "path": path.display().to_string() }))
    })
}

/// Return grouped failures (NAZ-402) for a specific run. Wraps
/// `tarn failures --format json` so agents fetch the failures-only view
/// without re-loading `report.json`.
pub fn tarn_last_failures(params: &Value) -> Result<Value, ToolError> {
    let workspace_root = resolve_workspace_root(params)?;
    let run_id = resolve_run_id_param(&workspace_root, params)?;
    let run_dir = report::run_dir::run_directory(&workspace_root, &run_id);
    let failures_path = run_dir.join("failures.json");
    let doc: FailuresDoc = read_json_artifact(&failures_path)?;

    let report = report::failures_command::build_report(&doc, failures_path.display().to_string());
    let rendered: Value = serde_json::from_str(&report::failures_command::render_json(&report))
        .map_err(|e| ToolError::new(ERR_ARTIFACT_PARSE, format!("failures render parse: {}", e)))?;

    Ok(json!({
        "run_id": run_id,
        "workspace_root": workspace_root.display().to_string(),
        "failures": rendered,
        "artifacts": artifact_paths(&run_dir),
    }))
}

/// Return artifact paths plus existence flags for a run. Answers the
/// question "what's on disk for this run?" without loading any of the
/// payloads into the agent context.
pub fn tarn_get_run_artifacts(params: &Value) -> Result<Value, ToolError> {
    let workspace_root = resolve_workspace_root(params)?;
    let run_id = resolve_run_id_param(&workspace_root, params)?;
    let run_dir = report::run_dir::run_directory(&workspace_root, &run_id);

    let report_p = run_dir.join("report.json");
    let summary_p = run_dir.join("summary.json");
    let failures_p = run_dir.join("failures.json");
    let state_p = run_dir.join("state.json");
    let events_p = run_dir.join("events.jsonl");

    Ok(json!({
        "run_id": run_id,
        "workspace_root": workspace_root.display().to_string(),
        "run_dir": run_dir.display().to_string(),
        "report_path": report_p.display().to_string(),
        "summary_path": summary_p.display().to_string(),
        "failures_path": failures_p.display().to_string(),
        "state_path": state_p.display().to_string(),
        "events_path": events_p.display().to_string(),
        "exists": {
            "report": path_exists_bool(&report_p),
            "summary": path_exists_bool(&summary_p),
            "failures": path_exists_bool(&failures_p),
            "state": path_exists_bool(&state_p),
            "events": path_exists_bool(&events_p),
        },
    }))
}

/// Rerun only the failing `(file, test)` pairs from a prior run.
/// Response shape matches `tarn_run` (run_id, artifacts, AgentReport
/// body) so agents can loop: run → inspect failures → rerun_failed →
/// repeat, without branching on tool surface.
pub fn tarn_rerun_failed(params: &Value) -> Result<Value, ToolError> {
    let cwd = resolve_cwd(params)?;
    require_config_for_explicit_cwd(&cwd)?;

    // Workspace anchor: same rule as `tarn_run` so "explicit cwd ⇒
    // explicit workspace" holds end-to-end.
    let workspace_root = if cwd.explicit {
        cwd.path.clone()
    } else {
        config::find_project_root(&cwd.path).unwrap_or_else(|| cwd.path.clone())
    };

    // Pick the failures.json to drive selection from: an explicit
    // run_id → archive; absent → workspace pointer at
    // `.tarn/failures.json` (= last run).
    let source = match params.get("run_id").and_then(|v| v.as_str()) {
        Some(id) => {
            let run_id = report::run_dir::resolve_run_id(&workspace_root, id).map_err(|e| {
                ToolError::new(ERR_RUN_ID_UNKNOWN, e.to_string()).with_data(json!({
                    "run_id": id,
                    "workspace_root": workspace_root.display().to_string(),
                }))
            })?;
            let path =
                report::run_dir::run_directory(&workspace_root, &run_id).join("failures.json");
            report::rerun::RerunSourcePath::Archive { run_id, path }
        }
        None => report::rerun::RerunSourcePath::LatestPointer(
            workspace_root.join(".tarn").join("failures.json"),
        ),
    };

    let selection = report::rerun::load_selection(&source).map_err(|e| {
        ToolError::new(ERR_ARTIFACT_MISSING, e.to_string())
            .with_data(json!({ "source": source.display_path() }))
    })?;

    if selection.targets.is_empty() {
        return Err(ToolError::new(
            ERR_RERUN_EMPTY,
            format!(
                "no failing tests to rerun (source: {})",
                source.display_path()
            ),
        )
        .with_data(json!({
            "source": source.display_path(),
            "source_run_id": selection.source.run_id,
        })));
    }

    // `files` is the unique set touched by the selection; the runner
    // uses it for discovery. `selectors` narrows execution to just
    // those (file, test) pairs.
    let files = selection.files();
    let selectors: Vec<tarn::selector::Selector> = selection.selectors();

    let env_name = params.get("env_name").and_then(|v| v.as_str());
    let cli_vars = parse_vars(params);
    let report_mode = match params.get("report_mode") {
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| ToolError::new(ERR_INVALID_PARAM, "report_mode must be a string"))?;
            ReportMode::parse(s)?
        }
        None => ReportMode::Agent,
    };

    let opts = build_run_opts();
    let outputs = execute_and_persist_with_selectors(
        &cwd,
        &files,
        env_name,
        &cli_vars,
        &[],
        &selectors,
        &opts,
        &["tarn-mcp".to_string(), "rerun".to_string()],
    )?;

    let mut response = build_run_response(&outputs, report_mode)?;
    if let Value::Object(ref mut map) = response {
        map.insert(
            "rerun_source".into(),
            serde_json::to_value(&selection.source).unwrap_or(Value::Null),
        );
    }
    Ok(response)
}

// Same contract as `execute_and_persist` but applies a selector list so
// rerun narrows execution to specific `(file, test)` pairs via
// `runner::run_file_with_cookie_jars`. Kept separate rather than
// threading an optional slice through `execute_and_persist` because
// the non-rerun path has no selectors at all.
#[allow(clippy::too_many_arguments)]
fn execute_and_persist_with_selectors(
    cwd: &ResolvedCwd,
    files: &[String],
    env_name: Option<&str>,
    cli_vars: &[(String, String)],
    tag_filter: &[String],
    selectors: &[tarn::selector::Selector],
    opts: &RunOptions,
    extra_args: &[String],
) -> Result<RunOutputs, ToolError> {
    let started_at = chrono::Utc::now();
    let start_instant = std::time::Instant::now();

    let workspace_root = if cwd.explicit {
        cwd.path.clone()
    } else {
        config::find_project_root(&cwd.path).unwrap_or_else(|| cwd.path.clone())
    };

    let run_id = report::run_dir::generate_run_id(started_at);
    let run_dir = report::run_dir::ensure_run_directory(&workspace_root, &run_id).map_err(|e| {
        ToolError::new(
            ERR_RUN_FAILED,
            format!(
                "Failed to create run directory under {}/.tarn/runs: {}",
                workspace_root.display(),
                e
            ),
        )
        .with_data(json!({ "workspace_root": workspace_root.display().to_string() }))
    })?;

    let events = match tarn::report::event_stream::EventStream::new(
        run_dir.join("events.jsonl"),
        run_id.clone(),
    ) {
        Ok(s) => Some(std::sync::Arc::new(s)),
        Err(_) => None,
    };
    if let Some(ref ev) = events {
        ev.emit_run_started(files, false, extra_args);
    }

    let mut file_results = Vec::new();
    for file_path in files {
        let fp = Path::new(file_path);
        let test_file = parser::parse_file(fp).map_err(|e| {
            ToolError::new(ERR_PARSE, e.to_string()).with_data(json!({ "file": file_path }))
        })?;
        let project_config = config::load_config(&workspace_root).map_err(|e| {
            ToolError::new(ERR_PARSE, e.to_string()).with_data(json!({
                "workspace_root": workspace_root.display().to_string(),
            }))
        })?;
        let resolved_env = env::resolve_env_with_profiles(
            &test_file.env,
            env_name,
            cli_vars,
            &workspace_root,
            &project_config.env_file,
            &project_config.environments,
        )
        .map_err(|e| {
            ToolError::new(ERR_PARSE, e.to_string()).with_data(json!({ "file": file_path }))
        })?;

        let mut cookie_jars: std::collections::HashMap<String, tarn::cookie::CookieJar> =
            std::collections::HashMap::new();
        let result = runner::run_file_with_cookie_jars(
            &test_file,
            file_path,
            &resolved_env,
            tag_filter,
            selectors,
            opts,
            &mut cookie_jars,
            None,
        )
        .map_err(|e| {
            ToolError::new(ERR_RUN_FAILED, e.to_string()).with_data(json!({ "file": file_path }))
        })?;
        file_results.push(result);
    }

    let run_result = RunResult {
        file_results,
        duration_ms: start_instant.elapsed().as_millis() as u64,
    };
    let ended_at = chrono::Utc::now();
    let exit_code = report::compute_exit_code(&run_result);

    let report_json = report::json::render_with_rerun_source(
        &run_result,
        report::json::JsonOutputMode::Verbose,
        report::RenderOptions::default(),
        None,
    );
    if let Err(e) = std::fs::write(run_dir.join("report.json"), &report_json) {
        return Err(ToolError::new(
            ERR_RUN_FAILED,
            format!("Failed to write report.json: {}", e),
        ));
    }
    let state = report::state_writer::build_state_with_run_id(
        &run_result,
        started_at,
        ended_at,
        exit_code,
        extra_args,
        env_name.map(|s| s.to_string()),
        cli_vars
            .iter()
            .find(|(k, _)| k == "base_url")
            .map(|(_, v)| v.clone()),
        Some(run_id.clone()),
    );
    let (summary_doc, failures_doc) = report::summary::build_summary_and_failures(
        &run_result,
        started_at,
        ended_at,
        exit_code,
        Some(run_id.clone()),
        None,
    );
    let _ = report::state_writer::write_state_to_dir(&run_dir, &state);
    let _ = report::summary::write_summary_to_dir(&run_dir, &summary_doc);
    let _ = report::summary::write_failures_to_dir(&run_dir, &failures_doc);
    let tarn_dir = workspace_root.join(".tarn");
    let _ = report::state_writer::write_state(&workspace_root, &state);
    let _ = report::summary::write_summary_to_dir(&tarn_dir, &summary_doc);
    let _ = report::summary::write_failures_to_dir(&tarn_dir, &failures_doc);
    let _ = report::run_dir::copy_to_pointer(
        &run_dir.join("report.json"),
        &tarn_dir.join("last-run.json"),
    );

    if let Some(ref ev) = events {
        let failed_files = run_result.file_results.iter().filter(|f| !f.passed).count();
        let total_tests: usize = run_result
            .file_results
            .iter()
            .map(|f| f.test_results.len())
            .sum();
        let failed_tests = run_result
            .file_results
            .iter()
            .flat_map(|f| f.test_results.iter())
            .filter(|t| !t.passed)
            .count();
        let outcome = tarn::report::event_stream::RunOutcome {
            passed: run_result.passed(),
            exit_code,
            duration_ms: run_result.duration_ms,
            files: run_result.total_files(),
            tests: total_tests,
            steps: run_result.total_steps(),
            failed_files,
            failed_tests,
            failed_steps: run_result.failed_steps(),
        };
        ev.emit_run_completed(outcome);
    }

    Ok(RunOutputs {
        run_id,
        run_dir,
        workspace_root,
        run_result,
        started_at,
        ended_at,
        exit_code,
        selectors: selectors.to_vec(),
        executed_files: files.to_vec(),
    })
}

/// Read the persisted summary+failures for a run and render the concise
/// NAZ-404 view. Agents call this to check "is run X green?" without
/// shipping the full report body.
pub fn tarn_report(params: &Value) -> Result<Value, ToolError> {
    let workspace_root = resolve_workspace_root(params)?;
    let run_id = resolve_run_id_param(&workspace_root, params)?;
    let run_dir = report::run_dir::run_directory(&workspace_root, &run_id);
    let summary: SummaryDoc = read_json_artifact(&run_dir.join("summary.json"))?;
    let failures: FailuresDoc = read_json_artifact(&run_dir.join("failures.json"))?;

    let concise = report::concise::render_json(&summary, &failures, &run_id);
    Ok(json!({
        "run_id": run_id,
        "workspace_root": workspace_root.display().to_string(),
        "report": concise,
        "artifacts": artifact_paths(&run_dir),
    }))
}

/// Drive `tarn inspect`'s library entry points against a prior run.
/// Returns the inspect JSON view plus the artifact paths of the run
/// that seeded it.
pub fn tarn_inspect(params: &Value) -> Result<Value, ToolError> {
    let workspace_root = resolve_workspace_root(params)?;
    let run_alias = params
        .get("run_id")
        .and_then(|v| v.as_str())
        .unwrap_or("last");
    let target_str = params.get("target").and_then(|v| v.as_str());
    let filter_category = params.get("filter_category").and_then(|v| v.as_str());

    if let Some(cat) = filter_category {
        if let Err(msg) = report::inspect::validate_category(cat) {
            return Err(ToolError::new(ERR_INVALID_PARAM, msg).with_data(json!({
                "param": "filter_category",
                "got": cat,
            })));
        }
    }

    let source = report::inspect::resolve_source(&workspace_root, run_alias).map_err(|e| {
        ToolError::new(ERR_INSPECT_FAILED, e.to_string()).with_data(json!({ "run_id": run_alias }))
    })?;
    let report_value = report::inspect::load_report(&source).map_err(|e| {
        ToolError::new(ERR_INSPECT_FAILED, e.to_string()).with_data(json!({
            "path": source.display_path(),
        }))
    })?;
    let parsed_target = report::inspect::Target::parse(target_str).map_err(|e| {
        ToolError::new(ERR_INVALID_PARAM, e.to_string()).with_data(json!({
            "param": "target",
            "got": target_str,
        }))
    })?;
    let view = report::inspect::build_view(&source, &report_value, &parsed_target, filter_category)
        .map_err(|e| {
            ToolError::new(ERR_INSPECT_FAILED, e.to_string())
                .with_data(json!({ "target": target_str }))
        })?;

    let run_dir = source
        .run_id
        .as_deref()
        .map(|id| report::run_dir::run_directory(&workspace_root, id));

    Ok(json!({
        "run_id": source.run_id,
        "workspace_root": workspace_root.display().to_string(),
        "view": view,
        "artifacts": run_dir
            .as_ref()
            .map(|d| artifact_paths(d))
            .unwrap_or(Value::Null),
    }))
}

/// Backing handler for the MCP `tarn_fix_plan` tool.
pub fn tarn_fix_plan(params: &Value) -> Result<Value, ToolError> {
    let report = if let Some(report) = params.get("report") {
        report.clone()
    } else {
        // Reuse `tarn_run` so cwd/env/vars resolve through the same
        // path — the full verbose report is what the fix planner
        // expects to walk.
        let mut inner = params.clone();
        if let Value::Object(ref mut map) = inner {
            map.insert("report_mode".into(), Value::String("full".into()));
        }
        let run_response = tarn_run(&inner)?;
        run_response
            .get("report")
            .cloned()
            .ok_or_else(|| ToolError::new(ERR_RUN_FAILED, "tarn_run returned no report"))?
    };

    if report
        .get("files")
        .and_then(|value| value.as_array())
        .is_none()
    {
        return Err(ToolError::new(
            ERR_INVALID_REPORT,
            "Invalid Tarn report: missing `files` array",
        ));
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

    Ok(json!({
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
        assert_eq!(err.code, ERR_INVALID_CWD);
        assert!(err.message.contains("absolute"), "got: {}", err.message);
    }

    #[test]
    fn resolve_cwd_rejects_non_string() {
        let params = serde_json::json!({ "cwd": 42 });
        let err = resolve_cwd(&params).unwrap_err();
        assert_eq!(err.code, ERR_INVALID_PARAM);
        assert!(
            err.message.contains("must be a string"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn resolve_cwd_rejects_missing_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = tmp.path().join("definitely-not-a-real-tarn-cwd-xyzzy");
        let params = serde_json::json!({ "cwd": missing.to_string_lossy() });
        let err = resolve_cwd(&params).unwrap_err();
        assert_eq!(err.code, ERR_INVALID_CWD);
        assert!(
            err.message.contains("does not exist"),
            "got: {}",
            err.message
        );
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
        assert_eq!(err.code, ERR_MISSING_CONFIG);
        assert!(err.message.contains("tarn.config.yaml"));
        assert!(err.message.contains(&tmp.path().display().to_string()));
        // Structured data payload must include the offending path so
        // agents don't need to string-parse the message.
        let data = err.data.unwrap();
        assert_eq!(
            data.get("cwd").and_then(|v| v.as_str()),
            Some(tmp.path().display().to_string()).as_deref()
        );
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
        require_config_for_explicit_cwd(&resolved).unwrap();
    }

    #[test]
    fn resolve_path_against_cwd_joins_relative() {
        let cwd = std::path::Path::new("/tmp/workspace");
        let joined = resolve_path_against_cwd("tests/x.tarn.yaml", cwd);
        assert_eq!(
            joined,
            std::path::PathBuf::from("/tmp/workspace/tests/x.tarn.yaml")
        );
    }

    #[test]
    fn resolve_path_against_cwd_preserves_absolute() {
        let cwd = std::path::Path::new("/tmp/workspace");
        let joined = resolve_path_against_cwd("/other/file.tarn.yaml", cwd);
        assert_eq!(joined, std::path::PathBuf::from("/other/file.tarn.yaml"));
    }

    #[test]
    fn report_mode_parses_known_values() {
        assert_eq!(ReportMode::parse("full").unwrap(), ReportMode::Full);
        assert_eq!(ReportMode::parse("summary").unwrap(), ReportMode::Summary);
        assert_eq!(ReportMode::parse("failures").unwrap(), ReportMode::Failures);
        assert_eq!(ReportMode::parse("agent").unwrap(), ReportMode::Agent);
        assert_eq!(ReportMode::parse("AGENT").unwrap(), ReportMode::Agent);
    }

    #[test]
    fn report_mode_rejects_unknown() {
        let err = ReportMode::parse("verbose").unwrap_err();
        assert_eq!(err.code, ERR_INVALID_PARAM);
        let data = err.data.unwrap();
        assert_eq!(data.get("got").and_then(|v| v.as_str()), Some("verbose"));
    }

    #[test]
    fn tool_error_to_tool_call_json_carries_triple() {
        let err = ToolError::new(ERR_INVALID_CWD, "nope").with_data(json!({ "x": 1 }));
        let v = err.to_tool_call_json();
        assert_eq!(
            v.get("code").and_then(|v| v.as_i64()),
            Some(ERR_INVALID_CWD as i64)
        );
        assert_eq!(v.get("message").and_then(|v| v.as_str()), Some("nope"));
        assert_eq!(
            v.get("data")
                .and_then(|v| v.get("x"))
                .and_then(|v| v.as_i64()),
            Some(1)
        );
    }

    #[test]
    fn artifact_paths_carries_all_expected_fields() {
        let v = artifact_paths(std::path::Path::new("/tmp/ws/.tarn/runs/rid"));
        for key in &[
            "run_dir", "report", "summary", "failures", "state", "events",
        ] {
            assert!(
                v.get(*key).is_some(),
                "missing artifact path field: {}",
                key
            );
        }
    }
}
