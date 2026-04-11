//! LSP server lifecycle + in-memory document store.
//!
//! This module owns:
//!   - `run()`: the entry point that wires up stdio, drives the initialize
//!     handshake, and runs the main message loop until `shutdown` + `exit`.
//!   - `DocumentStore`: a tiny wrapper around `HashMap<Url, String>` that
//!     `didOpen` / `didChange` / `didClose` mutate.
//!   - `run_with_connection()`: the same lifecycle loop, but parameterised
//!     over a `lsp_server::Connection`. This is what the integration tests in
//!     `tarn-lsp/tests/` drive over an in-memory transport.
//!
//! Phase L1.2 (NAZ-291) layers diagnostics on top of L1.1's document store.
//! The main loop now:
//!
//!   1. Flushes a `publishDiagnostics` immediately on `didOpen` and `didSave`.
//!   2. Records a 300ms debounce deadline on each `didChange`. The loop uses
//!      `recv_timeout` so it wakes in time to fire the pending diagnostics.
//!   3. Clears diagnostics (empty array) on `didClose`.
//!
//! The debounce strategy is deliberately runtime-free — no threads, no async,
//! no tokio — so the server remains a single synchronous loop. See
//! [`crate::debounce`] for the pure helper.

use std::collections::HashMap;
use std::error::Error;
use std::time::Instant;

use lsp_server::{Connection, ExtractError, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _,
};
use lsp_types::request::{
    CodeLensRequest, Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest,
    PrepareRenameRequest, References, Rename, Request as _,
};
use lsp_types::{
    CodeLensParams, CompletionParams, DocumentSymbolParams, GotoDefinitionParams, HoverParams,
    InitializeParams, ReferenceParams, RenameParams, TextDocumentPositionParams, Url,
};

use crate::capabilities::server_capabilities;
use crate::code_lens;
use crate::completion;
use crate::debounce::DebounceTracker;
use crate::definition;
use crate::diagnostics;
use crate::hover;
use crate::references;
use crate::rename;
use crate::symbols;
use crate::workspace::WorkspaceIndex;

/// Short tag used in the `eprintln!` server-info log. Tests grep for this.
pub const SERVER_NAME: &str = "tarn-lsp";

/// Tracked version of the server binary — mirrors `tarn` and `tarn-mcp`.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// In-memory store of currently open documents, keyed by LSP `Url`.
///
/// Phase L1.1 uses full-document sync, so each `didChange` replaces the entire
/// text for that URI. When a document is closed the entry is evicted — the
/// server never reads from disk, which keeps it simple and monorepo-safe.
#[derive(Debug, Default)]
pub struct DocumentStore {
    docs: HashMap<Url, String>,
}

impl DocumentStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the full text of a newly opened document.
    pub fn open(&mut self, uri: Url, text: String) {
        self.docs.insert(uri, text);
    }

    /// Replace the full text of an already-tracked document.
    ///
    /// If `didChange` arrives for a URI the server hasn't seen, we still
    /// accept it — some clients send change events before the matching
    /// didOpen if the document was already dirty at startup.
    pub fn change(&mut self, uri: Url, text: String) {
        self.docs.insert(uri, text);
    }

    /// Forget a document. Called from `didClose`.
    pub fn close(&mut self, uri: &Url) {
        self.docs.remove(uri);
    }

    /// Number of open documents.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// `true` when no documents are tracked.
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Fetch the text of an open document, if any.
    pub fn get(&self, uri: &Url) -> Option<&str> {
        self.docs.get(uri).map(String::as_str)
    }
}

/// Aggregated server state passed to handlers that need more than just
/// the in-memory document store.
///
/// L2.2 (NAZ-298) introduces the first cross-file feature
/// (`textDocument/references`), which needs both the document store and
/// the workspace index — the latter caches every `.tarn.yaml` outline
/// in the workspace root so the references walker doesn't reparse the
/// world on every keystroke. Existing handlers (hover, completion,
/// definition, document symbols) still take `&DocumentStore` directly;
/// only references reaches into the full state.
#[derive(Debug, Default)]
pub struct ServerState {
    /// In-memory document store. Keys are LSP `Url`s exactly as the
    /// client sends them.
    pub documents: DocumentStore,
    /// Bounded cross-file cache populated lazily on the first reference
    /// query. See [`crate::workspace::WorkspaceIndex`] for the
    /// invalidation rules.
    pub workspace_index: WorkspaceIndex,
}

impl ServerState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Entry point for the real binary: hook up stdio and run the loop.
pub fn run() -> Result<(), Box<dyn Error + Sync + Send>> {
    let (connection, io_threads) = Connection::stdio();
    run_with_connection(connection)?;
    io_threads.join()?;
    Ok(())
}

/// Drive the LSP lifecycle over an arbitrary `Connection`.
///
/// This is split out from [`run`] so integration tests can exercise the
/// full handshake over `Connection::memory()` without spawning a subprocess.
pub fn run_with_connection(connection: Connection) -> Result<(), Box<dyn Error + Sync + Send>> {
    let (initialize_id, initialize_params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(initialize_params)?;

    // Capture the workspace root URL up front so cross-file features
    // (NAZ-298 references and beyond) know where to walk. The LSP spec
    // prefers `workspace_folders` over the deprecated `root_uri`, so
    // we check both — first folder wins when both are populated.
    let workspace_root: Option<Url> = params
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first().map(|folder| folder.uri.clone()))
        .or_else(|| {
            #[allow(deprecated)]
            {
                params.root_uri.clone()
            }
        });

    let initialize_result = serde_json::json!({
        "capabilities": server_capabilities(),
        "serverInfo": {
            "name": SERVER_NAME,
            "version": SERVER_VERSION,
        },
    });

    // Log to stderr so any LSP client's "Language Server" output pane shows
    // the server identified itself. Plain `eprintln!` — no tracing dep.
    eprintln!("{} {} initialized", SERVER_NAME, SERVER_VERSION);

    connection.initialize_finish(initialize_id, initialize_result)?;

    main_loop(&connection, workspace_root)?;
    Ok(())
}

/// Main message loop. Dispatches requests and notifications until the
/// `shutdown` request arrives, then returns so the caller can drain `exit`.
///
/// Uses `recv_timeout` whenever a debounce deadline is pending so the loop
/// can wake itself and flush diagnostics without a separate thread.
fn main_loop(
    connection: &Connection,
    workspace_root: Option<Url>,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let mut state = ServerState::new();
    state.workspace_index.set_root(workspace_root);
    let mut debounce = DebounceTracker::new();

    loop {
        // Pick a wait strategy based on whether any debounces are pending.
        // An idle server blocks on `recv()`. A server with a pending fire
        // blocks on `recv_timeout()` with the exact wake-up duration, so
        // we fire diagnostics on schedule even if no new messages arrive.
        let msg = match debounce.earliest_deadline() {
            Some(deadline) => {
                let now = Instant::now();
                let wait = deadline.saturating_duration_since(now);
                if wait.is_zero() {
                    // Already past the deadline — flush before reading the
                    // next message so we don't sleep while diagnostics pile up.
                    flush_due_debounces(&mut debounce, &state.documents, connection, now)?;
                    continue;
                }
                match connection.receiver.recv_timeout(wait) {
                    Ok(msg) => msg,
                    Err(err) if is_timeout(&err) => {
                        // Debounce fired — flush and loop back to recv().
                        flush_due_debounces(
                            &mut debounce,
                            &state.documents,
                            connection,
                            Instant::now(),
                        )?;
                        continue;
                    }
                    // Any other error means the connection is closed; exit
                    // the loop like the old blocking `recv()` did on EOF.
                    Err(_) => return Ok(()),
                }
            }
            None => match connection.receiver.recv() {
                Ok(msg) => msg,
                Err(_) => return Ok(()),
            },
        };

        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    // `handle_shutdown` sends the reply itself and returns
                    // true once the client has asked to shut down. The loop
                    // exits; the next message the client sends should be the
                    // `exit` notification, at which point the sender side of
                    // the connection is closed and the receiver loop ends.
                    return Ok(());
                }
                // Dispatch typed LSP requests. Phase L1 handles hover
                // (L1.3), completion (L1.4), and documentSymbol (L1.5).
                // L2.1 added go-to-definition; L2.2 adds references.
                // Anything else falls through to a JSON-RPC "method not
                // found" response until later tickets add more handlers.
                let resp = dispatch_request(req, &mut state);
                connection.sender.send(Message::Response(resp))?;
            }
            Message::Response(_) => {
                // We never send server-to-client requests today.
            }
            Message::Notification(note) => {
                handle_notification(note, &mut state, &mut debounce, connection)?;
            }
        }
    }
}

/// Flush every URL whose debounce deadline has passed.
///
/// Each flushed URL has its pending entry removed from the tracker before
/// publishing, so a slow validator cannot pile up duplicate fires while it
/// runs.
fn flush_due_debounces(
    debounce: &mut DebounceTracker,
    store: &DocumentStore,
    connection: &Connection,
    now: Instant,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    for uri in debounce.drain_due(now) {
        diagnostics::validate_and_publish(store, &uri, connection)?;
    }
    Ok(())
}

/// True when a `RecvTimeoutError` is the `Timeout` variant rather than
/// `Disconnected`. We pattern-match on the `Display` output to avoid a
/// direct `crossbeam_channel` dependency — the `lsp_server` crate re-exports
/// its channel types but the variant names are not part of its public API.
fn is_timeout(err: &crossbeam_channel::RecvTimeoutError) -> bool {
    matches!(err, crossbeam_channel::RecvTimeoutError::Timeout)
}

/// Route a typed LSP request to the right handler and produce the
/// matching [`Response`]. Unknown methods fall through to
/// [`method_not_found`] so clients get a predictable error rather than
/// a silent hang.
///
/// L1.3 dispatches `textDocument/hover` through [`hover::text_document_hover`];
/// L1.4 adds `textDocument/completion` via
/// [`completion::text_document_completion`]; L1.5 adds
/// `textDocument/documentSymbol` via
/// [`symbols::text_document_document_symbol`]. L2.1 adds
/// `textDocument/definition` via [`definition::text_document_definition`].
/// L2.2 adds `textDocument/references` via
/// [`references::text_document_references`]. L2.3 adds
/// `textDocument/rename` + `prepareRename` via [`rename`]. L2.4 adds
/// `textDocument/codeLens` via [`code_lens::text_document_code_lens`],
/// which completes Phase L2.
fn dispatch_request(req: Request, state: &mut ServerState) -> Response {
    // Capture the id up-front: `Request::extract` takes `self` by value
    // so we can't read `req.id` after a failed extract.
    let id = req.id.clone();
    match req.method.as_str() {
        HoverRequest::METHOD => match req.extract::<HoverParams>(HoverRequest::METHOD) {
            Ok((req_id, params)) => {
                let uri = params.text_document_position_params.text_document.uri;
                let position = params.text_document_position_params.position;
                let result = hover::text_document_hover(&state.documents, &uri, position);
                // LSP spec: hover may return `null` when there is nothing
                // to show. We serialize `None` as JSON null via serde_json.
                serialize_response(req_id, &result, "hover")
            }
            Err(ExtractError::MethodMismatch(r)) => method_not_found(r),
            Err(ExtractError::JsonError { method, error }) => invalid_params(id, method, error),
        },
        Completion::METHOD => match req.extract::<CompletionParams>(Completion::METHOD) {
            Ok((req_id, params)) => {
                let uri = params.text_document_position.text_document.uri;
                let position = params.text_document_position.position;
                let result = completion::text_document_completion(&state.documents, &uri, position);
                // LSP spec: completion may return `null`. The helper
                // returns `None` when the cursor has no valid context.
                serialize_response(req_id, &result, "completion")
            }
            Err(ExtractError::MethodMismatch(r)) => method_not_found(r),
            Err(ExtractError::JsonError { method, error }) => invalid_params(id, method, error),
        },
        DocumentSymbolRequest::METHOD => {
            match req.extract::<DocumentSymbolParams>(DocumentSymbolRequest::METHOD) {
                Ok((req_id, params)) => {
                    let uri = params.text_document.uri;
                    let result = symbols::text_document_document_symbol(&state.documents, &uri);
                    serialize_response(req_id, &result, "documentSymbol")
                }
                Err(ExtractError::MethodMismatch(r)) => method_not_found(r),
                Err(ExtractError::JsonError { method, error }) => invalid_params(id, method, error),
            }
        }
        GotoDefinition::METHOD => match req.extract::<GotoDefinitionParams>(GotoDefinition::METHOD)
        {
            Ok((req_id, params)) => {
                let uri = params.text_document_position_params.text_document.uri;
                let position = params.text_document_position_params.position;
                let result = definition::text_document_definition(&state.documents, &uri, position);
                // LSP spec: definition may return `null`. The handler
                // returns `None` for unknown URIs, non-navigable tokens,
                // and empty lookups.
                serialize_response(req_id, &result, "definition")
            }
            Err(ExtractError::MethodMismatch(r)) => method_not_found(r),
            Err(ExtractError::JsonError { method, error }) => invalid_params(id, method, error),
        },
        References::METHOD => match req.extract::<ReferenceParams>(References::METHOD) {
            Ok((req_id, params)) => {
                let result = references::text_document_references(state, params);
                // LSP spec: references returns an array (possibly empty).
                serialize_response(req_id, &result, "references")
            }
            Err(ExtractError::MethodMismatch(r)) => method_not_found(r),
            Err(ExtractError::JsonError { method, error }) => invalid_params(id, method, error),
        },
        PrepareRenameRequest::METHOD => {
            match req.extract::<TextDocumentPositionParams>(PrepareRenameRequest::METHOD) {
                Ok((req_id, params)) => {
                    let result = rename::text_document_prepare_rename(state, params);
                    // LSP spec: prepareRename returns `null` to decline.
                    serialize_response(req_id, &result, "prepareRename")
                }
                Err(ExtractError::MethodMismatch(r)) => method_not_found(r),
                Err(ExtractError::JsonError { method, error }) => invalid_params(id, method, error),
            }
        }
        Rename::METHOD => match req.extract::<RenameParams>(Rename::METHOD) {
            Ok((req_id, params)) => match rename::text_document_rename(state, params) {
                Ok(edit) => serialize_response(req_id, &edit, "rename"),
                Err(err) => Response {
                    id: req_id,
                    result: None,
                    error: Some(err),
                },
            },
            Err(ExtractError::MethodMismatch(r)) => method_not_found(r),
            Err(ExtractError::JsonError { method, error }) => invalid_params(id, method, error),
        },
        CodeLensRequest::METHOD => match req.extract::<CodeLensParams>(CodeLensRequest::METHOD) {
            Ok((req_id, params)) => {
                let uri = params.text_document.uri;
                let result = code_lens::text_document_code_lens(&state.documents, &uri);
                // LSP spec: codeLens returns an array (possibly empty).
                // The handler short-circuits to `Vec::new()` for
                // non-`*.tarn.yaml` URIs and unknown buffers, so clients
                // always get a well-formed JSON array.
                serialize_response(req_id, &result, "codeLens")
            }
            Err(ExtractError::MethodMismatch(r)) => method_not_found(r),
            Err(ExtractError::JsonError { method, error }) => invalid_params(id, method, error),
        },
        _ => method_not_found(req),
    }
}

/// Shared JSON serialization for successful request handlers. Produces
/// a `Response` carrying either the serialized result or an
/// `InternalError` with the formatting failure details.
fn serialize_response<T: serde::Serialize>(
    req_id: lsp_server::RequestId,
    value: &T,
    method_label: &str,
) -> Response {
    match serde_json::to_value(value) {
        Ok(json) => Response {
            id: req_id,
            result: Some(json),
            error: None,
        },
        Err(err) => Response {
            id: req_id,
            result: None,
            error: Some(lsp_server::ResponseError {
                code: lsp_server::ErrorCode::InternalError as i32,
                message: format!("failed to serialize {method_label}: {err}"),
                data: None,
            }),
        },
    }
}

/// Shared JSON-RPC "invalid params" response builder for the request
/// dispatcher. Keeps the hover and completion branches terse.
fn invalid_params(id: lsp_server::RequestId, method: String, err: serde_json::Error) -> Response {
    Response {
        id,
        result: None,
        error: Some(lsp_server::ResponseError {
            code: lsp_server::ErrorCode::InvalidParams as i32,
            message: format!("failed to parse {method} params: {err}"),
            data: None,
        }),
    }
}

/// Build a JSON-RPC "method not found" response for an unhandled request.
fn method_not_found(req: Request) -> Response {
    Response {
        id: req.id,
        result: None,
        error: Some(lsp_server::ResponseError {
            code: lsp_server::ErrorCode::MethodNotFound as i32,
            message: format!("method not supported yet: {}", req.method),
            data: None,
        }),
    }
}

/// Apply a notification to the document store + debounce tracker, and
/// push diagnostics where appropriate.
///
/// This function is where L1.2 hooks into the handler surface that L1.1
/// already owned. Each case matches an acceptance-criterion bullet on the
/// ticket. L2.2 (NAZ-298) extends every change/save/close branch to
/// invalidate the workspace index entry for the affected URL so the next
/// reference query re-reads the freshest content.
fn handle_notification(
    note: Notification,
    state: &mut ServerState,
    debounce: &mut DebounceTracker,
    connection: &Connection,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    match note.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params = cast_notification::<DidOpenTextDocument>(note)?;
            let uri = params.text_document.uri;
            state.documents.open(uri.clone(), params.text_document.text);
            // Drop any stale workspace-index entry — the buffer the
            // client just opened is the source of truth from now on.
            state.workspace_index.invalidate(&uri);
            // Acceptance: "On didOpen ... the server parses the document ...
            // and publishes publishDiagnostics for that URL." — flush now.
            diagnostics::validate_and_publish(&state.documents, &uri, connection)?;
        }
        DidChangeTextDocument::METHOD => {
            let params = cast_notification::<DidChangeTextDocument>(note)?;
            // Full sync: the spec guarantees exactly one content change with
            // the entire document text when the server advertises
            // `TextDocumentSyncKind::FULL`. Take the last one defensively in
            // case any client batches.
            if let Some(change) = params.content_changes.into_iter().next_back() {
                let uri = params.text_document.uri;
                state.documents.change(uri.clone(), change.text);
                state.workspace_index.invalidate(&uri);
                // Debounce the flush: a burst of keystrokes collapses to a
                // single publishDiagnostics after 300ms of quiet.
                debounce.record_change(uri, Instant::now());
            }
        }
        DidCloseTextDocument::METHOD => {
            let params = cast_notification::<DidCloseTextDocument>(note)?;
            let uri = params.text_document.uri;
            // Acceptance: "On didClose, diagnostics for that URI are cleared
            // (publishDiagnostics with empty array)."
            diagnostics::publish_empty(connection, &uri)?;
            // Then tear down the store entry + any pending debounce.
            state.documents.close(&uri);
            state.workspace_index.invalidate(&uri);
            debounce.forget(&uri);
        }
        DidSaveTextDocument::METHOD => {
            let params = cast_notification::<DidSaveTextDocument>(note)?;
            let uri = params.text_document.uri;
            // Acceptance: "On ... didSave ... publishes publishDiagnostics".
            // Save flushes immediately, bypassing the debounce, and cancels
            // any pending debounced fire for this URL (no double-publish).
            debounce.forget(&uri);
            state.workspace_index.invalidate(&uri);
            diagnostics::validate_and_publish(&state.documents, &uri, connection)?;
        }
        // `initialized` and `exit` are handled by `Connection::initialize_*`
        // and `Connection::handle_shutdown` respectively. Anything else we
        // ignore — servers are required to tolerate unknown notifications.
        _ => {}
    }

    Ok(())
}

/// Deserialize a notification's params into the typed struct declared by `N`.
///
/// Any failure is wrapped in a `Box<dyn Error>` so `main_loop` can bubble it
/// up uniformly.
fn cast_notification<N>(note: Notification) -> Result<N::Params, Box<dyn Error + Sync + Send>>
where
    N: lsp_types::notification::Notification,
{
    note.extract::<N::Params>(N::METHOD).map_err(|e| match e {
        ExtractError::MethodMismatch(n) => format!("method mismatch: got {}", n.method).into(),
        ExtractError::JsonError { method, error } => {
            format!("failed to parse {method} params: {error}").into()
        }
    })
}
