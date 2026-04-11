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

/// Sanity check: `server_capabilities()` is the single source of truth for
/// what L1.1 advertises, and it must advertise exactly `Full` text sync and
/// nothing else.
#[test]
fn server_capabilities_advertises_only_full_text_sync() {
    let caps = tarn_lsp::server_capabilities();

    assert_eq!(
        caps.text_document_sync,
        Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        "phase L1.1 must advertise Full text document sync"
    );

    // Every feature capability must be unset. These flip on in later L1
    // tickets — if one of them is already set, capabilities.rs has drifted
    // from the roadmap and needs a compensating update to the doc + tests.
    assert!(caps.hover_provider.is_none(), "hover is NAZ-292, not L1.1");
    assert!(caps.completion_provider.is_none(), "completion is NAZ-293");
    assert!(
        caps.document_symbol_provider.is_none(),
        "symbols are NAZ-294"
    );
    assert!(caps.definition_provider.is_none());
    assert!(caps.references_provider.is_none());
    assert!(caps.rename_provider.is_none());
    assert!(caps.code_action_provider.is_none());
    assert!(caps.execute_command_provider.is_none());
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

    // Capabilities must round-trip cleanly and declare Full text sync.
    let caps_json = result
        .get("capabilities")
        .expect("initialize result must include capabilities");
    let caps: ServerCapabilities =
        serde_json::from_value(caps_json.clone()).expect("server capabilities must deserialize");
    assert_eq!(
        caps.text_document_sync,
        Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
    );
    assert!(caps.hover_provider.is_none());
    assert!(caps.completion_provider.is_none());

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

    // Send a bogus request. `textDocument/hover` is chosen deliberately: it
    // is unhandled today, but will become handled in NAZ-292 (L1.3). When
    // that ticket lands the test can be updated alongside it.
    let hover_id: RequestId = 42.into();
    client_conn
        .sender
        .send(Message::Request(Request {
            id: hover_id.clone(),
            method: "textDocument/hover".to_owned(),
            params: serde_json::Value::Null,
        }))
        .unwrap();

    let resp = recv_response(&client_conn, &hover_id);
    let err = resp.error.expect("hover must return an error in L1.1");
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
