//! End-to-end integration tests for L3.2 code actions (NAZ-303).
//!
//! Drives the full `initialize → didOpen → codeAction → shutdown →
//! exit` loop over an in-memory `lsp_server::Connection`, the same
//! transport the other Phase L3 handler tests use. The unit tests in
//! `src/code_actions/extract_env.rs` exercise the pure renderer's
//! behaviour; this file only confirms:
//!
//!   * the dispatch in `server.rs` hands the request to the renderer
//!     and returns a `Vec<CodeActionOrCommand>` the client can
//!     deserialize,
//!   * the capability advertisement actually works end-to-end,
//!   * a `.tarn.yaml` buffer with a plain URL under the cursor
//!     produces one `"Extract to env var…"` action with a
//!     `WorkspaceEdit` carrying two edits,
//!   * collision-suffix resolution survives the round-trip,
//!   * unrelated positions (cursor on a step name) return an empty
//!     array rather than an error.

use std::thread;
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidOpenTextDocument, Exit, Initialized, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{CodeActionRequest, Initialize, Request as _, Shutdown};
use lsp_types::{
    ClientCapabilities, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    DidOpenTextDocumentParams, InitializeParams, InitializedParams, PartialResultParams, Position,
    PublishDiagnosticsParams, Range, TextDocumentIdentifier, TextDocumentItem, Url,
    WorkDoneProgressParams,
};

const FIXTURE_NO_ENV: &str = "name: fixture\nsteps:\n  - name: s1\n    request:\n      method: GET\n      url: http://example.com/items\n";

const FIXTURE_COLLISION: &str = "name: fixture\nenv:\n  new_env_key: already taken\nsteps:\n  - name: s1\n    request:\n      method: GET\n      url: http://example.com/items\n";

#[test]
fn code_action_extracts_env_var_with_workspace_edit_and_refactor_kind() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/ca-extract.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, FIXTURE_NO_ENV);
    drain_publish_diagnostics_for(&client_conn, &uri);

    // Cursor inside `http://example.com/items` on line 6 (0-based: 5).
    let range = Range::new(Position::new(5, 15), Position::new(5, 15));
    let actions = request_code_action(&client_conn, &uri, range);
    assert_eq!(
        actions.len(),
        1,
        "expected exactly one extract-env action, got {actions:#?}"
    );

    let action = actions
        .into_iter()
        .next()
        .and_then(|ac| match ac {
            CodeActionOrCommand::CodeAction(a) => Some(a),
            CodeActionOrCommand::Command(_) => None,
        })
        .expect("response must carry a CodeAction, not a bare Command");

    assert_eq!(action.title, "Extract to env var…");
    assert_eq!(action.kind, Some(CodeActionKind::REFACTOR_EXTRACT));
    let edit = action.edit.expect("action must carry a workspace edit");
    let changes = edit.changes.expect("workspace edit must have changes");
    let edits = changes.get(&uri).expect("edits for the current uri");
    assert_eq!(
        edits.len(),
        2,
        "extract env var always emits literal + env block edits, got {edits:#?}"
    );
    let any_interpolation = edits
        .iter()
        .any(|e| e.new_text.contains("{{ env.new_env_key }}"));
    assert!(
        any_interpolation,
        "one edit must replace the literal with the interpolation, got {edits:#?}"
    );
    let any_env_block = edits
        .iter()
        .any(|e| e.new_text.contains("new_env_key: http://example.com/items"));
    assert!(
        any_env_block,
        "one edit must insert the env key into the env block, got {edits:#?}"
    );

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn code_action_on_non_eligible_node_returns_empty_array() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/ca-no-op.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, FIXTURE_NO_ENV);
    drain_publish_diagnostics_for(&client_conn, &uri);

    // Cursor on the step `name: s1` value — extract should decline.
    let range = Range::new(Position::new(2, 12), Position::new(2, 12));
    let actions = request_code_action(&client_conn, &uri, range);
    assert!(
        actions.is_empty(),
        "cursor on step name must not yield an extract-env action, got {actions:#?}"
    );

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn code_action_collision_suffix_flows_through_server_round_trip() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/ca-collide.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, FIXTURE_COLLISION);
    drain_publish_diagnostics_for(&client_conn, &uri);

    // Cursor inside the URL literal. The existing env block already
    // holds `new_env_key`, so the server must suffix the new key with
    // `_2` instead of colliding.
    let range = Range::new(Position::new(7, 15), Position::new(7, 15));
    let actions = request_code_action(&client_conn, &uri, range);
    assert_eq!(actions.len(), 1);
    let action = match actions.into_iter().next().unwrap() {
        CodeActionOrCommand::CodeAction(a) => a,
        CodeActionOrCommand::Command(_) => panic!("expected CodeAction, got Command"),
    };
    let edit = action.edit.expect("workspace edit");
    let changes = edit.changes.expect("changes");
    let edits = changes.get(&uri).expect("edits");
    let any_suffixed_interpolation = edits
        .iter()
        .any(|e| e.new_text.contains("{{ env.new_env_key_2 }}"));
    assert!(
        any_suffixed_interpolation,
        "collision must suffix the env key to new_env_key_2, got {edits:#?}"
    );

    shutdown_and_join(client_conn, server_thread);
}

// ---------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------

fn request_code_action(
    client_conn: &Connection,
    uri: &Url,
    range: Range,
) -> Vec<CodeActionOrCommand> {
    let req_id: RequestId = 9303.into();
    let params = CodeActionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        range,
        context: CodeActionContext {
            diagnostics: Vec::new(),
            only: None,
            trigger_kind: None,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    client_conn
        .sender
        .send(Message::Request(Request {
            id: req_id.clone(),
            method: CodeActionRequest::METHOD.to_owned(),
            params: serde_json::to_value(params).unwrap(),
        }))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for codeAction response");
        }
        match client_conn.receiver.recv_timeout(remaining) {
            Ok(Message::Response(Response { id, result, error })) if id == req_id => {
                assert!(error.is_none(), "codeAction returned error: {error:?}");
                let value = result.expect("codeAction had neither result nor error");
                // Tolerate `null` for empty responses.
                if value.is_null() {
                    return Vec::new();
                }
                return serde_json::from_value::<Vec<CodeActionOrCommand>>(value)
                    .expect("codeAction response shape");
            }
            Ok(_) => {}
            Err(e) => panic!("recv failed while waiting for codeAction response: {e}"),
        }
    }
}

fn drain_publish_diagnostics_for(client_conn: &Connection, expected: &Url) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for publishDiagnostics for {expected}");
        }
        match client_conn.receiver.recv_timeout(remaining) {
            Ok(Message::Notification(note)) if note.method == PublishDiagnostics::METHOD => {
                let params: PublishDiagnosticsParams =
                    serde_json::from_value(note.params).expect("publishDiagnostics shape");
                if &params.uri == expected {
                    return;
                }
            }
            Ok(_) => {}
            Err(e) => panic!("recv failed while draining diagnostics: {e}"),
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
}

fn send_did_open(client_conn: &Connection, uri: &Url, text: &str) {
    let open_params = DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id: "tarn".to_owned(),
            version: 1,
            text: text.to_owned(),
        },
    };
    client_conn
        .sender
        .send(Message::Notification(Notification {
            method: DidOpenTextDocument::METHOD.to_owned(),
            params: serde_json::to_value(open_params).unwrap(),
        }))
        .unwrap();
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
