//! `workspace/executeCommand` handler for `tarn.explainFailure`
//! (NAZ-257, Epic NAZ-258).
//!
//! Given a `(file, test?, step?)` locator, the handler aggregates
//! the most recent fixture for the failure, the enclosing run's
//! `.tarn/last-run.json` summary, and the captures from all steps
//! that ran before the failure into a single structured payload
//! an LLM client can feed back into a fix-plan prompt.
//!
//! The structured shape keeps every field uniformly addressable so
//! the LLM does not have to parse free-form prose. Missing pieces
//! (no fixture, no last-run report, no preceding captures) are all
//! represented explicitly rather than elided so the client can tell
//! "not recorded" apart from "no data".

use std::path::{Path, PathBuf};

use lsp_server::{ErrorCode, ResponseError};
use lsp_types::{ExecuteCommandParams, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::envelope;
use crate::server::ServerState;

/// Stable LSP command id advertised in [`crate::capabilities`].
pub const EXPLAIN_FAILURE_COMMAND: &str = "tarn.explainFailure";

/// Argument shape. `test` and `step` are optional: when both are
/// omitted the handler walks `.tarn/last-run.json`, picks the first
/// failure inside `file`, and explains that. This matches the LLM
/// workflow of "just ran `tarn run foo.tarn.yaml`; why did it fail?".
#[derive(Debug, Clone, Deserialize)]
pub struct ExplainArgs {
    pub file: String,
    #[serde(default)]
    pub test: Option<String>,
    #[serde(default)]
    pub step: Option<StepSelector>,
}

/// Same dual-form as [`crate::fixtures::StepSelector`] — step by
/// name or by 0-based index.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StepSelector {
    Index(usize),
    Name(String),
}

/// The structured explanation emitted in `data` under the envelope.
#[derive(Debug, Clone, Serialize)]
pub struct Explanation {
    pub test: String,
    pub step: String,
    /// Short string summarising what assertion was expected to hold
    /// (e.g. `"status == 200"`). Empty when the failure is non-
    /// assertional (runtime, unresolved template).
    pub expected: String,
    /// Observed state at failure time.
    pub actual: ActualState,
    /// Primary failure message, redacted.
    pub failure_message: String,
    /// Captures + step summaries for every step that ran before the
    /// failure in the same test scope. Helps the LLM correlate the
    /// failure with the upstream state that produced it.
    pub preceding_steps: Vec<PrecedingStep>,
    /// Natural-language hint the heuristic engine produced from the
    /// failure signature. Guaranteed non-empty — when no rule fires
    /// we fall back to a generic "no automated hint available".
    pub root_cause_hint: String,
    /// Reserved for future integration with a log backend (NAZ-261).
    pub backend_logs: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActualState {
    pub status: Option<u16>,
    pub body_summary: Option<String>,
    /// Full response body (redacted) when available. Optional so
    /// "no response at all" (connection refused) is represented
    /// explicitly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrecedingStep {
    pub step: String,
    pub captures: Value,
    pub passed: bool,
}

/// Request dispatcher. Returns the wrapped explanation or an LSP
/// `InvalidParams` error when the file cannot be parsed.
pub fn workspace_explain_failure(
    state: &ServerState,
    params: ExecuteCommandParams,
) -> Result<Value, ResponseError> {
    let args = parse_args(&params.arguments)?;
    let file = parse_file_arg(&args.file)?;
    let root = crate::fixtures::workspace_root(state, &file);

    let source = std::fs::read_to_string(&file).map_err(|err| {
        invalid_params(format!(
            "tarn.explainFailure: cannot read `{}`: {err}",
            file.display()
        ))
    })?;
    let parsed = tarn::parser::parse_str(&source, &file).map_err(|err| {
        invalid_params(format!(
            "tarn.explainFailure: parse error in `{}`: {err}",
            file.display()
        ))
    })?;

    // Resolve (test, step_index, step_name). Default to the first
    // failure recorded under `.tarn/last-run.json` for the file when
    // the caller didn't pin one.
    let (test_name, step_index, step_name) = resolve_target(
        &parsed,
        &file,
        &root,
        args.test.as_deref(),
        args.step.as_ref(),
    )?;

    let fixture =
        tarn::report::fixture_writer::read_latest_fixture(&root, &file, &test_name, step_index);

    // Preceding steps — every step in the same scope that ran before
    // the failure. Pulled from the fixture store so the captures we
    // surface are the ones actually produced at run time, not the
    // static values in the YAML.
    let preceding = collect_preceding(&parsed, &root, &file, &test_name, step_index);

    let fixture_for_hint = fixture.clone();
    let (expected, actual, failure_message) = derive_failure_state(&fixture);
    let root_cause_hint = generate_root_cause_hint(
        &fixture_for_hint,
        &failure_message,
        &preceding,
        &parsed,
        &test_name,
        step_index,
    );

    let explanation = Explanation {
        test: test_name,
        step: step_name,
        expected,
        actual,
        failure_message,
        preceding_steps: preceding,
        root_cause_hint,
        backend_logs: None,
    };

    envelope::wrap(explanation).map_err(internal_error_from_serde)
}

fn parse_args(args: &[Value]) -> Result<ExplainArgs, ResponseError> {
    let first = args
        .first()
        .ok_or_else(|| invalid_params(format!("{EXPLAIN_FAILURE_COMMAND} requires one argument")))?;
    serde_json::from_value::<ExplainArgs>(first.clone())
        .map_err(|e| invalid_params(format!("{EXPLAIN_FAILURE_COMMAND}: invalid argument: {e}")))
}

fn parse_file_arg(file: &str) -> Result<PathBuf, ResponseError> {
    if let Ok(url) = Url::parse(file) {
        if let Ok(p) = url.to_file_path() {
            return Ok(p);
        }
    }
    Ok(PathBuf::from(file))
}

fn resolve_target(
    parsed: &tarn::model::TestFile,
    file: &Path,
    root: &Path,
    requested_test: Option<&str>,
    requested_step: Option<&StepSelector>,
) -> Result<(String, usize, String), ResponseError> {
    // When the caller pinned both test and step, use them.
    if let (Some(test), Some(step)) = (requested_test, requested_step) {
        let steps = steps_for_test(parsed, test);
        let idx = match step {
            StepSelector::Index(i) => *i,
            StepSelector::Name(name) => steps
                .iter()
                .position(|s| &s.name == name)
                .ok_or_else(|| {
                    invalid_params(format!(
                        "tarn.explainFailure: step `{name}` not found in test `{test}`"
                    ))
                })?,
        };
        let step_name = steps
            .get(idx)
            .map(|s| s.name.clone())
            .ok_or_else(|| {
                invalid_params(format!(
                    "tarn.explainFailure: step #{idx} out of range in test `{test}`"
                ))
            })?;
        return Ok((test.to_string(), idx, step_name));
    }

    // Otherwise consult `.tarn/last-run.json` for the first failure
    // belonging to this file.
    if let Some(guess) = first_failure_for_file(root, file) {
        return Ok(guess);
    }

    Err(invalid_params(
        "tarn.explainFailure: no (test, step) specified and no failures recorded in .tarn/last-run.json for this file",
    ))
}

fn steps_for_test<'a>(parsed: &'a tarn::model::TestFile, test: &str) -> &'a [tarn::model::Step] {
    match test {
        "setup" => parsed.setup.as_slice(),
        "teardown" => parsed.teardown.as_slice(),
        "<flat>" => parsed.steps.as_slice(),
        named => parsed
            .tests
            .get(named)
            .map(|g| g.steps.as_slice())
            .unwrap_or(parsed.steps.as_slice()),
    }
}

fn first_failure_for_file(root: &Path, file: &Path) -> Option<(String, usize, String)> {
    let report = read_last_run_report(root)?;
    let files = report.get("file_results")?.as_array()?;
    let file_str = file.to_string_lossy();
    for f in files {
        let path = f.get("file")?.as_str()?;
        if !file_str.ends_with(path) && path != file_str.as_ref() {
            continue;
        }
        if let Some(tests) = f.get("test_results").and_then(|v| v.as_array()) {
            for t in tests {
                let test_name = t.get("name")?.as_str()?.to_string();
                if let Some(steps) = t.get("step_results").and_then(|v| v.as_array()) {
                    for (idx, step) in steps.iter().enumerate() {
                        if step.get("passed").and_then(|v| v.as_bool()) == Some(false) {
                            let step_name = step
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            return Some((test_name, idx, step_name));
                        }
                    }
                }
            }
        }
    }
    None
}

fn read_last_run_report(root: &Path) -> Option<Value> {
    let path = root.join(".tarn").join("last-run.json");
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()
}

fn collect_preceding(
    parsed: &tarn::model::TestFile,
    root: &Path,
    file: &Path,
    test: &str,
    step_index: usize,
) -> Vec<PrecedingStep> {
    let steps = steps_for_test(parsed, test);
    let mut out = Vec::new();
    for (idx, step) in steps.iter().enumerate() {
        if idx >= step_index {
            break;
        }
        let fixture =
            tarn::report::fixture_writer::read_latest_fixture(root, file, test, idx);
        match fixture {
            Some(fx) => out.push(PrecedingStep {
                step: step.name.clone(),
                captures: serde_json::Value::Object(fx.captures.clone()),
                passed: fx.passed,
            }),
            None => out.push(PrecedingStep {
                step: step.name.clone(),
                captures: serde_json::Value::Object(Default::default()),
                passed: false,
            }),
        }
    }
    out
}

fn derive_failure_state(
    fixture: &Option<tarn::report::fixture_writer::Fixture>,
) -> (String, ActualState, String) {
    match fixture {
        Some(fx) if !fx.passed => {
            let status = fx.response.as_ref().map(|r| r.status);
            let body = fx.response.as_ref().and_then(|r| r.body.clone());
            let body_summary = body.as_ref().map(summarise_body);
            let expected = expected_from_failure_message(fx.failure_message.as_deref());
            (
                expected,
                ActualState {
                    status,
                    body_summary,
                    body,
                },
                fx.failure_message.clone().unwrap_or_default(),
            )
        }
        Some(fx) => {
            // Fixture says the step passed — the caller may be
            // explaining a flake that recovered. Still surface the
            // response so the LLM can see the current state.
            let status = fx.response.as_ref().map(|r| r.status);
            let body = fx.response.as_ref().and_then(|r| r.body.clone());
            let body_summary = body.as_ref().map(summarise_body);
            (
                String::new(),
                ActualState {
                    status,
                    body_summary,
                    body,
                },
                "step is currently passing; nothing to explain".to_string(),
            )
        }
        None => (
            String::new(),
            ActualState {
                status: None,
                body_summary: None,
                body: None,
            },
            "no fixture recorded for this step; run the test once to populate".to_string(),
        ),
    }
}

fn expected_from_failure_message(message: Option<&str>) -> String {
    let Some(message) = message else {
        return String::new();
    };
    // Assertion failure messages look like "status expected 200, got
    // 500" or "JSONPath $.x: expected ... actual ...". We keep the
    // first "expected ..." clause when present.
    if let Some(idx) = message.to_ascii_lowercase().find("expected") {
        let rest = &message[idx..];
        // Stop at a comma or period to keep the expected clause
        // focused, but only when the delimiter clearly closes the
        // clause — otherwise the whole line is still more useful
        // than truncating at a random comma inside a JSON payload.
        let end = rest.find([',', '\n']).unwrap_or(rest.len());
        return rest[..end].trim().to_string();
    }
    message.lines().next().unwrap_or("").trim().to_string()
}

fn summarise_body(body: &Value) -> String {
    match body {
        Value::Array(a) => format!("Array[{}]", a.len()),
        Value::Object(o) => {
            if let Some(Value::String(msg)) = o.get("message") {
                truncate(msg, 120)
            } else if let Some(Value::String(err)) = o.get("error") {
                truncate(err, 120)
            } else {
                format!("Object {{ {} keys }}", o.len())
            }
        }
        Value::String(s) => truncate(s, 120),
        other => truncate(&other.to_string(), 120),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut cut: String = s.chars().take(max.saturating_sub(3)).collect();
    cut.push_str("...");
    cut
}

fn generate_root_cause_hint(
    fixture: &Option<tarn::report::fixture_writer::Fixture>,
    failure_message: &str,
    preceding: &[PrecedingStep],
    parsed: &tarn::model::TestFile,
    test: &str,
    step_index: usize,
) -> String {
    let message_lower = failure_message.to_ascii_lowercase();

    // Heuristic 1: 5xx server error. Very high signal — hand the
    // LLM a short nudge to look at backend logs instead of tweaking
    // the test.
    if let Some(fx) = fixture {
        if let Some(resp) = fx.response.as_ref() {
            if (500..600).contains(&resp.status) {
                return format!(
                    "HTTP {} from the server — check backend logs; this is almost always a \
                     server-side bug, not a test bug. Rerun with `--verbose` to see the \
                     request that triggered it.",
                    resp.status
                );
            }
            if resp.status == 401 || resp.status == 403 {
                return format!(
                    "HTTP {} — auth failure. Check that the token/capture feeding the \
                     `Authorization` header is still valid and was produced by an earlier \
                     step in this test.",
                    resp.status
                );
            }
        }
    }

    // Heuristic 2: unresolved capture / failed upstream capture.
    if message_lower.contains("unresolved")
        || message_lower.contains("capture")
            && preceding.iter().any(|s| !s.passed)
    {
        if let Some(broken) = preceding.iter().find(|s| !s.passed) {
            return format!(
                "Upstream step `{}` failed to produce its captures, so this step's template \
                 references are unresolved. Fix the root-cause step first.",
                broken.step
            );
        }
    }

    // Heuristic 3: JSONPath mismatch. Detected either by the word
    // "jsonpath" in the message or an assertion label starting with
    // `body $`.
    if message_lower.contains("jsonpath")
        || message_lower.contains("body $")
        || message_lower.contains("json path")
    {
        return "Response shape changed — use `tarn.evaluateJsonpath` against the latest \
                 fixture to see what the path currently matches, then realign the assertion."
            .to_string();
    }

    // Heuristic 4: connection refused / timeout on the request.
    if message_lower.contains("connection refused") || message_lower.contains("timed out") {
        return "Transport-level failure (refusal or timeout). Check that the target service \
                 is actually running and reachable from this host before treating this as a \
                 test regression."
            .to_string();
    }

    // Heuristic 5: assertion on a capture that a later step would
    // rely on. Flag that to the LLM so it doesn't just patch the
    // assertion in place.
    if let Some(step) = steps_for_test(parsed, test).get(step_index) {
        if !step.capture.is_empty() {
            return format!(
                "This step declares captures ({}); if you relax its assertions make sure the \
                 captures still succeed, otherwise every downstream step will cascade-fail.",
                step.capture.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        }
    }

    "No automated hint available — inspect the request/response in the fixture and realign \
     the assertions or the server response."
        .to_string()
}

fn invalid_params(message: impl Into<String>) -> ResponseError {
    ResponseError {
        code: ErrorCode::InvalidParams as i32,
        message: message.into(),
        data: None,
    }
}

fn internal_error_from_serde(err: serde_json::Error) -> ResponseError {
    ResponseError {
        code: ErrorCode::InternalError as i32,
        message: format!("{EXPLAIN_FAILURE_COMMAND}: failed to serialise: {err}"),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_test_file(dir: &Path) -> PathBuf {
        let yaml = r#"
name: Users
tests:
  happy:
    steps:
      - name: login
        request:
          method: POST
          url: http://x.test/login
          body:
            user: alice
        capture:
          token:
            jsonpath: $.token
      - name: list
        request:
          method: GET
          url: "http://x.test/users"
          headers:
            Authorization: "Bearer {{ capture.token }}"
        assert:
          status: 200
"#;
        let path = dir.join("tests/users.tarn.yaml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, yaml).unwrap();
        path
    }

    fn make_fixture(
        status: u16,
        passed: bool,
        failure: Option<&str>,
    ) -> tarn::report::fixture_writer::Fixture {
        tarn::report::fixture_writer::Fixture {
            recorded_at: "2026-04-17T00:00:00Z".into(),
            request: tarn::report::fixture_writer::FixtureRequest {
                method: "GET".into(),
                url: "http://x.test/users".into(),
                headers: Default::default(),
                body: None,
            },
            response: Some(tarn::report::fixture_writer::FixtureResponse {
                status,
                headers: Default::default(),
                body: Some(json!({"error": "boom"})),
            }),
            captures: Default::default(),
            passed,
            failure_message: failure.map(String::from),
            duration_ms: 3,
        }
    }

    fn write_fixture(
        root: &Path,
        file: &Path,
        test: &str,
        step_index: usize,
        fixture: &tarn::report::fixture_writer::Fixture,
    ) {
        let config = tarn::report::fixture_writer::FixtureWriteConfig {
            enabled: true,
            workspace_root: root.to_path_buf(),
            retention: 5,
        };
        tarn::report::fixture_writer::write_step_fixture(&config, file, test, step_index, fixture)
            .unwrap();
    }

    #[test]
    fn explain_returns_5xx_hint_for_server_error() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file = write_test_file(&root);
        let fx = make_fixture(500, false, Some("status: expected 200, got 500"));
        write_fixture(&root, &file, "happy", 1, &fx);

        let params = ExecuteCommandParams {
            command: EXPLAIN_FAILURE_COMMAND.into(),
            arguments: vec![json!({
                "file": file.to_string_lossy(),
                "test": "happy",
                "step": "list",
            })],
            work_done_progress_params: Default::default(),
        };
        let state = ServerState::new();
        let resp = workspace_explain_failure(&state, params).expect("ok");
        let hint = resp["data"]["root_cause_hint"].as_str().unwrap();
        assert!(hint.contains("HTTP 500"), "expected 5xx hint, got: {hint}");
        assert!(!resp["data"]["failure_message"].as_str().unwrap().is_empty());
    }

    #[test]
    fn explain_returns_jsonpath_hint_for_body_mismatch() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file = write_test_file(&root);
        let fx = make_fixture(
            200,
            false,
            Some("body $.users[0].id: expected 1, actual null"),
        );
        write_fixture(&root, &file, "happy", 1, &fx);

        let params = ExecuteCommandParams {
            command: EXPLAIN_FAILURE_COMMAND.into(),
            arguments: vec![json!({
                "file": file.to_string_lossy(),
                "test": "happy",
                "step": 1,
            })],
            work_done_progress_params: Default::default(),
        };
        let state = ServerState::new();
        let resp = workspace_explain_failure(&state, params).expect("ok");
        let hint = resp["data"]["root_cause_hint"].as_str().unwrap();
        assert!(
            hint.to_ascii_lowercase().contains("jsonpath")
                || hint.to_ascii_lowercase().contains("response shape"),
            "expected JSONPath hint, got: {hint}"
        );
    }

    #[test]
    fn explain_returns_upstream_capture_hint() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file = write_test_file(&root);

        // Step 0 failed without producing `token`.
        let step0 = make_fixture(500, false, Some("status: expected 200, got 500"));
        write_fixture(&root, &file, "happy", 0, &step0);
        // Step 1 could not resolve `capture.token`.
        let step1 = make_fixture(
            0,
            false,
            Some("Unresolved template variables: capture.token"),
        );
        // Step 1's response is synthetic — overwrite it with no
        // response so the hint chain falls into the unresolved
        // branch rather than the 5xx branch.
        let mut step1_no_resp = step1.clone();
        step1_no_resp.response = None;
        write_fixture(&root, &file, "happy", 1, &step1_no_resp);

        let params = ExecuteCommandParams {
            command: EXPLAIN_FAILURE_COMMAND.into(),
            arguments: vec![json!({
                "file": file.to_string_lossy(),
                "test": "happy",
                "step": "list",
            })],
            work_done_progress_params: Default::default(),
        };
        let state = ServerState::new();
        let resp = workspace_explain_failure(&state, params).expect("ok");
        let hint = resp["data"]["root_cause_hint"].as_str().unwrap();
        assert!(
            hint.to_ascii_lowercase().contains("upstream"),
            "expected upstream-capture hint, got: {hint}"
        );

        let preceding = resp["data"]["preceding_steps"].as_array().unwrap();
        assert_eq!(preceding.len(), 1);
        assert_eq!(preceding[0]["step"], json!("login"));
    }

    #[test]
    fn explain_errors_when_no_test_and_no_last_run_report() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file = write_test_file(&root);

        let params = ExecuteCommandParams {
            command: EXPLAIN_FAILURE_COMMAND.into(),
            arguments: vec![json!({
                "file": file.to_string_lossy(),
            })],
            work_done_progress_params: Default::default(),
        };
        let state = ServerState::new();
        let err = workspace_explain_failure(&state, params).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams as i32);
        assert!(err.message.contains("no (test, step) specified"));
    }
}
