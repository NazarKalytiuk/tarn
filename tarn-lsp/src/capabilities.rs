//! Server capability advertisement.
//!
//! This module is intentionally small: it owns the single source of truth for
//! what LSP features `tarn-lsp` supports at any given phase of Epic NAZ-289.
//!
//! # Phase L1 wiring plan
//!
//! Keep this block in sync with `docs/TARN_LSP.md` as each feature ticket
//! lands. Each later ticket turns on one additional field below:
//!
//! - L1.1 (NAZ-290, this ticket): `text_document_sync: Full`. Nothing else.
//! - L1.2 (NAZ-291): diagnostics. No new capability field — diagnostics are
//!   pushed via `textDocument/publishDiagnostics`, so the only change will be
//!   handling `didOpen`/`didChange`/`didSave` with a real parse+validate path.
//! - L1.3 (NAZ-292): set `hover_provider: Some(HoverProviderCapability::Simple(true))`.
//! - L1.4 (NAZ-293): set `completion_provider: Some(CompletionOptions { .. })`
//!   with trigger characters for `{`, `.`, `"`.
//! - L1.5 (NAZ-294): set `document_symbol_provider: Some(OneOf::Left(true))`
//!   and publish the final Claude Code configuration snippet.
//!
//! Nothing in this file should ever grow conditional logic — if a capability
//! is on, it is on for every client and every workspace.

use lsp_types::{ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind};

/// Return the `ServerCapabilities` this server currently advertises.
///
/// The contents of this function are the entire public surface area of the
/// server at phase L1.1. Tests should assert against the output of this
/// function directly rather than spinning up a full stdio round-trip.
pub fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        // Full-document sync. The server keeps the last known full text of
        // every open document in its `DocumentStore`. Incremental sync will
        // not be added in Phase L1 — the parser in `tarn::parser` consumes
        // whole files anyway, so incremental sync would be wasted effort.
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),

        // All other capabilities are intentionally left unset. See the module
        // docs for the ticket that turns each one on.
        ..ServerCapabilities::default()
    }
}
