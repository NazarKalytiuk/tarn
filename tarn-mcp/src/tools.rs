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
// NAZ-416: new high-level surfaces. Each domain gets its own code so the
// structured error data the agent receives ties a failure to a specific
// CLI equivalent without text-matching the message.
pub const ERR_IMPACT_INVALID_INPUT: i32 = -32064;
pub const ERR_IMPACT_PARSE_FAILED: i32 = -32065;
pub const ERR_SCAFFOLD_INVALID_INPUT: i32 = -32066;
pub const ERR_SCAFFOLD_FAILED: i32 = -32067;
pub const ERR_PACK_CONTEXT_INVALID_INPUT: i32 = -32068;

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
        // NAZ-416: echo the knobs the caller passed so the agent can
        // confirm the rerun used the expected env / vars without
        // re-parsing its own arguments, plus the selection slice the
        // runner actually executed.
        map.insert(
            "selection".into(),
            json!({
                "files": selection.files(),
                "targets": selection
                    .targets
                    .iter()
                    .map(|t| json!({
                        "file": t.file,
                        "test": t.test,
                        "label": t.label(),
                    }))
                    .collect::<Vec<_>>(),
            }),
        );
        let mut vars_obj = serde_json::Map::new();
        for (k, v) in &cli_vars {
            vars_obj.insert(k.clone(), Value::String(v.clone()));
        }
        map.insert(
            "inputs".into(),
            json!({
                "env_name": env_name,
                "vars": Value::Object(vars_obj),
            }),
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

// Collect a `Vec<String>` from an optional JSON array-of-strings field.
// Used by tool parameter validation so the same "wrong type" / "wrong
// element type" error shape appears everywhere.
fn parse_string_array(params: &Value, key: &str) -> Result<Vec<String>, ToolError> {
    let Some(raw) = params.get(key) else {
        return Ok(Vec::new());
    };
    let Some(arr) = raw.as_array() else {
        return Err(
            ToolError::new(ERR_INVALID_PARAM, format!("`{}` must be an array", key))
                .with_data(json!({ "param": key, "got": raw })),
        );
    };
    let mut out = Vec::with_capacity(arr.len());
    for (idx, v) in arr.iter().enumerate() {
        let s = v.as_str().ok_or_else(|| {
            ToolError::new(
                ERR_INVALID_PARAM,
                format!("`{}[{}]` must be a string", key, idx),
            )
            .with_data(json!({ "param": key, "index": idx, "got": v }))
        })?;
        out.push(s.to_string());
    }
    Ok(out)
}

/// `tarn_impact` — map a change to the tests it most likely affects.
/// Mirrors the CLI's `tarn impact --format json` contract so agents that
/// already read one surface can read the other.
pub fn tarn_impact(params: &Value) -> Result<Value, ToolError> {
    let cwd = resolve_cwd(params)?;
    require_config_for_explicit_cwd(&cwd)?;
    let workspace_root = if cwd.explicit {
        cwd.path.clone()
    } else {
        config::find_project_root(&cwd.path).unwrap_or_else(|| cwd.path.clone())
    };

    let diff = params
        .get("diff")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let files = parse_string_array(params, "files")?;
    let openapi_ops = parse_string_array(params, "openapi_ops")?;
    let no_default_excludes = params
        .get("no_default_excludes")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Parse endpoints: accept either string specs (`"GET:/users"`) or
    // structured `{method, path}` objects so the MCP surface is friendlier
    // than the bare CLI flag.
    let mut endpoints: Vec<tarn::impact::EndpointChange> = Vec::new();
    if let Some(raw) = params.get("endpoints") {
        let arr = raw.as_array().ok_or_else(|| {
            ToolError::new(ERR_INVALID_PARAM, "`endpoints` must be an array")
                .with_data(json!({ "param": "endpoints" }))
        })?;
        for (idx, ep) in arr.iter().enumerate() {
            if let Some(s) = ep.as_str() {
                let parsed = tarn::impact::parse_endpoint(s).map_err(|e| {
                    ToolError::new(ERR_IMPACT_INVALID_INPUT, e)
                        .with_data(json!({ "param": "endpoints", "index": idx, "got": s }))
                })?;
                endpoints.push(parsed);
            } else if let Some(obj) = ep.as_object() {
                let method = obj.get("method").and_then(|v| v.as_str()).ok_or_else(|| {
                    ToolError::new(ERR_IMPACT_INVALID_INPUT, "endpoint object missing `method`")
                        .with_data(json!({ "param": "endpoints", "index": idx }))
                })?;
                let path = obj.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    ToolError::new(ERR_IMPACT_INVALID_INPUT, "endpoint object missing `path`")
                        .with_data(json!({ "param": "endpoints", "index": idx }))
                })?;
                endpoints.push(tarn::impact::EndpointChange {
                    method: method.to_string(),
                    path: path.to_string(),
                });
            } else {
                return Err(ToolError::new(
                    ERR_IMPACT_INVALID_INPUT,
                    "endpoints[] entries must be `METHOD:/path` strings or `{method, path}` objects",
                )
                .with_data(json!({ "param": "endpoints", "index": idx, "got": ep })));
            }
        }
    }

    // At least one input family is required — the CLI enforces the same
    // rule and surfaces a hint pointing at the flags that would satisfy it.
    if !diff && files.is_empty() && endpoints.is_empty() && openapi_ops.is_empty() {
        return Err(ToolError::new(
            ERR_IMPACT_INVALID_INPUT,
            "tarn_impact needs at least one of: diff, files, endpoints, openapi_ops",
        )
        .with_data(json!({
            "hint": "provide at least one of: { \"diff\": true }, files: [...], endpoints: [...], openapi_ops: [...]",
        })));
    }

    let min_confidence = match params.get("min_confidence").and_then(|v| v.as_str()) {
        Some(s) => tarn::impact::Confidence::parse(s).map_err(|e| {
            ToolError::new(ERR_INVALID_PARAM, e)
                .with_data(json!({ "param": "min_confidence", "got": s }))
        })?,
        None => tarn::impact::Confidence::Low,
    };

    // `diff` expects to run `git diff` under the workspace root. Keep the
    // behaviour scoped so we do not poke at the host's cwd.
    let diff_files: Vec<String> = if diff {
        read_git_diff_files_in(&workspace_root).map_err(|e| {
            ToolError::new(ERR_IMPACT_PARSE_FAILED, e).with_data(json!({
                "workspace_root": workspace_root.display().to_string(),
            }))
        })?
    } else {
        Vec::new()
    };

    // Discover tests under the given path (or the workspace's test_dir).
    let path_str = params.get("path").and_then(|v| v.as_str());
    let discovery_files = discover_impact_files(&workspace_root, path_str, no_default_excludes)?;

    // Load + parse each test; keep the raw source so the include matcher
    // has something to scan.
    let mut loaded: Vec<(String, tarn::model::TestFile, String)> =
        Vec::with_capacity(discovery_files.len());
    for file in &discovery_files {
        let source = std::fs::read_to_string(file).map_err(|e| {
            ToolError::new(
                ERR_IMPACT_PARSE_FAILED,
                format!("failed to read {}: {}", file, e),
            )
            .with_data(json!({ "file": file }))
        })?;
        let parsed = parser::parse_str(&source, Path::new(file)).map_err(|e| {
            ToolError::new(
                ERR_IMPACT_PARSE_FAILED,
                format!("failed to parse {}: {}", file, e),
            )
            .with_data(json!({ "file": file }))
        })?;
        loaded.push((file.clone(), parsed, source));
    }
    let tests: Vec<tarn::impact::LoadedTest<'_>> = loaded
        .iter()
        .map(|(p, parsed, source)| tarn::impact::LoadedTest {
            path: p.clone(),
            parsed,
            source: source.as_str(),
        })
        .collect();

    let change = tarn::impact::ChangeSet {
        diff_files,
        files,
        endpoints,
        openapi_ops,
    };
    let report = tarn::impact::analyze(&change, &tests);
    let report = tarn::impact::filter_by_confidence(report, min_confidence);

    // The CLI emits `ImpactReport` verbatim via `render_json`; we parse it
    // back to `Value` so the MCP envelope stays a single JSON object and
    // can carry the `cwd` echo without surprising the agent with a
    // stringly-typed body.
    let body: Value = serde_json::from_str(&tarn::impact::render_json(&report)).map_err(|e| {
        ToolError::new(
            ERR_IMPACT_PARSE_FAILED,
            format!("failed to serialise impact report: {}", e),
        )
    })?;
    let Value::Object(mut obj) = body else {
        return Err(ToolError::new(
            ERR_IMPACT_PARSE_FAILED,
            "impact report did not serialise as object",
        ));
    };
    obj.insert(
        "cwd".into(),
        Value::String(workspace_root.display().to_string()),
    );
    Ok(Value::Object(obj))
}

// Resolve the discovery root for `tarn_impact` without touching process
// cwd. Mirrors the CLI's `resolve_files_with_report` fallback but driven
// by the MCP caller's explicit workspace.
fn discover_impact_files(
    workspace_root: &Path,
    path: Option<&str>,
    no_default_excludes: bool,
) -> Result<Vec<String>, ToolError> {
    let opts = if no_default_excludes {
        runner::DiscoveryOptions {
            ignored_dirs: Vec::new(),
        }
    } else {
        runner::DiscoveryOptions::default()
    };

    let target: PathBuf = match path {
        Some(p) => resolve_path_against_cwd(p, workspace_root),
        None => {
            let project_config = config::load_config(workspace_root).map_err(|e| {
                ToolError::new(ERR_PARSE, e.to_string()).with_data(json!({
                    "workspace_root": workspace_root.display().to_string(),
                }))
            })?;
            let tests_dir = workspace_root.join(&project_config.test_dir);
            if tests_dir.is_dir() {
                tests_dir
            } else {
                workspace_root.to_path_buf()
            }
        }
    };

    if target.is_file() {
        return Ok(vec![target.to_string_lossy().into_owned()]);
    }
    if !target.is_dir() {
        return Err(ToolError::new(
            ERR_PATH_NOT_FOUND,
            format!("Path not found: {}", target.display()),
        )
        .with_data(json!({ "path": target.display().to_string() })));
    }
    let report = runner::discover_test_files_with_report(&target, &opts).map_err(|e| {
        ToolError::new(ERR_PATH_NOT_FOUND, e.to_string())
            .with_data(json!({ "path": target.display().to_string() }))
    })?;
    Ok(report.files)
}

// Run `git diff --name-only HEAD` under the given workspace root. Kept as
// an internal helper (rather than shelling out to the user's cwd) so the
// MCP server is insulated from whatever directory the host process is in.
fn read_git_diff_files_in(workspace_root: &Path) -> Result<Vec<String>, String> {
    use std::process::Command as StdCommand;

    let first = StdCommand::new("git")
        .current_dir(workspace_root)
        .args(["diff", "--name-only", "HEAD"])
        .output();
    let output = match first {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            if stderr.contains("unknown revision")
                || stderr.contains("bad revision")
                || stderr.contains("does not have any commits yet")
            {
                StdCommand::new("git")
                    .current_dir(workspace_root)
                    .args(["diff", "--name-only"])
                    .output()
                    .map_err(|e| format!("failed to run git diff: {e}"))?
            } else {
                return Err(format!(
                    "git diff failed (exit {}): {}",
                    o.status.code().unwrap_or(-1),
                    stderr.trim()
                ));
            }
        }
        Err(e) => {
            return Err(format!("failed to run git (is git installed?): {e}"));
        }
    };
    if !output.status.success() {
        return Err(format!(
            "git diff failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

/// `tarn_scaffold` — generate a starter `.tarn.yaml` from one of four
/// input modes (openapi / curl / explicit / recorded). Returns both the
/// rendered YAML and structured metadata so an agent can iterate without
/// re-reading the file.
pub fn tarn_scaffold(params: &Value) -> Result<Value, ToolError> {
    let cwd = resolve_cwd(params)?;
    require_config_for_explicit_cwd(&cwd)?;
    let workspace_root = if cwd.explicit {
        cwd.path.clone()
    } else {
        config::find_project_root(&cwd.path).unwrap_or_else(|| cwd.path.clone())
    };

    let mode = params.get("mode").and_then(|v| v.as_str()).ok_or_else(|| {
        ToolError::new(
            ERR_SCAFFOLD_INVALID_INPUT,
            "Missing required parameter: mode (openapi|curl|explicit|recorded)",
        )
        .with_data(json!({ "param": "mode" }))
    })?;

    // Exactly one of the mode-specific payload objects must be present;
    // the CLI enforces the same "pick one" rule by counting flags. Doing
    // the check up front keeps the error payload predictable.
    let openapi = params.get("openapi");
    let curl = params.get("curl");
    let explicit = params.get("explicit");
    let recorded = params.get("recorded");
    let provided = [openapi, curl, explicit, recorded]
        .iter()
        .filter(|v| v.is_some())
        .count();
    if provided > 1 {
        return Err(ToolError::new(
            ERR_SCAFFOLD_INVALID_INPUT,
            "Provide exactly one of: openapi, curl, explicit, recorded",
        )
        .with_data(json!({ "provided": provided, "expected": 1 })));
    }

    let input = match mode {
        "openapi" => {
            let obj = openapi.and_then(|v| v.as_object()).ok_or_else(|| {
                ToolError::new(
                    ERR_SCAFFOLD_INVALID_INPUT,
                    "mode=openapi requires `openapi: {spec_path, op_id}`",
                )
                .with_data(json!({ "param": "openapi" }))
            })?;
            let spec_path = obj
                .get("spec_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ToolError::new(ERR_SCAFFOLD_INVALID_INPUT, "openapi.spec_path is required")
                        .with_data(json!({ "param": "openapi.spec_path" }))
                })?;
            let op_id = obj.get("op_id").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::new(ERR_SCAFFOLD_INVALID_INPUT, "openapi.op_id is required")
                    .with_data(json!({ "param": "openapi.op_id" }))
            })?;
            tarn::scaffold::ScaffoldInput::OpenApi {
                spec_path: resolve_path_against_cwd(spec_path, &workspace_root),
                op_id: op_id.to_string(),
            }
        }
        "curl" => {
            let obj = curl.and_then(|v| v.as_object()).ok_or_else(|| {
                ToolError::new(
                    ERR_SCAFFOLD_INVALID_INPUT,
                    "mode=curl requires `curl: {command|file}`",
                )
                .with_data(json!({ "param": "curl" }))
            })?;
            // Accept the literal command text OR a file path that carries it.
            if let Some(cmd) = obj.get("command").and_then(|v| v.as_str()) {
                tarn::scaffold::ScaffoldInput::Curl {
                    curl_text: cmd.to_string(),
                    source_label: "inline".into(),
                }
            } else if let Some(file) = obj.get("file").and_then(|v| v.as_str()) {
                let resolved = resolve_path_against_cwd(file, &workspace_root);
                let text = std::fs::read_to_string(&resolved).map_err(|e| {
                    ToolError::new(
                        ERR_SCAFFOLD_INVALID_INPUT,
                        format!("failed to read curl file {}: {}", resolved.display(), e),
                    )
                    .with_data(
                        json!({ "param": "curl.file", "path": resolved.display().to_string() }),
                    )
                })?;
                let label = resolved
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(file)
                    .to_string();
                tarn::scaffold::ScaffoldInput::Curl {
                    curl_text: text,
                    source_label: label,
                }
            } else {
                return Err(ToolError::new(
                    ERR_SCAFFOLD_INVALID_INPUT,
                    "curl mode requires `curl.command` or `curl.file`",
                )
                .with_data(json!({ "param": "curl" })));
            }
        }
        "explicit" => {
            let obj = explicit.and_then(|v| v.as_object()).ok_or_else(|| {
                ToolError::new(
                    ERR_SCAFFOLD_INVALID_INPUT,
                    "mode=explicit requires `explicit: {method, url}`",
                )
                .with_data(json!({ "param": "explicit" }))
            })?;
            let method = obj.get("method").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::new(ERR_SCAFFOLD_INVALID_INPUT, "explicit.method is required")
                    .with_data(json!({ "param": "explicit.method" }))
            })?;
            let url = obj.get("url").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::new(ERR_SCAFFOLD_INVALID_INPUT, "explicit.url is required")
                    .with_data(json!({ "param": "explicit.url" }))
            })?;
            tarn::scaffold::ScaffoldInput::Explicit {
                method: method.to_string(),
                url: url.to_string(),
            }
        }
        "recorded" => {
            let obj = recorded.and_then(|v| v.as_object()).ok_or_else(|| {
                ToolError::new(
                    ERR_SCAFFOLD_INVALID_INPUT,
                    "mode=recorded requires `recorded: {path}`",
                )
                .with_data(json!({ "param": "recorded" }))
            })?;
            let path_str = obj.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::new(ERR_SCAFFOLD_INVALID_INPUT, "recorded.path is required")
                    .with_data(json!({ "param": "recorded.path" }))
            })?;
            tarn::scaffold::ScaffoldInput::Recorded {
                path: resolve_path_against_cwd(path_str, &workspace_root),
            }
        }
        other => {
            return Err(ToolError::new(
                ERR_SCAFFOLD_INVALID_INPUT,
                format!(
                    "unknown mode '{}' (expected: openapi|curl|explicit|recorded)",
                    other
                ),
            )
            .with_data(json!({
                "param": "mode",
                "got": other,
                "allowed": ["openapi", "curl", "explicit", "recorded"],
            })));
        }
    };

    let name_override = params
        .get("name")
        .and_then(|v| v.as_str())
        .map(String::from);
    let options = tarn::scaffold::ScaffoldOptions { name_override };

    let result = tarn::scaffold::generate(&input, &options).map_err(|e| {
        ToolError::new(ERR_SCAFFOLD_FAILED, e.to_string()).with_data(json!({ "mode": mode }))
    })?;

    let body_shape = match &result.request.body {
        Some(tarn::scaffold::BodyShape::Json(v)) => Some(v.clone()),
        Some(tarn::scaffold::BodyShape::Raw(s)) => Some(Value::String(s.clone())),
        None => None,
    };
    let todos: Vec<Value> = result
        .todos
        .iter()
        .map(|t| {
            json!({
                "line": t.line,
                "category": t.category.as_str(),
                "message": t.message,
            })
        })
        .collect();
    let mut response_captures: Vec<String> = result.request.captures.keys().cloned().collect();
    response_captures.sort();
    let headers: serde_json::Map<String, Value> = result
        .request
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();

    let mut response = json!({
        "schema_version": 1,
        "run_id": Value::Null,
        "source_mode": result.source_mode.as_str(),
        "inferred": {
            "method": result.request.method,
            "url": result.request.url,
            "headers": Value::Object(headers),
            "body_shape": body_shape,
            "response_captures": response_captures,
            "response_shape_keys": result.request.response_shape_keys,
            "path_params": result.request.path_params,
        },
        "todos": todos,
        "yaml": result.yaml.clone(),
        "validation": {
            "parsed_ok": result.parsed_ok,
            "schema_ok": result.schema_ok,
        },
    });

    // Optional `out` writes the rendered YAML (or JSON) to disk. We
    // mirror the CLI rule: refuse to clobber an existing file unless
    // `force: true` so scaffolds never silently overwrite a user's work.
    if let Some(out_raw) = params.get("out").and_then(|v| v.as_str()) {
        let force = params
            .get("force")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let out_path = resolve_path_against_cwd(out_raw, &workspace_root);
        if out_path.exists() && !force {
            return Err(ToolError::new(
                ERR_SCAFFOLD_INVALID_INPUT,
                format!(
                    "{} already exists (pass force=true to overwrite)",
                    out_path.display()
                ),
            )
            .with_data(json!({ "param": "out", "path": out_path.display().to_string() })));
        }
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ToolError::new(
                        ERR_SCAFFOLD_FAILED,
                        format!("failed to create {}: {}", parent.display(), e),
                    )
                })?;
            }
        }
        let format = params
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("yaml");
        let payload = match format {
            "yaml" => result.yaml.clone(),
            "json" => serde_json::to_string_pretty(&response).unwrap_or_default() + "\n",
            other => {
                return Err(ToolError::new(
                    ERR_SCAFFOLD_INVALID_INPUT,
                    format!("unknown scaffold format '{}' (expected yaml|json)", other),
                )
                .with_data(json!({ "param": "format", "got": other })));
            }
        };
        std::fs::write(&out_path, payload.as_bytes()).map_err(|e| {
            ToolError::new(
                ERR_SCAFFOLD_FAILED,
                format!("failed to write {}: {}", out_path.display(), e),
            )
        })?;
        if let Value::Object(ref mut map) = response {
            map.insert(
                "written_to".into(),
                Value::String(out_path.display().to_string()),
            );
        }
    }

    Ok(response)
}

/// `tarn_run_agent` — convenience wrapper over `tarn_run` with
/// `report_mode: agent` pre-selected and the selector knobs surfaced
/// explicitly. Agents driving the inner loop land here first so they do
/// not need to remember to set `report_mode`.
pub fn tarn_run_agent(params: &Value) -> Result<Value, ToolError> {
    let cwd = resolve_cwd(params)?;
    require_config_for_explicit_cwd(&cwd)?;

    let path_str = params
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("tests");
    let env_name = params.get("env_name").and_then(|v| v.as_str());
    let tag_str = params.get("tag").and_then(|v| v.as_str()).unwrap_or("");
    let cli_vars = parse_vars(params);
    let no_default_excludes = params
        .get("no_default_excludes")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let tag_filter = if tag_str.is_empty() {
        Vec::new()
    } else {
        runner::parse_tag_filter(tag_str)
    };

    // Build selector list from the three NAZ-412 knobs. `select` is a
    // full `FILE[::TEST[::STEP]]` grammar; `test_filter` / `step_filter`
    // synthesize wildcard selectors so a caller who knows the test name
    // but not the file path can still narrow the run.
    let mut selectors: Vec<tarn::selector::Selector> = Vec::new();
    for (idx, s) in parse_string_array(params, "select")?.iter().enumerate() {
        let parsed = tarn::selector::Selector::parse(s).map_err(|e| {
            ToolError::new(ERR_INVALID_PARAM, e)
                .with_data(json!({ "param": "select", "index": idx, "got": s }))
        })?;
        selectors.push(parsed);
    }
    let test_filter = params
        .get("test_filter")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let step_filter = params
        .get("step_filter")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    if test_filter.is_some() || step_filter.is_some() {
        let step = step_filter.map(|s| {
            if let Ok(idx) = s.parse::<usize>() {
                tarn::selector::StepSelector::Index(idx)
            } else {
                tarn::selector::StepSelector::Name(s.to_string())
            }
        });
        selectors.push(tarn::selector::Selector::wildcard(
            test_filter.map(String::from),
            step,
        ));
    }

    // Discover files the same way `tarn_run` does, but honor the
    // `no_default_excludes` override that the ticket calls out for the
    // agent surface.
    let workspace_root = if cwd.explicit {
        cwd.path.clone()
    } else {
        config::find_project_root(&cwd.path).unwrap_or_else(|| cwd.path.clone())
    };
    let files = discover_impact_files(&workspace_root, Some(path_str), no_default_excludes)?;
    if files.is_empty() {
        return Err(ToolError::new(ERR_NO_TESTS, "No .tarn.yaml files found")
            .with_data(json!({ "path": path_str })));
    }

    let opts = build_run_opts();
    let outputs = if selectors.is_empty() {
        execute_and_persist(
            &cwd,
            &files,
            env_name,
            &cli_vars,
            &tag_filter,
            &[],
            &opts,
            &["tarn-mcp".to_string(), "run-agent".to_string()],
        )?
    } else {
        execute_and_persist_with_selectors(
            &cwd,
            &files,
            env_name,
            &cli_vars,
            &tag_filter,
            &selectors,
            &opts,
            &["tarn-mcp".to_string(), "run-agent".to_string()],
        )?
    };

    build_run_response(&outputs, ReportMode::Agent)
}

/// `tarn_last_root_causes` — failures-first read. Returns only the
/// root-cause groups (NAZ-402), skipping the wider failures envelope,
/// so the agent can immediately plan a fix without filtering cascade
/// fallout itself.
pub fn tarn_last_root_causes(params: &Value) -> Result<Value, ToolError> {
    let workspace_root = resolve_workspace_root(params)?;
    let run_id = resolve_run_id_param(&workspace_root, params)?;
    let run_dir = report::run_dir::run_directory(&workspace_root, &run_id);
    let failures_path = run_dir.join("failures.json");
    let doc: FailuresDoc = read_json_artifact(&failures_path)?;

    let built = report::failures_command::build_report(&doc, failures_path.display().to_string());
    let groups_value = serde_json::to_value(&built.groups).map_err(|e| {
        ToolError::new(
            ERR_ARTIFACT_PARSE,
            format!("failed to serialise failure groups: {}", e),
        )
    })?;

    Ok(json!({
        "schema_version": report::failures_command::FAILURES_REPORT_SCHEMA_VERSION,
        "run_id": run_id,
        "workspace_root": workspace_root.display().to_string(),
        "groups": groups_value,
        "total_failures": built.total_failures,
        "total_cascades": built.total_cascades,
        "artifacts": artifact_paths(&run_dir),
    }))
}

/// `tarn_pack_context` — assemble a remediation bundle (NAZ-414) from a
/// prior run's artifacts. Supports the same narrowing (failed-only,
/// files/tests filters) and both JSON and markdown render targets the
/// CLI offers.
pub fn tarn_pack_context(params: &Value) -> Result<Value, ToolError> {
    let workspace_root = resolve_workspace_root(params)?;

    // `run_id` is optional: when omitted, read the `.tarn/` pointers so
    // the tool works the same way `tarn pack-context` does without
    // `--run`.
    let (summary_path, failures_path, report_path, state_path, run_dir_opt, resolved_run_id) =
        match params.get("run_id").and_then(|v| v.as_str()) {
            Some(alias) => {
                let run_id =
                    report::run_dir::resolve_run_id(&workspace_root, alias).map_err(|e| {
                        ToolError::new(ERR_RUN_ID_UNKNOWN, e.to_string()).with_data(json!({
                            "run_id": alias,
                        }))
                    })?;
                let dir = report::run_dir::run_directory(&workspace_root, &run_id);
                (
                    dir.join("summary.json"),
                    dir.join("failures.json"),
                    dir.join("report.json"),
                    dir.join("state.json"),
                    Some(dir),
                    Some(run_id),
                )
            }
            None => {
                let tarn_dir = workspace_root.join(".tarn");
                (
                    tarn_dir.join("summary.json"),
                    tarn_dir.join("failures.json"),
                    tarn_dir.join("last-run.json"),
                    tarn_dir.join("state.json"),
                    None,
                    None,
                )
            }
        };

    let summary: report::summary::SummaryDoc = read_json_artifact(&summary_path)?;
    let failures: FailuresDoc = read_json_artifact(&failures_path)?;

    // `report.json` and `state.json` are best-effort enrichment. A parse
    // error blocks execution because silent corruption would hide real
    // issues; a missing file degrades gracefully to `None`.
    let report_value: Option<Value> = if report_path.is_file() {
        Some(read_json_artifact::<Value>(&report_path)?)
    } else {
        None
    };
    let state_doc: Option<report::state_writer::StateDoc> = if state_path.is_file() {
        Some(read_json_artifact::<report::state_writer::StateDoc>(
            &state_path,
        )?)
    } else {
        None
    };

    let failed_only = params
        .get("failed")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let files_filter = parse_string_array(params, "files")?;
    let tests_filter = parse_string_array(params, "tests")?;

    let inputs = report::pack_context::PackContextInputs {
        summary: &summary,
        failures: &failures,
        report: report_value.as_ref(),
        state: state_doc.as_ref(),
        run_dir: run_dir_opt.as_deref(),
        file_filters: &files_filter,
        test_filters: &tests_filter,
        failed_only,
        workspace_root: &workspace_root,
    };
    let mut pack = report::pack_context::build(&inputs);

    let max_chars = params
        .get("max_chars")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(12_000);

    let format = params
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("json")
        .to_ascii_lowercase();

    let artifacts = match &run_dir_opt {
        Some(dir) => artifact_paths(dir),
        None => Value::Null,
    };

    match format.as_str() {
        "markdown" | "md" => {
            let rendered = report::pack_context::render_markdown(&mut pack, max_chars);
            Ok(json!({
                "schema_version": 1,
                "run_id": resolved_run_id,
                "workspace_root": workspace_root.display().to_string(),
                "markdown": rendered,
                "artifacts": artifacts,
            }))
        }
        "json" => {
            // Apply truncation in-place, then surface the bundle as the
            // structured object it already is — no need to re-parse a
            // rendered string.
            report::pack_context::apply_truncation(
                &mut pack,
                max_chars,
                report::pack_context::RenderFormat::Json,
            );
            let bundle = serde_json::to_value(&pack).map_err(|e| {
                ToolError::new(
                    ERR_PACK_CONTEXT_INVALID_INPUT,
                    format!("failed to serialise pack-context: {}", e),
                )
            })?;
            Ok(json!({
                "schema_version": 1,
                "run_id": resolved_run_id,
                "workspace_root": workspace_root.display().to_string(),
                "bundle": bundle,
                "artifacts": artifacts,
            }))
        }
        other => Err(ToolError::new(
            ERR_PACK_CONTEXT_INVALID_INPUT,
            format!(
                "unknown pack-context format '{}' (expected json|markdown)",
                other
            ),
        )
        .with_data(json!({ "param": "format", "got": other }))),
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
    fn tarn_impact_rejects_no_inputs_with_hint() {
        // A completely empty input set must surface a structured error
        // pointing the agent at the fields that would satisfy the tool.
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("tarn.config.yaml"), "test_dir: tests\n").unwrap();
        let err = tarn_impact(&json!({ "cwd": tmp.path().to_string_lossy() }))
            .expect_err("missing inputs must error");
        assert_eq!(err.code, ERR_IMPACT_INVALID_INPUT);
        let data = err.data.expect("error carries structured data");
        assert!(data.get("hint").is_some(), "hint must be present");
    }

    #[test]
    fn tarn_scaffold_rejects_missing_mode() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("tarn.config.yaml"), "test_dir: tests\n").unwrap();
        let err = tarn_scaffold(&json!({ "cwd": tmp.path().to_string_lossy() }))
            .expect_err("missing mode must error");
        assert_eq!(err.code, ERR_SCAFFOLD_INVALID_INPUT);
        assert!(err.data.is_some());
    }

    #[test]
    fn tarn_scaffold_rejects_unknown_mode() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("tarn.config.yaml"), "test_dir: tests\n").unwrap();
        let err = tarn_scaffold(&json!({
            "cwd": tmp.path().to_string_lossy(),
            "mode": "bogus",
        }))
        .expect_err("unknown mode must error");
        assert_eq!(err.code, ERR_SCAFFOLD_INVALID_INPUT);
    }

    #[test]
    fn tarn_scaffold_rejects_multiple_mode_payloads() {
        // Two payload objects must short-circuit even before the mode
        // handler runs — otherwise an ambiguous intent might silently
        // collapse to whichever branch the dispatcher happens to pick.
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("tarn.config.yaml"), "test_dir: tests\n").unwrap();
        let err = tarn_scaffold(&json!({
            "cwd": tmp.path().to_string_lossy(),
            "mode": "explicit",
            "explicit": { "method": "GET", "url": "http://example.com/" },
            "curl": { "command": "curl http://example.com" },
        }))
        .expect_err("ambiguous payloads must error");
        assert_eq!(err.code, ERR_SCAFFOLD_INVALID_INPUT);
    }

    #[test]
    fn parse_string_array_rejects_non_string_elements() {
        let err = parse_string_array(&json!({ "files": [1, 2, 3] }), "files").unwrap_err();
        assert_eq!(err.code, ERR_INVALID_PARAM);
        assert!(err.message.contains("files[0]"));
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
