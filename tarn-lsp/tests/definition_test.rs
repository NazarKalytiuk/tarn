//! End-to-end integration tests for L2.1 go-to-definition.
//!
//! These drive the full `initialize → didOpen → textDocument/definition
//! → shutdown → exit` loop over an in-memory `lsp_server::Connection`,
//! the same harness L1.3/L1.4/L1.5 already use. The pure renderer and
//! context builder are unit-tested inside `src/definition.rs`; this
//! file confirms the dispatch wiring is correct and that the response
//! shape survives one full JSON round trip.

use std::thread;
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidOpenTextDocument, Exit, Initialized, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{GotoDefinition, Initialize, Request as _, Shutdown};
use lsp_types::{
    ClientCapabilities, DidOpenTextDocumentParams, GotoDefinitionParams, GotoDefinitionResponse,
    InitializeParams, InitializedParams, PartialResultParams, Position, PublishDiagnosticsParams,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Url,
    WorkDoneProgressParams,
};

/// Fixture — line numbers are 0-based so the cursor assertions stay
/// readable next to the source text.
///
///   0  name: def fixture
///   1  env:
///   2    base_url: http://localhost:3000
///   3  tests:
///   4    main:
///   5      steps:
///   6        - name: login
///   7          request:
///   8            method: POST
///   9            url: "{{ env.base_url }}/auth"
///  10          capture:
///  11            token: $.id
///  12        - name: next
///  13          request:
///  14            method: GET
///  15            url: "{{ env.base_url }}/items"
///  16            headers:
///  17              Authorization: "Bearer {{ capture.token }}"
const FIXTURE: &str = r#"name: def fixture
env:
  base_url: http://localhost:3000
tests:
  main:
    steps:
      - name: login
        request:
          method: POST
          url: "{{ env.base_url }}/auth"
        capture:
          token: $.id
      - name: next
        request:
          method: GET
          url: "{{ env.base_url }}/items"
          headers:
            Authorization: "Bearer {{ capture.token }}"
"#;

#[test]
fn definition_jumps_from_capture_interpolation_to_declaring_step() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/def-capture.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, FIXTURE);
    drain_publish_diagnostics_for(&client_conn, &uri);

    // Cursor inside `capture.token` in the `Bearer {{ capture.token }}`
    // header on line 17 (0-based). Column 37 lands on the `t` in
    // `token`.
    let response =
        request_definition(&client_conn, &uri, Position::new(17, 37)).expect("capture jump");
    match response {
        GotoDefinitionResponse::Scalar(location) => {
            // The declaring `token:` key is on line 11 (0-based).
            assert_eq!(location.range.start.line, 11);
            assert_eq!(location.uri, uri);
        }
        other => panic!("expected single scalar response, got {other:?}"),
    }

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn definition_jumps_from_env_interpolation_to_inline_env_block() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/def-env.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, FIXTURE);
    drain_publish_diagnostics_for(&client_conn, &uri);

    // Cursor inside the `base_url` interpolation on line 9 (0-based).
    // Column 20 lands between `base` and `_url`, well inside the token.
    let response = request_definition(&client_conn, &uri, Position::new(9, 20)).expect("env jump");
    match response {
        GotoDefinitionResponse::Scalar(location) => {
            // The inline `env:` block's `base_url:` key value scalar
            // lives on line 2 (0-based).
            assert_eq!(location.range.start.line, 2);
            assert_eq!(location.uri, uri);
        }
        other => panic!("expected single scalar response, got {other:?}"),
    }

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn definition_on_builtin_token_returns_null() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    // Minimal fixture: a single step that emits a UUID via the
    // `$uuid` builtin. We request a definition at the cursor inside
    // `$uuid` — the handler must reply with `null`, which the
    // transport surfaces as "no response result".
    let fixture = "\
name: builtin
steps:
  - name: ping
    request:
      method: GET
      url: \"http://localhost/{{ $uuid }}\"
";
    let uri = Url::parse("file:///tmp/def-builtin.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, fixture);
    drain_publish_diagnostics_for(&client_conn, &uri);

    // Cursor inside `$uuid` on line 5 (0-based). The `$` lives at
    // column 28 after `url: "http://localhost/{{ `.
    let response = request_definition(&client_conn, &uri, Position::new(5, 30));
    assert!(
        response.is_none(),
        "builtin token must produce empty response, got {response:?}"
    );

    shutdown_and_join(client_conn, server_thread);
}

// ---------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------

fn request_definition(
    client_conn: &Connection,
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let req_id: RequestId = 9201.into();
    let params = GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    client_conn
        .sender
        .send(Message::Request(Request {
            id: req_id.clone(),
            method: GotoDefinition::METHOD.to_owned(),
            params: serde_json::to_value(params).unwrap(),
        }))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for definition response");
        }
        match client_conn.receiver.recv_timeout(remaining) {
            Ok(Message::Response(Response { id, result, error })) if id == req_id => {
                assert!(error.is_none(), "definition returned error: {error:?}");
                let value = result.expect("definition had neither result nor error");
                // `null` deserialises to `Option<GotoDefinitionResponse>::None`
                // — the handler returns `None` for non-navigable tokens.
                return serde_json::from_value(value).expect("definition response shape");
            }
            Ok(_) => {}
            Err(e) => panic!("recv failed while waiting for definition response: {e}"),
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
    let shutdown_id: RequestId = 9901.into();
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
