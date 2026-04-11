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
//! - L1.1 (NAZ-290): `text_document_sync: Full`. Shipped.
//! - L1.2 (NAZ-291): diagnostics on open/change/save. Shipped. No new
//!   capability field — `textDocument/publishDiagnostics` is a
//!   server-pushed notification and does not require a capability flag.
//! - L1.3 (NAZ-292): `hover_provider: Simple(true)`. Shipped.
//! - L1.4 (NAZ-293): `completion_provider: Some(CompletionOptions { .. })`
//!   with trigger characters `.` and `$`. Shipped.
//! - L1.5 (NAZ-294): `document_symbol_provider: Some(OneOf::Left(true))` —
//!   the final Phase L1 feature. Shipped.
//! - L2.1 (NAZ-297): `definition_provider: Some(OneOf::Left(true))` —
//!   go-to-definition for `{{ capture.* }}` and `{{ env.* }}`. Shipped.
//! - L2.2 (NAZ-298): `references_provider: Some(OneOf::Left(true))` —
//!   `textDocument/references` for capture and env interpolation tokens,
//!   with a workspace-wide walk for env keys. Shipped.
//! - L2.3 (NAZ-299): `rename_provider: Some(OneOf::Right(RenameOptions { prepare_provider: Some(true), .. }))` —
//!   `textDocument/rename` + `textDocument/prepareRename` for capture
//!   and env interpolation tokens, with identifier validation and
//!   per-scope collision detection. Shipped.
//! - L2.4 (NAZ-300): `code_lens_provider: Some(CodeLensOptions { resolve_provider: Some(false) })` —
//!   `textDocument/codeLens` emitting `Run test` / `Run step` actions
//!   with stable `tarn.runTest` / `tarn.runStep` command IDs. Shipped.
//!   This is the last Phase L2 capability — Phase L2 is now complete.
//!
//! Nothing in this file should ever grow conditional logic — if a capability
//! is on, it is on for every client and every workspace.

use lsp_types::{
    CodeLensOptions, CompletionOptions, HoverProviderCapability, OneOf, RenameOptions,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, WorkDoneProgressOptions,
};

/// Return the `ServerCapabilities` this server currently advertises.
///
/// The contents of this function are the entire public surface area of the
/// server as of phase L1.3. Tests should assert against the output of this
/// function directly rather than spinning up a full stdio round-trip.
pub fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        // Full-document sync. The server keeps the last known full text of
        // every open document in its `DocumentStore`. Incremental sync will
        // not be added in Phase L1 — the parser in `tarn::parser` consumes
        // whole files anyway, so incremental sync would be wasted effort.
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),

        // L1.3: the server answers `textDocument/hover` requests for
        // interpolation tokens (`{{ env.x }}`, `{{ capture.y }}`,
        // `{{ $builtin }}`) and top-level schema keys. The hover body is
        // always Markdown, so `Simple(true)` is the correct signal — we
        // do not need the structured `HoverOptions` variant.
        hover_provider: Some(HoverProviderCapability::Simple(true)),

        // L1.4: context-aware completion for interpolation tokens and
        // top-level schema keys. Trigger characters `.` and `$` match the
        // two punctuation marks that fire completion inside an
        // interpolation (`{{ env.`, `{{ $…`). Resolve is not implemented —
        // the list builders populate every field on each item up-front.
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".to_owned(), "$".to_owned()]),
            resolve_provider: Some(false),
            ..Default::default()
        }),

        // L1.5: the server answers `textDocument/documentSymbol` requests
        // with a hierarchical tree: file root (Namespace) → named tests
        // (Module) → steps (Function), plus setup / teardown / flat steps
        // as Function siblings. `OneOf::Left(true)` is the minimal form —
        // there are no extra options to configure (we do not support
        // work-done progress for symbol requests).
        document_symbol_provider: Some(OneOf::Left(true)),

        // L2.1: the server answers `textDocument/definition` requests
        // for `{{ capture.* }}` and `{{ env.* }}` interpolation tokens.
        // `OneOf::Left(true)` is the minimal form — we do not need
        // `DefinitionOptions` because we neither stream partial results
        // nor advertise any extra selectors.
        definition_provider: Some(OneOf::Left(true)),

        // L2.2: the server answers `textDocument/references` requests
        // for the same interpolation tokens. Capture references are
        // scoped per-test inside the current file; env references walk
        // every `.tarn.yaml` under the workspace root, bounded by the
        // 5000-file safety net inside `crate::workspace::WorkspaceIndex`.
        references_provider: Some(OneOf::Left(true)),

        // L2.3: the server answers `textDocument/rename` and
        // `textDocument/prepareRename` for capture and env interpolation
        // tokens. `prepare_provider: true` asks the client to send a
        // prepareRename round-trip before firing the rename so the UI
        // can highlight just the identifier under the cursor. Rename
        // semantics, identifier validation, and collision rules live in
        // `crate::rename`.
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        })),

        // L2.4: the server answers `textDocument/codeLens` requests for
        // `.tarn.yaml` buffers with one `Run test` lens per named test
        // and one `Run step` lens per step. `resolve_provider: false`
        // — every lens carries its command and arguments up-front so
        // clients never need to issue a `codeLens/resolve` round-trip.
        // Command IDs are `tarn.runTest` / `tarn.runStep`; the server
        // does not execute them, clients handle dispatch themselves.
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(false),
        }),

        // All other capabilities are intentionally left unset. See the module
        // docs for the ticket that turns each one on.
        ..ServerCapabilities::default()
    }
}
