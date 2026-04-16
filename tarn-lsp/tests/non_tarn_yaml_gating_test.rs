//! Regression coverage for the non-Tarn-YAML file gate.
//!
//! Claude Code's `.lsp.json` plugin format registers language servers by
//! bare extension (`.yaml`, `.yml`) and has no glob or compound-extension
//! support, so tarn-lsp is attached to every YAML buffer — Kubernetes
//! manifests, Compose files, CI configs — in any workspace where the
//! plugin is installed. Every request handler short-circuits through
//! `server::is_tarn_file_uri` and returns its LSP-appropriate empty
//! result for non-`*.tarn.yaml` buffers so we never emit bogus
//! diagnostics, hovers, completions, or lenses on foreign YAML.
//!
//! These tests drive the full LSP lifecycle over an in-memory
//! `lsp_server::Connection` and assert that for a bare `.yaml` URI:
//!
//!   * `didOpen` + `didSave` publish **no** diagnostics at all (not even
//!     an empty array — we never claim ownership of the URI).
//!   * `textDocument/hover`, `completion`, `definition`, `references`,
//!     `prepareRename`, `rename`, `codeLens`, `formatting`, `codeAction`,
//!     and `documentSymbol` all return their LSP-appropriate empty
//!     result.
//!
//! A matching positive test opens the same content under a
//! `*.tarn.yaml` URI to confirm the gate is URI-specific and the real
//! handlers still fire for Tarn buffers.

use std::thread;
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidOpenTextDocument, DidSaveTextDocument, Exit, Initialized, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, CodeLensRequest, Completion, DocumentSymbolRequest, Formatting,
    GotoDefinition, HoverRequest, Initialize, PrepareRenameRequest, References, Rename,
    Request as _, Shutdown,
};
use lsp_types::{
    ClientCapabilities, CodeActionContext, CodeActionParams, CodeActionResponse, CodeLens,
    CodeLensParams, CompletionParams, CompletionResponse, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentFormattingParams, DocumentSymbolParams,
    DocumentSymbolResponse, FormattingOptions, GotoDefinitionParams, GotoDefinitionResponse, Hover,
    HoverParams, InitializeParams, InitializedParams, Location as LspLocation, PartialResultParams,
    Position, PrepareRenameResponse, PublishDiagnosticsParams, Range, ReferenceContext,
    ReferenceParams, RenameParams, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, TextEdit, Url, WorkDoneProgressParams, WorkspaceEdit,
};

/// Minimal valid Tarn source with a named test group so the positive
/// control can observe `textDocument/codeLens` output as well as the
/// cheaper document-symbol handler — the code-lens renderer only emits
/// lenses for named tests, not top-level `steps:`.
const TARN_SOURCE: &str = "\
name: gated
env:
  base_url: http://example.com
tests:
  smoke:
    steps:
      - name: ping
        request:
          method: GET
          url: \"{{ env.base_url }}/ping\"
        capture:
          token: header(X-Token)
        assert:
          status: 200
      - name: follow
        request:
          method: GET
          url: \"{{ env.base_url }}/follow\"
        assert:
          status: 200
";

#[test]
fn bare_yaml_uri_silences_every_handler_but_tarn_yaml_still_works() {
    let (server_conn, client_conn) = Connection::memory();
    let server_thread = thread::spawn(move || {
        tarn_lsp::run_with_connection(server_conn).expect("server loop failed");
    });

    handshake(&client_conn);

    let bare = Url::parse("file:///tmp/k8s-deploy.yaml").unwrap();
    let tarn = Url::parse("file:///tmp/smoke.tarn.yaml").unwrap();

    send_did_open(&client_conn, &bare, TARN_SOURCE);
    // didSave gives the server another chance to publish. Ignored too.
    send_did_save(&client_conn, &bare);
    send_did_open(&client_conn, &tarn, TARN_SOURCE);

    // The Tarn URI must still publish (empty array, since the source
    // is valid). The bare URI must NOT publish at all — any publish
    // for it is a gate regression.
    let tarn_publish = recv_diagnostics_for(&client_conn, &tarn, Duration::from_secs(2));
    assert!(
        tarn_publish.diagnostics.is_empty(),
        "valid *.tarn.yaml buffer should publish empty diagnostics, got {:?}",
        tarn_publish.diagnostics,
    );
    assert_no_publish_for(&client_conn, &bare, Duration::from_millis(150));

    // Anchor position sits inside `{{ env.base_url }}` on the `ping`
    // step so hover/definition/references all have something real to
    // return when the gate is bypassed.
    let interp = Position::new(9, 25);

    assert!(
        request_hover(&client_conn, &bare, interp).is_none(),
        "hover on bare .yaml must return null",
    );
    assert!(
        request_completion(&client_conn, &bare, interp).is_none(),
        "completion on bare .yaml must return null",
    );
    match request_document_symbols(&client_conn, &bare) {
        DocumentSymbolResponse::Nested(symbols) => assert!(
            symbols.is_empty(),
            "documentSymbol on bare .yaml must be empty, got {symbols:?}",
        ),
        DocumentSymbolResponse::Flat(items) => assert!(
            items.is_empty(),
            "documentSymbol on bare .yaml must be empty, got {items:?}",
        ),
    }
    assert!(
        request_definition(&client_conn, &bare, interp).is_none(),
        "definition on bare .yaml must return null",
    );
    assert!(
        request_references(&client_conn, &bare, interp).is_empty(),
        "references on bare .yaml must return an empty array",
    );
    assert!(
        request_prepare_rename(&client_conn, &bare, interp).is_none(),
        "prepareRename on bare .yaml must return null",
    );
    let rename_edit = request_rename(&client_conn, &bare, interp, "new_name");
    assert!(
        rename_edit.changes.is_none() && rename_edit.document_changes.is_none(),
        "rename on bare .yaml must return an empty WorkspaceEdit, got {rename_edit:?}",
    );
    assert!(
        request_code_lens(&client_conn, &bare).is_empty(),
        "codeLens on bare .yaml must be empty",
    );
    assert!(
        request_formatting(&client_conn, &bare).is_empty(),
        "formatting on bare .yaml must be empty",
    );
    assert!(
        request_code_action(&client_conn, &bare).is_empty(),
        "codeAction on bare .yaml must be empty",
    );

    // Positive control: the same requests against the `.tarn.yaml`
    // sibling must still produce real work. Two document symbols is a
    // cheap proof that the handler actually ran (file-root + ping test
    // would both populate the outline).
    match request_document_symbols(&client_conn, &tarn) {
        DocumentSymbolResponse::Nested(symbols) => assert!(
            !symbols.is_empty(),
            "documentSymbol on *.tarn.yaml must be non-empty — gate should not fire here",
        ),
        DocumentSymbolResponse::Flat(items) => assert!(
            !items.is_empty(),
            "documentSymbol on *.tarn.yaml must be non-empty",
        ),
    }
    assert!(
        !request_code_lens(&client_conn, &tarn).is_empty(),
        "codeLens on *.tarn.yaml must produce run-test/run-step lenses",
    );

    shutdown_and_join(client_conn, server_thread);
}

// ---------------------------------------------------------------------------
// request helpers
// ---------------------------------------------------------------------------

fn request_hover(client: &Connection, uri: &Url, position: Position) -> Option<Hover> {
    let params = HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    round_trip(client, HoverRequest::METHOD, params)
}

fn request_completion(
    client: &Connection,
    uri: &Url,
    position: Position,
) -> Option<CompletionResponse> {
    let params = CompletionParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: None,
    };
    round_trip(client, Completion::METHOD, params)
}

fn request_document_symbols(client: &Connection, uri: &Url) -> DocumentSymbolResponse {
    let params = DocumentSymbolParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    round_trip::<_, Option<DocumentSymbolResponse>>(client, DocumentSymbolRequest::METHOD, params)
        .unwrap_or(DocumentSymbolResponse::Nested(Vec::new()))
}

fn request_definition(
    client: &Connection,
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let params = GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    round_trip(client, GotoDefinition::METHOD, params)
}

fn request_references(client: &Connection, uri: &Url, position: Position) -> Vec<LspLocation> {
    let params = ReferenceParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext {
            include_declaration: true,
        },
    };
    round_trip(client, References::METHOD, params)
}

fn request_prepare_rename(
    client: &Connection,
    uri: &Url,
    position: Position,
) -> Option<PrepareRenameResponse> {
    let params = TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        position,
    };
    round_trip(client, PrepareRenameRequest::METHOD, params)
}

fn request_rename(
    client: &Connection,
    uri: &Url,
    position: Position,
    new_name: &str,
) -> WorkspaceEdit {
    let params = RenameParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position,
        },
        new_name: new_name.to_owned(),
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    round_trip(client, Rename::METHOD, params)
}

fn request_code_lens(client: &Connection, uri: &Url) -> Vec<CodeLens> {
    let params = CodeLensParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    round_trip(client, CodeLensRequest::METHOD, params)
}

fn request_formatting(client: &Connection, uri: &Url) -> Vec<TextEdit> {
    let params = DocumentFormattingParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        options: FormattingOptions {
            tab_size: 2,
            insert_spaces: true,
            ..Default::default()
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    round_trip(client, Formatting::METHOD, params)
}

fn request_code_action(client: &Connection, uri: &Url) -> CodeActionResponse {
    let params = CodeActionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        range: Range::new(Position::new(0, 0), Position::new(0, 0)),
        context: CodeActionContext {
            diagnostics: Vec::new(),
            only: None,
            trigger_kind: None,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    round_trip(client, CodeActionRequest::METHOD, params)
}

// ---------------------------------------------------------------------------
// lifecycle + transport helpers
// ---------------------------------------------------------------------------

fn round_trip<P: serde::Serialize, R: for<'de> serde::Deserialize<'de>>(
    client: &Connection,
    method: &str,
    params: P,
) -> R {
    static NEXT_ID: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(100);
    let id: RequestId = NEXT_ID
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        .into();
    client
        .sender
        .send(Message::Request(Request {
            id: id.clone(),
            method: method.to_owned(),
            params: serde_json::to_value(params).expect("serialize request params"),
        }))
        .expect("send request");
    let resp = recv_response_for(client, &id);
    assert!(
        resp.error.is_none(),
        "{method} returned error: {:?}",
        resp.error,
    );
    let value = resp.result.unwrap_or(serde_json::Value::Null);
    serde_json::from_value(value)
        .unwrap_or_else(|err| panic!("{method} response did not match expected shape: {err}"))
}

fn recv_response_for(client: &Connection, id: &RequestId) -> Response {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for response id {id:?}");
        }
        match client.receiver.recv_timeout(remaining) {
            Ok(Message::Response(resp)) if resp.id == *id => return resp,
            Ok(_) => continue,
            Err(err) => panic!("recv failed while waiting for response: {err}"),
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

fn send_did_open(client: &Connection, uri: &Url, text: &str) {
    let params = DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id: "yaml".to_owned(),
            version: 1,
            text: text.to_owned(),
        },
    };
    client
        .sender
        .send(Message::Notification(Notification {
            method: DidOpenTextDocument::METHOD.to_owned(),
            params: serde_json::to_value(params).unwrap(),
        }))
        .unwrap();
}

fn send_did_save(client: &Connection, uri: &Url) {
    let params = DidSaveTextDocumentParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        text: None,
    };
    client
        .sender
        .send(Message::Notification(Notification {
            method: DidSaveTextDocument::METHOD.to_owned(),
            params: serde_json::to_value(params).unwrap(),
        }))
        .unwrap();
}

fn recv_diagnostics_for(
    client: &Connection,
    expected: &Url,
    timeout: Duration,
) -> PublishDiagnosticsParams {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for publishDiagnostics for {}", expected);
        }
        let msg = client
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

/// Fail loudly if any `publishDiagnostics` for `uri` arrives within
/// `window`. Unrelated diagnostics (for other URIs) are tolerated.
fn assert_no_publish_for(client: &Connection, uri: &Url, window: Duration) {
    let deadline = Instant::now() + window;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match client.receiver.recv_timeout(remaining) {
            Ok(Message::Notification(note)) if note.method == PublishDiagnostics::METHOD => {
                let params: PublishDiagnosticsParams =
                    serde_json::from_value(note.params).expect("malformed publishDiagnostics");
                if &params.uri == uri {
                    panic!(
                        "gate regression: publishDiagnostics arrived for non-Tarn URI {} with {:?}",
                        uri, params.diagnostics,
                    );
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

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
    loop {
        match client_conn.receiver.recv() {
            Ok(Message::Response(resp)) if resp.id == shutdown_id => break,
            Ok(_) => continue,
            Err(err) => panic!("connection closed before shutdown response: {err}"),
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
