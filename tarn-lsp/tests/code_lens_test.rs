//! End-to-end integration tests for L2.4 code lens (NAZ-300).
//!
//! These tests drive the full `initialize → didOpen → codeLens →
//! shutdown → exit` loop over an in-memory `lsp_server::Connection`,
//! the same transport the other Phase L handler tests use. The pure
//! renderer and selector-composition logic live in unit tests inside
//! `src/code_lens.rs`; this file only wires everything together and
//! confirms that:
//!
//!   * the dispatch in `server.rs` hands the request to the renderer
//!     and sends back a `Vec<CodeLens>` the client can deserialize,
//!   * the capability advertisement actually works end-to-end (the
//!     `initialize` handshake test elsewhere covers the capability
//!     shape; we only exercise the runtime path here),
//!   * a `.tarn.yaml` buffer with two tests and nested steps returns
//!     the expected lens count with stable command IDs and argument
//!     shapes,
//!   * non-tarn URIs (and URIs the server has not seen) return an
//!     empty array rather than a JSON-RPC error — the client contract
//!     that "unsupported files get silently empty lenses".

use std::thread;
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidOpenTextDocument, Exit, Initialized, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{CodeLensRequest, Initialize, Request as _, Shutdown};
use lsp_types::{
    ClientCapabilities, CodeLens, CodeLensParams, DidOpenTextDocumentParams, InitializeParams,
    InitializedParams, PartialResultParams, PublishDiagnosticsParams, TextDocumentIdentifier,
    TextDocumentItem, Url, WorkDoneProgressParams,
};

/// Fixture with two named tests. The first has one step, the second
/// has two steps. Setup and flat-step blocks are intentionally absent
/// — the handler only emits lenses for named-test groups so nothing
/// here should create stray lenses.
const FIXTURE: &str = r#"name: code lens fixture
tests:
  first:
    steps:
      - name: list
        request:
          method: GET
          url: http://localhost/items
  second:
    steps:
      - name: create
        request:
          method: POST
          url: http://localhost/items
      - name: delete
        request:
          method: DELETE
          url: http://localhost/items/1
"#;

#[test]
fn code_lens_returns_run_test_and_run_step_lenses_for_every_named_test() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/cl-full.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, FIXTURE);
    drain_publish_diagnostics_for(&client_conn, &uri);

    let lenses = request_code_lens(&client_conn, &uri);

    // 2 test lenses + 1 step + 2 steps = 5.
    assert_eq!(
        lenses.len(),
        5,
        "expected two test lenses plus three step lenses, got {lenses:#?}"
    );

    // Collect titles + command ids for an order-independent assertion.
    let mut run_test_count = 0;
    let mut run_step_count = 0;
    for lens in &lenses {
        let cmd = lens
            .command
            .as_ref()
            .expect("every lens must carry a command");
        match cmd.command.as_str() {
            "tarn.runTest" => {
                assert_eq!(cmd.title, "Run test");
                run_test_count += 1;
            }
            "tarn.runStep" => {
                assert_eq!(cmd.title, "Run step");
                run_step_count += 1;
            }
            other => panic!("unexpected command id: {other}"),
        }
        let args = cmd
            .arguments
            .as_ref()
            .expect("every lens must carry arguments");
        assert_eq!(args.len(), 1, "each lens carries a single JSON argument");
        let obj = args[0].as_object().expect("argument must be a JSON object");
        let file = obj
            .get("file")
            .and_then(|v| v.as_str())
            .expect("lens argument must carry a `file` string");
        assert_eq!(file, uri.as_str());
        let selector = obj
            .get("selector")
            .and_then(|v| v.as_str())
            .expect("lens argument must carry a `selector` string");
        assert!(
            selector.starts_with("/tmp/cl-full.tarn.yaml::"),
            "selector must start with the file path and ::, got {selector}"
        );
    }
    assert_eq!(run_test_count, 2, "expected two Run test lenses");
    assert_eq!(run_step_count, 3, "expected three Run step lenses");

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn code_lens_for_non_tarn_uri_returns_empty_array() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    // The handler intentionally short-circuits on URIs that are not
    // `.tarn.yaml` / `.tarn.yml`. It also returns an empty array for
    // unknown URIs the server has never seen — both paths hit the
    // same `Vec::new()` return, and both are part of the contract.
    let uri = Url::parse("file:///tmp/not-tarn.txt").unwrap();
    let lenses = request_code_lens(&client_conn, &uri);
    assert!(
        lenses.is_empty(),
        "non-tarn URI must yield an empty lens list, got {lenses:#?}"
    );

    shutdown_and_join(client_conn, server_thread);
}

// ---------------------------------------------------------------------
// helpers (mirror the ones in symbols_test.rs / references_test.rs)
// ---------------------------------------------------------------------

fn request_code_lens(client_conn: &Connection, uri: &Url) -> Vec<CodeLens> {
    let req_id: RequestId = 9301.into();
    let params = CodeLensParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    client_conn
        .sender
        .send(Message::Request(Request {
            id: req_id.clone(),
            method: CodeLensRequest::METHOD.to_owned(),
            params: serde_json::to_value(params).unwrap(),
        }))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for codeLens response");
        }
        match client_conn.receiver.recv_timeout(remaining) {
            Ok(Message::Response(Response { id, result, error })) if id == req_id => {
                assert!(error.is_none(), "codeLens returned error: {error:?}");
                let value = result.expect("codeLens had neither result nor error");
                return serde_json::from_value(value).expect("codeLens response shape");
            }
            Ok(_) => {}
            Err(e) => panic!("recv failed while waiting for codeLens response: {e}"),
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
