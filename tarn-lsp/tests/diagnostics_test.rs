//! End-to-end integration tests for L1.2 diagnostics.
//!
//! These tests drive `tarn-lsp` over an in-memory `lsp_server::Connection`,
//! the same transport rust-analyzer uses in its own tests. They are the
//! only place that exercises the full didOpen → publishDiagnostics chain:
//! the unit tests in `src/diagnostics.rs` and `src/debounce.rs` cover the
//! pure pieces, this file ties them together.

use std::thread;
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument, Exit,
    Initialized, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{Initialize, Request as _, Shutdown};
use lsp_types::{
    ClientCapabilities, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, InitializeParams, InitializedParams,
    NumberOrString, PublishDiagnosticsParams, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentItem, Url, VersionedTextDocumentIdentifier,
};

/// Valid minimal document — must not produce any diagnostics.
const VALID_DOC: &str = "name: smoke\nsteps:\n  - name: ping\n    request:\n      method: GET\n      url: http://example.com\n    assert:\n      status: 200\n";

/// Broken document with an unknown top-level field — produces one
/// `tarn_validation` diagnostic from `validate_yaml_shape`.
const BROKEN_UNKNOWN_FIELD: &str = "name: broken\nstep: []\n";

/// Raw-YAML syntax error: the quoted string is never closed.
const BROKEN_YAML: &str = "name: \"Broken\nsteps:\n  - name: x\n";

#[test]
fn did_open_publishes_diagnostics_for_broken_document() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });

    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/broken-unknown.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, BROKEN_UNKNOWN_FIELD);

    let params = recv_diagnostics_for(&client_conn, &uri);
    assert_eq!(
        params.diagnostics.len(),
        1,
        "expected exactly one diagnostic for broken doc, got {:?}",
        params.diagnostics
    );
    let d = &params.diagnostics[0];
    assert_eq!(d.source.as_deref(), Some("tarn"));
    assert_eq!(
        d.code,
        Some(NumberOrString::String("tarn_validation".to_owned()))
    );
    assert_eq!(
        d.severity,
        Some(lsp_types::DiagnosticSeverity::ERROR),
        "parse/validation errors must be ERROR severity"
    );

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn did_open_valid_document_publishes_empty_diagnostics() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });

    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/valid.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, VALID_DOC);

    // Valid documents must still publish — clients rely on the empty array
    // as a "no problems" acknowledgement.
    let params = recv_diagnostics_for(&client_conn, &uri);
    assert!(
        params.diagnostics.is_empty(),
        "valid document should publish an empty diagnostics array, got {:?}",
        params.diagnostics
    );

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn did_open_yaml_syntax_error_carries_location_and_source() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });

    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/yaml-syntax.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, BROKEN_YAML);

    let params = recv_diagnostics_for(&client_conn, &uri);
    assert_eq!(params.diagnostics.len(), 1);
    let d = &params.diagnostics[0];
    assert_eq!(
        d.code,
        Some(NumberOrString::String("yaml_syntax".to_owned()))
    );
    assert_eq!(d.source.as_deref(), Some("tarn"));
    // serde_yaml always reports a location for unclosed quotes, so the
    // 0-based range must also be non-default.
    assert!(
        d.range.start.line > 0 || d.range.start.character > 0,
        "yaml syntax error should have a non-default range, got {:?}",
        d.range
    );

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn did_save_triggers_a_fresh_publish() {
    // After didOpen with a broken doc and then didSave, the client must
    // receive two publishDiagnostics notifications — one per trigger.
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });

    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/save.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, BROKEN_UNKNOWN_FIELD);
    let first = recv_diagnostics_for(&client_conn, &uri);
    assert_eq!(first.diagnostics.len(), 1);

    let save_params = DidSaveTextDocumentParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        text: None,
    };
    client_conn
        .sender
        .send(Message::Notification(Notification {
            method: DidSaveTextDocument::METHOD.to_owned(),
            params: serde_json::to_value(save_params).unwrap(),
        }))
        .unwrap();

    let second = recv_diagnostics_for(&client_conn, &uri);
    assert_eq!(second.diagnostics.len(), 1);

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn did_close_publishes_empty_diagnostics_for_closed_uri() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });

    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/close.tarn.yaml").unwrap();
    send_did_open(&client_conn, &uri, BROKEN_UNKNOWN_FIELD);
    let first = recv_diagnostics_for(&client_conn, &uri);
    assert_eq!(first.diagnostics.len(), 1, "expected broken doc to publish");

    // Close it. The server must follow up with an empty diagnostic array.
    let close_params = DidCloseTextDocumentParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
    };
    client_conn
        .sender
        .send(Message::Notification(Notification {
            method: DidCloseTextDocument::METHOD.to_owned(),
            params: serde_json::to_value(close_params).unwrap(),
        }))
        .unwrap();

    let close_publish = recv_diagnostics_for(&client_conn, &uri);
    assert!(
        close_publish.diagnostics.is_empty(),
        "didClose must clear diagnostics, got {:?}",
        close_publish.diagnostics
    );

    shutdown_and_join(client_conn, server_thread);
}

#[test]
fn rapid_did_change_debounces_to_single_publish() {
    // Three didChange notifications on the same URI within the debounce
    // window must collapse into exactly one publishDiagnostics for the
    // final content. This test is the only end-to-end check of the
    // debounce loop — the pure helper is covered separately in
    // `src/debounce.rs`.
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });

    handshake(&client_conn);

    let uri = Url::parse("file:///tmp/debounce.tarn.yaml").unwrap();

    // Open with a valid doc → single empty publish.
    send_did_open(&client_conn, &uri, VALID_DOC);
    let open_publish = recv_diagnostics_for(&client_conn, &uri);
    assert!(open_publish.diagnostics.is_empty());

    // Fire three rapid changes. The first two are still valid docs, the
    // final one introduces a validation error. The debounce must collapse
    // them and publish exactly one notification for the final content.
    for (i, content) in [VALID_DOC, VALID_DOC, BROKEN_UNKNOWN_FIELD]
        .iter()
        .enumerate()
    {
        let change_params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: (i + 2) as i32,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: (*content).to_owned(),
            }],
        };
        client_conn
            .sender
            .send(Message::Notification(Notification {
                method: DidChangeTextDocument::METHOD.to_owned(),
                params: serde_json::to_value(change_params).unwrap(),
            }))
            .unwrap();
    }

    // Wait up to 2 seconds for a debounced publish. If the server fires
    // more than once we catch it by draining any extra notifications in
    // a tight follow-up loop.
    let publish = recv_diagnostics_for(&client_conn, &uri);
    assert_eq!(
        publish.diagnostics.len(),
        1,
        "debounced publish should reflect the final BROKEN content"
    );
    assert_eq!(publish.diagnostics[0].source.as_deref(), Some("tarn"));

    // Drain any stray publishes that arrive within a short grace window.
    let deadline = Instant::now() + Duration::from_millis(150);
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match client_conn.receiver.recv_timeout(remaining) {
            Ok(Message::Notification(note)) if note.method == PublishDiagnostics::METHOD => {
                let extra: PublishDiagnosticsParams = serde_json::from_value(note.params).unwrap();
                if extra.uri == uri {
                    panic!(
                        "debounce leaked: got a second publishDiagnostics for {}",
                        uri
                    );
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    shutdown_and_join(client_conn, server_thread);
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Drive the `initialize` + `initialized` handshake over the given connection.
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

    // Wait for the initialize response. Ignore any notifications that
    // somehow arrive first — none should, but be defensive.
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

/// Send a full `didOpen` for the given URI + source text.
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

/// Block until we receive a `publishDiagnostics` notification for the
/// expected URI. Any other messages (responses, unrelated notifications)
/// are discarded. Times out after 2 seconds with a clear panic message.
fn recv_diagnostics_for(client_conn: &Connection, expected: &Url) -> PublishDiagnosticsParams {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for publishDiagnostics for {}", expected);
        }
        let msg = client_conn
            .receiver
            .recv_timeout(remaining)
            .unwrap_or_else(|e| panic!("recv failed while waiting for diagnostics: {e}"));
        if let Message::Notification(note) = msg {
            if note.method == PublishDiagnostics::METHOD {
                let params: PublishDiagnosticsParams =
                    serde_json::from_value(note.params).expect("malformed publishDiagnostics");
                if &params.uri == expected {
                    return params;
                }
            }
        }
    }
}

/// Drive `shutdown` + `exit` and join the server thread.
fn shutdown_and_join(client_conn: Connection, server_thread: thread::JoinHandle<()>) {
    let shutdown_id: RequestId = 9001.into();
    client_conn
        .sender
        .send(Message::Request(Request {
            id: shutdown_id.clone(),
            method: Shutdown::METHOD.to_owned(),
            params: serde_json::Value::Null,
        }))
        .unwrap();

    // Drain until we see the shutdown response. Ignore stray notifications
    // so the shutdown handshake completes deterministically.
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
