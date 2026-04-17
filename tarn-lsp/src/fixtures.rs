//! `workspace/executeCommand` handlers for fixture store inspection
//! and maintenance.
//!
//! * [`GET_FIXTURE_COMMAND`] — read the most recent fixture for a
//!   `(file, test, step)` triple.
//! * [`CLEAR_FIXTURES_COMMAND`] — delete every fixture under a
//!   workspace, or under a single file.
//!
//! Both commands share the [`crate::envelope`] wrapper so clients can
//! read the `schema_version` field before trusting the payload.
//!
//! ### Why the workspace root is a parameter
//!
//! tarn-lsp does not keep the workspace root in [`crate::server::ServerState`]
//! directly — [`crate::workspace::WorkspaceIndex`] stores it but only
//! exposes it through its own methods. We fetch it from the server
//! state where possible and fall back to the file's closest ancestor
//! holding a `.tarn/` directory otherwise, matching the heuristic
//! [`crate::code_actions::response_source::DiskResponseSource`] uses.

use std::path::{Path, PathBuf};

use lsp_server::{ErrorCode, ResponseError};
use lsp_types::{ExecuteCommandParams, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::envelope;
use crate::server::ServerState;

/// Stable LSP command id for [`workspace_get_fixture`].
pub const GET_FIXTURE_COMMAND: &str = "tarn.getFixture";

/// Stable LSP command id for [`workspace_clear_fixtures`].
pub const CLEAR_FIXTURES_COMMAND: &str = "tarn.clearFixtures";

/// Argument shape for `tarn.getFixture`.
///
/// `step` is identified by **name** (`String`) or **index** (`Number`)
/// via [`StepSelector`]. Accepting both keeps the surface friendly to
/// LLM clients that have already parsed the YAML and know the index,
/// while legacy clients that only know the display name still work.
#[derive(Debug, Clone, Deserialize)]
pub struct GetFixtureArgs {
    pub file: String,
    pub test: String,
    pub step: StepSelector,
}

/// Either a 0-based integer index or a step name. The untagged
/// deserializer picks based on JSON type.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StepSelector {
    Index(usize),
    Name(String),
}

/// Argument shape for `tarn.clearFixtures`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ClearFixturesArgs {
    /// When set, removes only the subtree for that file. When
    /// absent, every fixture under `.tarn/fixtures/` is deleted.
    #[serde(default)]
    pub file: Option<String>,
}

/// Response shape for `tarn.getFixture` — either the fixture JSON or
/// a typed `no-fixture` marker.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum GetFixtureResponse {
    Ok(Value),
    Missing { error: &'static str, message: String },
}

/// Response shape for `tarn.clearFixtures` — reports how many
/// subtrees were touched and whether the root was removed.
#[derive(Debug, Clone, Serialize)]
pub struct ClearFixturesResponse {
    pub cleared: bool,
    pub scope: String,
}

/// Dispatch `tarn.getFixture`. Returns the wrapped fixture JSON or a
/// typed "no fixture recorded" sentinel — never an RPC error, so the
/// client can show the sentinel inline without retry logic.
pub fn workspace_get_fixture(
    state: &ServerState,
    params: ExecuteCommandParams,
) -> Result<Value, ResponseError> {
    let args = parse_args::<GetFixtureArgs>(&params.arguments, GET_FIXTURE_COMMAND)?;
    let file = parse_file_arg(&args.file)?;
    let root = workspace_root(state, &file);
    let step_index = resolve_step_index(&file, &args.test, &args.step).ok_or_else(|| {
        invalid_params(format!(
            "tarn.getFixture: step `{}` not found in test `{}` of `{}`",
            describe_selector(&args.step),
            args.test,
            file.display()
        ))
    })?;

    match tarn::report::fixture_writer::read_latest_fixture(&root, &file, &args.test, step_index) {
        Some(fixture) => {
            let fx_value = serde_json::to_value(&fixture).map_err(internal_error_from_serde)?;
            let payload = GetFixtureResponse::Ok(fx_value);
            envelope::wrap(payload).map_err(internal_error_from_serde)
        }
        None => {
            let payload = GetFixtureResponse::Missing {
                error: "no-fixture",
                message: format!(
                    "no fixture recorded for step {} in test `{}`; run the test once to populate",
                    step_index, args.test
                ),
            };
            envelope::wrap(payload).map_err(internal_error_from_serde)
        }
    }
}

/// Dispatch `tarn.clearFixtures`. Returns the wrapped status blob.
pub fn workspace_clear_fixtures(
    state: &ServerState,
    params: ExecuteCommandParams,
) -> Result<Value, ResponseError> {
    let args = if params.arguments.is_empty() {
        ClearFixturesArgs::default()
    } else {
        parse_args::<ClearFixturesArgs>(&params.arguments, CLEAR_FIXTURES_COMMAND)?
    };

    let scope: String;
    let root = workspace_root_from_state(state)
        .or_else(|| args.file.as_ref().and_then(|f| {
            let p = PathBuf::from(f);
            crate::code_actions::response_source::workspace_root_for(&p)
                .or_else(|| p.parent().map(Path::to_path_buf))
        }))
        .ok_or_else(|| {
            invalid_params(
                "tarn.clearFixtures: unable to determine workspace root; open a Tarn buffer first",
            )
        })?;

    let file_path = if let Some(f) = args.file.as_ref() {
        let p = parse_file_arg(f)?;
        scope = format!("file:{}", p.display());
        Some(p)
    } else {
        scope = format!("workspace:{}", root.display());
        None
    };

    tarn::report::fixture_writer::clear_fixtures(&root, file_path.as_deref()).map_err(|err| {
        ResponseError {
            code: ErrorCode::InternalError as i32,
            message: format!("tarn.clearFixtures: failed to remove fixtures: {err}"),
            data: None,
        }
    })?;

    envelope::wrap(ClearFixturesResponse {
        cleared: true,
        scope,
    })
    .map_err(internal_error_from_serde)
}

/// Resolve the workspace root from the server state or, as a
/// fallback, from the file path itself. Used by every fixture
/// handler so the NAZ-252 `.tarn/fixtures/` layout anchors to the
/// same root the runner wrote into.
pub fn workspace_root(state: &ServerState, file: &Path) -> PathBuf {
    if let Some(root) = workspace_root_from_state(state) {
        return root;
    }
    crate::code_actions::response_source::workspace_root_for(file)
        .or_else(|| file.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn workspace_root_from_state(state: &ServerState) -> Option<PathBuf> {
    let url = state.workspace_index.root()?;
    url.to_file_path().ok()
}

fn parse_args<T: for<'de> Deserialize<'de>>(
    args: &[Value],
    command: &str,
) -> Result<T, ResponseError> {
    let first = args
        .first()
        .ok_or_else(|| invalid_params(format!("{command} requires one argument object")))?;
    serde_json::from_value::<T>(first.clone()).map_err(|e| {
        invalid_params(format!("{command}: invalid argument shape: {e}"))
    })
}

fn parse_file_arg(file: &str) -> Result<PathBuf, ResponseError> {
    if let Ok(url) = Url::parse(file) {
        if let Ok(p) = url.to_file_path() {
            return Ok(p);
        }
    }
    Ok(PathBuf::from(file))
}

fn resolve_step_index(file: &Path, test: &str, step: &StepSelector) -> Option<usize> {
    match step {
        StepSelector::Index(i) => Some(*i),
        StepSelector::Name(name) => {
            let source = std::fs::read_to_string(file).ok()?;
            let parsed = tarn::parser::parse_str(&source, file).ok()?;
            let steps = match test {
                "setup" => &parsed.setup,
                "teardown" => &parsed.teardown,
                "<flat>" => &parsed.steps,
                named => parsed
                    .tests
                    .get(named)
                    .map(|g| &g.steps)
                    .unwrap_or(&parsed.steps),
            };
            steps.iter().position(|s| &s.name == name)
        }
    }
}

fn describe_selector(step: &StepSelector) -> String {
    match step {
        StepSelector::Index(i) => format!("#{i}"),
        StepSelector::Name(n) => format!("'{n}'"),
    }
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
        message: format!("tarn fixture command: failed to serialise result: {err}"),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::COMMAND_SCHEMA_VERSION;
    use serde_json::json;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_file(dir: &Path, rel: &str, body: &str) -> PathBuf {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    }

    fn write_fixture(
        root: &Path,
        file: &Path,
        test: &str,
        step_index: usize,
        passed: bool,
    ) -> tarn::report::fixture_writer::Fixture {
        let fx = tarn::report::fixture_writer::Fixture {
            recorded_at: "2026-04-17T00:00:00Z".into(),
            request: tarn::report::fixture_writer::FixtureRequest {
                method: "GET".into(),
                url: "http://x.test/users".into(),
                headers: Default::default(),
                body: None,
            },
            response: Some(tarn::report::fixture_writer::FixtureResponse {
                status: if passed { 200 } else { 500 },
                headers: Default::default(),
                body: Some(json!({"items": [{"id": 1}]})),
            }),
            captures: Default::default(),
            passed,
            failure_message: if passed {
                None
            } else {
                Some("status mismatch".into())
            },
            duration_ms: 10,
        };
        let config = tarn::report::fixture_writer::FixtureWriteConfig {
            enabled: true,
            workspace_root: root.to_path_buf(),
            retention: 5,
        };
        tarn::report::fixture_writer::write_step_fixture(&config, file, test, step_index, &fx)
            .unwrap();
        fx
    }

    #[test]
    fn get_fixture_returns_recorded_payload_when_present() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file = write_file(
            &root,
            "tests/users.tarn.yaml",
            r#"
name: Users
tests:
  happy:
    steps:
      - name: list
        request:
          method: GET
          url: http://x.test/users
"#,
        );
        let _ = write_fixture(&root, &file, "happy", 0, true);

        let state = ServerState::new();
        let params = ExecuteCommandParams {
            command: GET_FIXTURE_COMMAND.into(),
            arguments: vec![json!({
                "file": file.to_string_lossy(),
                "test": "happy",
                "step": "list",
            })],
            work_done_progress_params: Default::default(),
        };
        let resp = workspace_get_fixture(&state, params).expect("ok");
        assert_eq!(resp["schema_version"], json!(COMMAND_SCHEMA_VERSION));
        let data = &resp["data"];
        assert_eq!(data["passed"], json!(true));
        assert_eq!(data["response"]["status"], json!(200));
    }

    #[test]
    fn get_fixture_reports_no_fixture_when_nothing_recorded() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file = write_file(
            &root,
            "tests/users.tarn.yaml",
            r#"
name: Users
tests:
  happy:
    steps:
      - name: list
        request:
          method: GET
          url: http://x.test/users
"#,
        );

        let state = ServerState::new();
        let params = ExecuteCommandParams {
            command: GET_FIXTURE_COMMAND.into(),
            arguments: vec![json!({
                "file": file.to_string_lossy(),
                "test": "happy",
                "step": 0,
            })],
            work_done_progress_params: Default::default(),
        };
        let resp = workspace_get_fixture(&state, params).expect("ok");
        assert_eq!(resp["data"]["error"], json!("no-fixture"));
    }

    #[test]
    fn get_fixture_rejects_unknown_step_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file = write_file(
            &root,
            "tests/users.tarn.yaml",
            r#"
name: Users
tests:
  happy:
    steps:
      - name: list
        request:
          method: GET
          url: http://x.test/users
"#,
        );

        let state = ServerState::new();
        let params = ExecuteCommandParams {
            command: GET_FIXTURE_COMMAND.into(),
            arguments: vec![json!({
                "file": file.to_string_lossy(),
                "test": "happy",
                "step": "does-not-exist",
            })],
            work_done_progress_params: Default::default(),
        };
        let err = workspace_get_fixture(&state, params).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams as i32);
        assert!(err.message.contains("does-not-exist"));
    }

    #[test]
    fn clear_fixtures_wipes_subtree() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        // Create .tarn so workspace_root_for picks up the root.
        std::fs::create_dir_all(root.join(".tarn")).unwrap();
        let file = write_file(
            &root,
            "tests/users.tarn.yaml",
            r#"
name: Users
tests:
  happy:
    steps:
      - name: list
        request:
          method: GET
          url: http://x.test/users
"#,
        );
        let _ = write_fixture(&root, &file, "happy", 0, true);
        assert!(root.join(".tarn/fixtures").exists());

        let state = ServerState::new();
        let params = ExecuteCommandParams {
            command: CLEAR_FIXTURES_COMMAND.into(),
            arguments: vec![json!({
                "file": file.to_string_lossy(),
            })],
            work_done_progress_params: Default::default(),
        };
        let resp = workspace_clear_fixtures(&state, params).expect("ok");
        assert_eq!(resp["data"]["cleared"], json!(true));

        // The per-file subtree should be gone; the root `.tarn/fixtures`
        // may still exist (it's shared across files).
        let hash_dir = root
            .join(".tarn/fixtures")
            .join(tarn::fixtures::file_path_hash(&root, &file));
        assert!(!hash_dir.exists());
    }
}
