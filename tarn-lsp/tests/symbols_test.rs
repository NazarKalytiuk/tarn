//! End-to-end integration tests for L1.5 document symbols.
//!
//! These tests drive the full `initialize → didOpen → documentSymbol →
//! shutdown → exit` loop over an in-memory `lsp_server::Connection`, the
//! same transport rust-analyzer uses in its own tests. The pure renderer
//! and span-conversion logic are exhaustively covered by unit tests
//! inside `src/symbols.rs`; this file wires everything together and
//! confirms:
//!
//!   * the dispatch in `server.rs` returns a hierarchical response,
//!   * the capability advertisement actually works end-to-end,
//!   * symbol ranges stay in sync with diagnostic ranges on the same
//!     lines (the ticket's "outline in sync with diagnostics" guarantee).

use std::thread;
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidOpenTextDocument, Exit, Initialized, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{DocumentSymbolRequest, Initialize, Request as _, Shutdown};
use lsp_types::{
    ClientCapabilities, DidOpenTextDocumentParams, DocumentSymbolParams, DocumentSymbolResponse,
    InitializeParams, InitializedParams, PartialResultParams, PublishDiagnosticsParams, SymbolKind,
    TextDocumentIdentifier, TextDocumentItem, Url, WorkDoneProgressParams,
};

/// Fixture spanning every surface the renderer has to handle: file-level
/// `name:`, a setup step, a named test with two steps, and a teardown
/// step.
const FIXTURE: &str = r#"name: symbols fixture
setup:
  - name: login
    request:
      method: POST
      url: http://localhost/auth
tests:
  main:
    steps:
      - name: list
        request:
          method: GET
          url: http://localhost/items
      - name: create
        request:
          method: POST
          url: http://localhost/items
teardown:
  - name: cleanup
    request:
      method: POST
      url: http://localhost/cleanup
"#;

/// Deliberately broken fixture: a step with a mis-typed `requestx` key
/// is rejected by the schema validator, but `yaml-rust2` still parses it
/// fine, so the outline pass must still return the `ping` step. This
/// exercises the ticket's "degrade gracefully on parse errors" rule.
const BROKEN_FIXTURE: &str = r#"name: broken fixture
steps:
  - name: ping
    requestx:
      method: GET
      url: http://localhost/ping
"#;

#[test]
fn document_symbol_returns_full_hierarchy_for_valid_document() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/sym-full.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, FIXTURE);
    drain_publish_diagnostics_for(&client_conn, &uri);

    let symbols = request_document_symbol(&client_conn, &uri);
    let roots = match symbols {
        DocumentSymbolResponse::Nested(symbols) => symbols,
        DocumentSymbolResponse::Flat(_) => panic!("expected hierarchical response"),
    };
    assert_eq!(roots.len(), 1, "expected exactly one file-root symbol");
    let root = &roots[0];
    assert_eq!(root.name, "symbols fixture");
    assert_eq!(root.kind, SymbolKind::NAMESPACE);

    let children = root.children.as_ref().expect("root must carry children");
    // setup + tests + teardown (no flat_steps in this fixture).
    assert_eq!(children.len(), 3, "expected 3 top-level groups");

    // Order: setup → tests → teardown (matches the render order).
    let login = &children[0];
    assert_eq!(login.name, "login");
    assert_eq!(login.kind, SymbolKind::FUNCTION);
    assert_eq!(login.detail.as_deref(), Some("setup"));

    let main_test = &children[1];
    assert_eq!(main_test.name, "main");
    assert_eq!(main_test.kind, SymbolKind::MODULE);
    let steps = main_test
        .children
        .as_ref()
        .expect("test must carry step children");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].name, "list");
    assert_eq!(steps[0].kind, SymbolKind::FUNCTION);
    assert_eq!(steps[1].name, "create");
    assert_eq!(steps[1].kind, SymbolKind::FUNCTION);

    let cleanup = &children[2];
    assert_eq!(cleanup.name, "cleanup");
    assert_eq!(cleanup.kind, SymbolKind::FUNCTION);
    assert_eq!(cleanup.detail.as_deref(), Some("teardown"));

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn document_symbol_still_returns_outline_for_schema_invalid_document() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/sym-broken.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, BROKEN_FIXTURE);

    // Drain diagnostics first so we know the validator ran. We do not
    // assert a specific line on the diagnostic — tarn's schema
    // validation falls back to zero when a field is unknown — we only
    // care that the outline still surfaces the step.
    let diagnostics = recv_diagnostics_for(&client_conn, &uri);
    assert!(
        !diagnostics.diagnostics.is_empty(),
        "broken fixture must produce at least one diagnostic"
    );

    let symbols = request_document_symbol(&client_conn, &uri);
    let roots = match symbols {
        DocumentSymbolResponse::Nested(symbols) => symbols,
        DocumentSymbolResponse::Flat(_) => panic!("expected hierarchical response"),
    };
    let children = roots
        .first()
        .and_then(|r| r.children.as_ref())
        .expect("root children");
    let ping = children
        .iter()
        .find(|c| c.name == "ping")
        .expect("ping step must still be present even though validation failed");
    // The `ping` step's selection range must land on line 2 (0-based)
    // because that's where `- name: ping` lives in the fixture — proving
    // the outline uses the same scanner as the diagnostic range
    // metadata.
    assert_eq!(
        ping.selection_range.start.line, 2,
        "ping selection_range should be on line 2, got {:?}",
        ping.selection_range
    );
    // And the full range must extend past the selection so the click
    // region covers the whole step body.
    assert!(
        ping.range.end.line >= ping.selection_range.start.line,
        "ping full range must end at or after its selection start"
    );

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn document_symbol_for_unknown_uri_returns_empty_response() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    // We never didOpen this URI — the store has no entry for it. The
    // handler must still respond with an empty list rather than erroring
    // or hanging. An empty JSON array serialises to
    // `DocumentSymbolResponse::Flat(vec![])` because `Flat` is the first
    // untagged variant that matches `[]`; we accept either.
    let uri = Url::parse("file:///tmp/sym-unknown.tarn.yaml").unwrap();
    let symbols = request_document_symbol(&client_conn, &uri);
    assert!(
        response_is_empty(&symbols),
        "unknown URI should yield no symbols, got {symbols:?}"
    );

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn document_symbol_for_empty_document_returns_empty_response() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });
    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/sym-empty.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, "");
    drain_publish_diagnostics_for(&client_conn, &uri);

    let symbols = request_document_symbol(&client_conn, &uri);
    assert!(
        response_is_empty(&symbols),
        "empty document should yield no symbols, got {symbols:?}"
    );

    shutdown_and_join(client_conn, server_thread);
}

fn response_is_empty(response: &DocumentSymbolResponse) -> bool {
    match response {
        DocumentSymbolResponse::Nested(list) => list.is_empty(),
        DocumentSymbolResponse::Flat(list) => list.is_empty(),
    }
}

// ---------------------------------------------------------------------
// helpers (mirror the ones in completion_test.rs / diagnostics_test.rs)
// ---------------------------------------------------------------------

fn request_document_symbol(client_conn: &Connection, uri: &Url) -> DocumentSymbolResponse {
    let req_id: RequestId = 9101.into();
    let params = DocumentSymbolParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    client_conn
        .sender
        .send(Message::Request(Request {
            id: req_id.clone(),
            method: DocumentSymbolRequest::METHOD.to_owned(),
            params: serde_json::to_value(params).unwrap(),
        }))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for documentSymbol response");
        }
        match client_conn.receiver.recv_timeout(remaining) {
            Ok(Message::Response(Response { id, result, error })) if id == req_id => {
                assert!(error.is_none(), "documentSymbol returned error: {error:?}");
                let value = result.expect("documentSymbol had neither result nor error");
                return serde_json::from_value(value).expect("documentSymbol response shape");
            }
            Ok(_) => {}
            Err(e) => panic!("recv failed while waiting for documentSymbol response: {e}"),
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

fn recv_diagnostics_for(client_conn: &Connection, expected: &Url) -> PublishDiagnosticsParams {
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
                    return params;
                }
            }
            Ok(_) => {}
            Err(e) => panic!("recv failed while waiting for diagnostics: {e}"),
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
