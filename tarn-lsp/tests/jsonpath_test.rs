//! End-to-end integration tests for `workspace/executeCommand` with
//! the `tarn.evaluateJsonpath` command.
//!
//! Drives the full `initialize → executeCommand → shutdown → exit`
//! loop over an in-memory `lsp_server::Connection`. Unit tests under
//! `src/jsonpath_eval.rs` cover the pure helpers; this file confirms
//! the capability advertisement, server dispatch, and round-trip
//! serialisation all behave correctly from a client's perspective.
//!
//! The response envelope shipped with NAZ-254 is
//! `{ "schema_version": 1, "data": { "result": ..., ... } }` — every
//! assertion below checks that exact shape rather than the legacy
//! `{ "matches": [...] }` form.

use std::thread;
use std::time::{Duration, Instant};

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{Exit, Initialized, Notification as _};
use lsp_types::request::{ExecuteCommand, Initialize, Request as _, Shutdown};
use lsp_types::{
    ClientCapabilities, ExecuteCommandParams, InitializeParams, InitializedParams,
    PartialResultParams, WorkDoneProgressParams,
};

#[test]
fn execute_jsonpath_inline_happy_path_returns_matches_envelope() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let response = request_execute_command(
        &client_conn,
        "tarn.evaluateJsonpath",
        vec![serde_json::json!({
            "path": "$.items[*].id",
            "response": {"items": [{"id": 1}, {"id": 2}, {"id": 3}]}
        })],
    );
    assert_eq!(response["schema_version"], serde_json::json!(1));
    assert_eq!(response["data"]["result"], serde_json::json!("match"));
    assert_eq!(response["data"]["value"], serde_json::json!(1));
    assert_eq!(response["data"]["values"], serde_json::json!([1, 2, 3]));

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn execute_jsonpath_step_ref_happy_path_reads_disk_sidecar() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    // Build a real sidecar on disk the way the NAZ-304 writer will.
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("fixture.tarn.yaml");
    std::fs::write(&file, "name: fixture\n").unwrap();
    let dir = tmp.path().join("fixture.tarn.yaml.last-run").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let sidecar = dir.join("list-items.response.json");
    std::fs::write(&sidecar, br#"{"items":[{"id":42}]}"#).unwrap();

    let args = vec![serde_json::json!({
        "path": "$.items[0].id",
        "step": {
            "file": file.display().to_string(),
            "test": "main",
            "step": "list items",
        }
    })];
    let response = request_execute_command(&client_conn, "tarn.evaluateJsonpath", args);
    assert_eq!(response["data"]["result"], serde_json::json!("match"));
    assert_eq!(response["data"]["value"], serde_json::json!(42));

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn execute_jsonpath_bad_path_returns_invalid_params_error() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let err = request_execute_command_expect_error(
        &client_conn,
        "tarn.evaluateJsonpath",
        vec![serde_json::json!({
            "path": "$.[not valid",
            "response": {}
        })],
    );
    assert_eq!(
        err.code,
        ErrorCode::InvalidParams as i32,
        "bad path must return InvalidParams, got {err:?}"
    );
    assert!(
        err.message.contains("invalid JSONPath expression"),
        "error message must mention the JSONPath parse failure, got: {}",
        err.message
    );

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn execute_jsonpath_step_ref_missing_sidecar_returns_no_fixture_variant() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("fixture.tarn.yaml");
    std::fs::write(&file, "name: fixture\n").unwrap();
    // No sidecar on disk — the lookup must resolve to the NoFixture
    // variant of the three-variant return shape (NAZ-254).

    let response = request_execute_command(
        &client_conn,
        "tarn.evaluateJsonpath",
        vec![serde_json::json!({
            "path": "$.x",
            "step": {
                "file": file.display().to_string(),
                "test": "main",
                "step": "list items",
            }
        })],
    );
    assert_eq!(response["data"]["result"], serde_json::json!("no_fixture"));
    assert!(response["data"]["message"].as_str().unwrap().contains("no fixture"));

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn execute_jsonpath_no_match_returns_top_keys() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let response = request_execute_command(
        &client_conn,
        "tarn.evaluateJsonpath",
        vec![serde_json::json!({
            "path": "$.missing",
            "response": {"alpha": 1, "beta": 2}
        })],
    );
    assert_eq!(response["data"]["result"], serde_json::json!("no_match"));
    let mut keys: Vec<String> = response["data"]["available_top_keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    keys.sort();
    assert_eq!(keys, vec!["alpha", "beta"]);

    shutdown_and_join(client_conn, server_thread);
}

// ---------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------

fn request_execute_command(
    client_conn: &Connection,
    command: &str,
    arguments: Vec<serde_json::Value>,
) -> serde_json::Value {
    let req_id: RequestId = 9307.into();
    let params = ExecuteCommandParams {
        command: command.to_owned(),
        arguments,
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    client_conn
        .sender
        .send(Message::Request(Request {
            id: req_id.clone(),
            method: ExecuteCommand::METHOD.to_owned(),
            params: serde_json::to_value(params).unwrap(),
        }))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for executeCommand response");
        }
        match client_conn.receiver.recv_timeout(remaining) {
            Ok(Message::Response(Response { id, result, error })) if id == req_id => {
                assert!(error.is_none(), "executeCommand returned error: {error:?}");
                return result.expect("executeCommand had neither result nor error");
            }
            Ok(_) => {}
            Err(e) => panic!("recv failed while waiting for executeCommand response: {e}"),
        }
    }
}

fn request_execute_command_expect_error(
    client_conn: &Connection,
    command: &str,
    arguments: Vec<serde_json::Value>,
) -> lsp_server::ResponseError {
    let req_id: RequestId = 9308.into();
    let params = ExecuteCommandParams {
        command: command.to_owned(),
        arguments,
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    client_conn
        .sender
        .send(Message::Request(Request {
            id: req_id.clone(),
            method: ExecuteCommand::METHOD.to_owned(),
            params: serde_json::to_value(params).unwrap(),
        }))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for executeCommand response");
        }
        match client_conn.receiver.recv_timeout(remaining) {
            Ok(Message::Response(Response { id, error, .. })) if id == req_id => {
                return error.expect("expected an error response, got success");
            }
            Ok(_) => {}
            Err(e) => panic!("recv failed while waiting for executeCommand error: {e}"),
        }
    }
}

fn handshake(client_conn: &Connection) {
    let init_id: RequestId = 1.into();
    client_conn
        .sender
        .send(Message::Request(Request {
            id: init_id.clone(),
            method: Initialize::METHOD.to_owned(),
            params: serde_json::to_value(InitializeParams {
                capabilities: ClientCapabilities::default(),
                ..Default::default()
            })
            .unwrap(),
        }))
        .unwrap();

    loop {
        let msg = client_conn
            .receiver
            .recv()
            .expect("connection closed before initialize response");
        if let Message::Response(resp) = msg {
            if resp.id == init_id {
                assert!(resp.error.is_none(), "initialize failed: {:?}", resp.error);
                // Sanity-check: the advertised capabilities must include
                // `executeCommandProvider` with `tarn.evaluateJsonpath`.
                let caps: serde_json::Value = resp.result.expect("initialize result");
                let commands = &caps["capabilities"]["executeCommandProvider"]["commands"];
                assert!(
                    commands
                        .as_array()
                        .map(|a| a.iter().any(|v| v == "tarn.evaluateJsonpath"))
                        .unwrap_or(false),
                    "capability missing tarn.evaluateJsonpath, got {commands:?}"
                );
                break;
            }
        }
    }

    client_conn
        .sender
        .send(Message::Notification(Notification {
            method: Initialized::METHOD.to_owned(),
            params: serde_json::to_value(InitializedParams {}).unwrap(),
        }))
        .unwrap();

    // Silence the unused-struct warning from clippy for
    // `PartialResultParams` on this tiny integration-test file. The
    // helper pattern used by the other integration tests brings the
    // struct in for symmetry even though executeCommand does not
    // consume it. Without this reference the import would be dead.
    let _ = PartialResultParams::default();
}

fn shutdown_and_join(client_conn: Connection, server_thread: thread::JoinHandle<()>) {
    let shutdown_id: RequestId = 9999.into();
    client_conn
        .sender
        .send(Message::Request(Request {
            id: shutdown_id.clone(),
            method: Shutdown::METHOD.to_owned(),
            params: serde_json::Value::Null,
        }))
        .unwrap();

    loop {
        match client_conn.receiver.recv() {
            Ok(Message::Response(resp)) if resp.id == shutdown_id => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }

    client_conn
        .sender
        .send(Message::Notification(Notification {
            method: Exit::METHOD.to_owned(),
            params: serde_json::Value::Null,
        }))
        .unwrap();
    drop(client_conn);
    server_thread.join().expect("server thread panicked");
}
