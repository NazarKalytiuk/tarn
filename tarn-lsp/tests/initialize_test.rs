//! Integration tests for the `tarn-lsp` LSP lifecycle.
//!
//! These tests drive the server over an in-memory `lsp_server::Connection`.
//! rust-analyzer uses the same pattern — it's deterministic, does not touch
//! the filesystem, and avoids spawning a subprocess.

use std::thread;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument, Exit,
    Initialized, Notification as _,
};
use lsp_types::request::{Initialize, Request as _, Shutdown};
use lsp_types::{
    ClientCapabilities, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, InitializeParams, InitializedParams,
    ServerCapabilities, TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentSyncCapability, TextDocumentSyncKind, Url, VersionedTextDocumentIdentifier,
};

/// Sanity check: `server_capabilities()` is the single source of truth
/// for what the current phase advertises. Phase L1 is now complete and
/// Phase L2 has started shipping — every feature capability L1.1
/// through L1.5 must be present, plus any L2 ticket that has landed.
#[test]
fn server_capabilities_advertises_every_phase_l1_feature() {
    let caps = tarn_lsp::server_capabilities();

    assert_eq!(
        caps.text_document_sync,
        Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        "phase L1.1 must advertise Full text document sync"
    );

    // L1.3 (NAZ-292): hover is on.
    assert_eq!(
        caps.hover_provider,
        Some(lsp_types::HoverProviderCapability::Simple(true)),
        "L1.3 must advertise hover provider as Simple(true)"
    );

    // L1.4 (NAZ-293): completion is on with trigger characters `.` and `$`.
    let completion = caps
        .completion_provider
        .as_ref()
        .expect("L1.4 must advertise completion provider");
    assert_eq!(
        completion.trigger_characters.as_deref(),
        Some(&[".".to_owned(), "$".to_owned()][..]),
        "L1.4 must advertise `.` and `$` as completion trigger characters"
    );
    assert_eq!(
        completion.resolve_provider,
        Some(false),
        "L1.4 does not implement completionItem/resolve"
    );

    // L1.5 (NAZ-294): documentSymbol is on. We accept either variant of
    // `OneOf<bool, DocumentSymbolOptions>` since both advertise the same
    // server-side capability in Phase L1.
    assert_eq!(
        caps.document_symbol_provider,
        Some(lsp_types::OneOf::Left(true)),
        "L1.5 must advertise document_symbol_provider as OneOf::Left(true)"
    );

    // L2.1 (NAZ-297): go-to-definition is on.
    assert_eq!(
        caps.definition_provider,
        Some(lsp_types::OneOf::Left(true)),
        "L2.1 must advertise definition_provider as OneOf::Left(true)"
    );

    // Phase L2/L3 capabilities that have not shipped yet remain unset.
    assert!(caps.references_provider.is_none(), "L2: find references");
    assert!(caps.rename_provider.is_none(), "L2: rename symbol");
    assert!(caps.code_action_provider.is_none(), "L3: code actions");
    assert!(
        caps.execute_command_provider.is_none(),
        "L3: execute command"
    );
}

/// End-to-end handshake test over an in-memory transport.
///
/// This drives the full LSP lifecycle the ticket requires:
///   initialize → initialized → didOpen → didChange → didSave → didClose
///   → shutdown → exit
#[test]
fn full_lifecycle_over_memory_transport() {
    let (server_conn, client_conn) = Connection::memory();

    // Spawn the server loop on a worker thread so this test can act as the
    // client on the main thread. `run_with_connection` runs until the
    // receiver end of the connection is closed (which happens when the
    // client drops its sender on `exit`).
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });

    // ---- initialize ----
    let initialize_params = InitializeParams {
        capabilities: ClientCapabilities::default(),
        ..Default::default()
    };

    let init_id: RequestId = 1.into();
    client_conn
        .sender
        .send(Message::Request(Request {
            id: init_id.clone(),
            method: Initialize::METHOD.to_owned(),
            params: serde_json::to_value(initialize_params).unwrap(),
        }))
        .unwrap();

    let init_resp = recv_response(&client_conn, &init_id);
    assert!(
        init_resp.error.is_none(),
        "initialize returned error: {:?}",
        init_resp.error
    );

    let result = init_resp.result.expect("initialize must return a result");

    // serverInfo must report name + version for client log panes.
    let server_info = result
        .get("serverInfo")
        .expect("initialize result must include serverInfo");
    assert_eq!(server_info["name"], "tarn-lsp");
    assert_eq!(server_info["version"], env!("CARGO_PKG_VERSION"));

    // Capabilities must round-trip cleanly and declare Full text sync
    // plus the L1.3 hover provider.
    let caps_json = result
        .get("capabilities")
        .expect("initialize result must include capabilities");
    let caps: ServerCapabilities =
        serde_json::from_value(caps_json.clone()).expect("server capabilities must deserialize");
    assert_eq!(
        caps.text_document_sync,
        Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
    );
    assert_eq!(
        caps.hover_provider,
        Some(lsp_types::HoverProviderCapability::Simple(true))
    );
    // L1.4 wires completion with trigger characters `.` and `$`.
    let completion = caps
        .completion_provider
        .as_ref()
        .expect("L1.4 must advertise completion provider");
    assert_eq!(
        completion.trigger_characters.as_deref(),
        Some(&[".".to_owned(), "$".to_owned()][..])
    );

    // ---- initialized ----
    client_conn
        .sender
        .send(Message::Notification(Notification {
            method: Initialized::METHOD.to_owned(),
            params: serde_json::to_value(InitializedParams {}).unwrap(),
        }))
        .unwrap();

    // ---- didOpen ----
    let uri = Url::parse("file:///tmp/smoke.tarn.yaml").unwrap();
    let open_params = DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id: "tarn".to_owned(),
            version: 1,
            text: "name: smoke\nsteps: []\n".to_owned(),
        },
    };
    client_conn
        .sender
        .send(Message::Notification(Notification {
            method: DidOpenTextDocument::METHOD.to_owned(),
            params: serde_json::to_value(open_params).unwrap(),
        }))
        .unwrap();

    // ---- didChange (full sync) ----
    let change_params = DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "name: smoke\nsteps:\n  - name: ping\n".to_owned(),
        }],
    };
    client_conn
        .sender
        .send(Message::Notification(Notification {
            method: DidChangeTextDocument::METHOD.to_owned(),
            params: serde_json::to_value(change_params).unwrap(),
        }))
        .unwrap();

    // ---- didSave (no-op in L1.1 but must be accepted) ----
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

    // ---- didClose ----
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

    // ---- shutdown ----
    let shutdown_id: RequestId = 2.into();
    client_conn
        .sender
        .send(Message::Request(Request {
            id: shutdown_id.clone(),
            method: Shutdown::METHOD.to_owned(),
            params: serde_json::Value::Null,
        }))
        .unwrap();

    let shutdown_resp = recv_response(&client_conn, &shutdown_id);
    assert!(
        shutdown_resp.error.is_none(),
        "shutdown returned error: {:?}",
        shutdown_resp.error
    );

    // ---- exit ----
    client_conn
        .sender
        .send(Message::Notification(Notification {
            method: Exit::METHOD.to_owned(),
            params: serde_json::Value::Null,
        }))
        .unwrap();

    // Drop the client's sender so the server's receiver loop terminates
    // cleanly. `Connection::memory()` wires the two halves together, so the
    // server only sees EOF once every client-side sender is dropped.
    drop(client_conn);

    server_thread.join().expect("server thread panicked");
}

/// Unknown request types should come back as `MethodNotFound` rather than
/// crashing the server. This guards against later tickets breaking the
/// method-not-found fallthrough.
#[test]
fn unknown_request_returns_method_not_found() {
    let (server_conn, client_conn) = Connection::memory();

    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });

    // Drive initialize first so the server leaves the handshake phase.
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
    let _ = recv_response(&client_conn, &init_id);

    client_conn
        .sender
        .send(Message::Notification(Notification {
            method: Initialized::METHOD.to_owned(),
            params: serde_json::to_value(InitializedParams {}).unwrap(),
        }))
        .unwrap();

    // Send a bogus request that no L1 ticket will ever implement. We can
    // no longer use `textDocument/hover` here because NAZ-292 (L1.3)
    // handles it; pick an obviously-unsupported method instead.
    let bogus_id: RequestId = 42.into();
    client_conn
        .sender
        .send(Message::Request(Request {
            id: bogus_id.clone(),
            method: "workspace/definitelyNotARealMethod".to_owned(),
            params: serde_json::Value::Null,
        }))
        .unwrap();

    let resp = recv_response(&client_conn, &bogus_id);
    let err = resp
        .error
        .expect("unsupported request must return an error");
    assert_eq!(err.code, lsp_server::ErrorCode::MethodNotFound as i32);

    // Shutdown + exit so the server thread exits cleanly.
    let shutdown_id: RequestId = 99.into();
    client_conn
        .sender
        .send(Message::Request(Request {
            id: shutdown_id.clone(),
            method: Shutdown::METHOD.to_owned(),
            params: serde_json::Value::Null,
        }))
        .unwrap();
    let _ = recv_response(&client_conn, &shutdown_id);
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

/// Block until a response with the expected id arrives on `conn`. Any other
/// messages received in the meantime (notifications, out-of-band responses)
/// are ignored, which mirrors how a real LSP client would dispatch.
fn recv_response(conn: &Connection, expected: &RequestId) -> Response {
    loop {
        let msg = conn
            .receiver
            .recv()
            .expect("connection closed before response arrived");
        if let Message::Response(resp) = msg {
            if &resp.id == expected {
                return resp;
            }
        }
    }
}
