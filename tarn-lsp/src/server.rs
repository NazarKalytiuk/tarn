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
//! Phase L1.1 deliberately contains no language intelligence. The message
//! handlers update the document store and return. Later tickets (NAZ-291
//! through NAZ-294) will attach diagnostics, hover, completion, and symbol
//! behaviour onto the dispatch points already defined here.

use std::collections::HashMap;
use std::error::Error;

use lsp_server::{Connection, ExtractError, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _,
};
use lsp_types::{InitializeParams, Url};

use crate::capabilities::server_capabilities;

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
    let _params: InitializeParams = serde_json::from_value(initialize_params)?;

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

    main_loop(&connection)?;
    Ok(())
}

/// Main message loop. Dispatches requests and notifications until the
/// `shutdown` request arrives, then returns so the caller can drain `exit`.
fn main_loop(connection: &Connection) -> Result<(), Box<dyn Error + Sync + Send>> {
    let mut store = DocumentStore::new();

    for msg in &connection.receiver {
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
                // Phase L1.1 has no other requests to answer. Any request
                // that isn't shutdown gets a "method not found" error so
                // clients can recover cleanly.
                let resp = method_not_found(req);
                connection.sender.send(Message::Response(resp))?;
            }
            Message::Response(_) => {
                // We never send server-to-client requests in L1.1.
            }
            Message::Notification(note) => {
                handle_notification(note, &mut store)?;
            }
        }
    }

    Ok(())
}

/// Build a JSON-RPC "method not found" response for an unhandled request.
fn method_not_found(req: Request) -> Response {
    Response {
        id: req.id,
        result: None,
        error: Some(lsp_server::ResponseError {
            code: lsp_server::ErrorCode::MethodNotFound as i32,
            message: format!("method not supported in Phase L1.1: {}", req.method),
            data: None,
        }),
    }
}

/// Apply a notification to the document store.
fn handle_notification(
    note: Notification,
    store: &mut DocumentStore,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    match note.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params = cast_notification::<DidOpenTextDocument>(note)?;
            store.open(params.text_document.uri, params.text_document.text);
        }
        DidChangeTextDocument::METHOD => {
            let params = cast_notification::<DidChangeTextDocument>(note)?;
            // Full sync: the spec guarantees exactly one content change with
            // the entire document text when the server advertises
            // `TextDocumentSyncKind::FULL`. Take the last one defensively in
            // case any client batches.
            if let Some(change) = params.content_changes.into_iter().next_back() {
                store.change(params.text_document.uri, change.text);
            }
        }
        DidCloseTextDocument::METHOD => {
            let params = cast_notification::<DidCloseTextDocument>(note)?;
            store.close(&params.text_document.uri);
        }
        DidSaveTextDocument::METHOD => {
            // Accepted but intentionally a no-op. L1.2 (NAZ-291) will hook
            // diagnostics here. We still parse the params to catch malformed
            // messages early.
            let _params = cast_notification::<DidSaveTextDocument>(note)?;
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
