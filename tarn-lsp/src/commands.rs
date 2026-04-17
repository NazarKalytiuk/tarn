//! Central registry for every stable `workspace/executeCommand`
//! identifier tarn-lsp advertises.
//!
//! The goal of this module is to be the one place a reader can find
//! the full list of commands — every consumer (`docs/commands.json`,
//! the capability struct, the server dispatcher) pulls from
//! [`ALL_COMMAND_IDS`] so we cannot accidentally ship an advertised
//! command without a handler or vice versa.
//!
//! Commands not fully implemented in this epic (debug/test runner
//! integration, diff-against-last-passing, run-last-failures) return
//! a typed `not_yet_implemented` stub so LLM clients get a
//! deterministic error they can react to instead of `MethodNotFound`.
//! NAZ-256 will fill the stubs in without changing their ids.

use lsp_server::{ErrorCode, ResponseError};
use lsp_types::ExecuteCommandParams;
use serde_json::Value;

use crate::envelope;
use crate::explain_failure::{workspace_explain_failure, EXPLAIN_FAILURE_COMMAND};
use crate::fixtures::{
    workspace_clear_fixtures, workspace_get_fixture, CLEAR_FIXTURES_COMMAND, GET_FIXTURE_COMMAND,
};
use crate::jsonpath_eval::{dispatch_evaluate_jsonpath, EVALUATE_JSONPATH_COMMAND};
use crate::server::ServerState;

/// Full list of stable command ids — **source of truth**.
///
/// Keep in sync with `docs/commands.json` (the generator test uses
/// this list as the authoritative order).
pub const ALL_COMMAND_IDS: &[&str] = &[
    "tarn.runTest",
    "tarn.runStep",
    "tarn.runFile",
    "tarn.debugTest",
    "tarn.runLastFailures",
    EXPLAIN_FAILURE_COMMAND,
    "tarn.diffLastPassing",
    EVALUATE_JSONPATH_COMMAND,
    "tarn.getCaptureState",
    "tarn.captureField",
    "tarn.scaffoldAssertFromResponse",
    "tarn.renameCapture",
    GET_FIXTURE_COMMAND,
    CLEAR_FIXTURES_COMMAND,
];

/// Central dispatcher for `workspace/executeCommand`. The server
/// loop forwards every request here; unknown ids collapse to
/// `MethodNotFound` so clients learn quickly which commands are
/// supported.
pub fn dispatch(
    state: &mut ServerState,
    params: ExecuteCommandParams,
) -> Result<Value, ResponseError> {
    match params.command.as_str() {
        EVALUATE_JSONPATH_COMMAND => dispatch_evaluate_jsonpath(params),
        GET_FIXTURE_COMMAND => workspace_get_fixture(state, params),
        CLEAR_FIXTURES_COMMAND => workspace_clear_fixtures(state, params),
        EXPLAIN_FAILURE_COMMAND => workspace_explain_failure(state, params),
        // NAZ-256 (test-runner integration) will flesh these in. We
        // return a typed stub so LLM clients can detect the gap and
        // fall back to running `tarn` on the command line rather
        // than hitting `MethodNotFound` and thinking the LSP is
        // broken.
        "tarn.runTest"
        | "tarn.runStep"
        | "tarn.runFile"
        | "tarn.debugTest"
        | "tarn.runLastFailures"
        | "tarn.diffLastPassing"
        | "tarn.getCaptureState"
        | "tarn.captureField"
        | "tarn.scaffoldAssertFromResponse"
        | "tarn.renameCapture" => envelope::wrap(serde_json::json!({
            "error": "not_yet_implemented",
            "since": "schema_version: 1",
            "command": params.command,
        }))
        .map_err(internal_error_from_serde),
        other => Err(ResponseError {
            code: ErrorCode::MethodNotFound as i32,
            message: format!(
                "workspace/executeCommand: unknown command `{other}`. Known commands: {:?}",
                ALL_COMMAND_IDS
            ),
            data: None,
        }),
    }
}

fn internal_error_from_serde(err: serde_json::Error) -> ResponseError {
    ResponseError {
        code: ErrorCode::InternalError as i32,
        message: format!("tarn command dispatch: failed to serialise result: {err}"),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_command_returns_method_not_found() {
        let mut state = ServerState::new();
        let params = ExecuteCommandParams {
            command: "tarn.unknownCommand".into(),
            arguments: Vec::new(),
            work_done_progress_params: Default::default(),
        };
        let err = dispatch(&mut state, params).unwrap_err();
        assert_eq!(err.code, ErrorCode::MethodNotFound as i32);
        assert!(err.message.contains("tarn.unknownCommand"));
    }

    #[test]
    fn stub_command_returns_not_yet_implemented_marker() {
        let mut state = ServerState::new();
        let params = ExecuteCommandParams {
            command: "tarn.debugTest".into(),
            arguments: Vec::new(),
            work_done_progress_params: Default::default(),
        };
        let resp = dispatch(&mut state, params).expect("ok");
        assert_eq!(resp["data"]["error"], json!("not_yet_implemented"));
        assert_eq!(resp["data"]["command"], json!("tarn.debugTest"));
    }

    #[test]
    fn every_advertised_command_is_handled_or_stubbed() {
        let mut state = ServerState::new();
        for cmd in ALL_COMMAND_IDS {
            let params = ExecuteCommandParams {
                command: (*cmd).into(),
                arguments: Vec::new(),
                work_done_progress_params: Default::default(),
            };
            let outcome = dispatch(&mut state, params);
            // Fully-implemented commands will error on empty args
            // (InvalidParams). That is fine — we just want to
            // confirm none of them falls through to MethodNotFound.
            if let Err(err) = outcome {
                assert_ne!(
                    err.code,
                    ErrorCode::MethodNotFound as i32,
                    "advertised command {cmd} is not routed in dispatch()"
                );
            }
        }
    }
}
