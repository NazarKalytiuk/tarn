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
//! - L3.1 (NAZ-302): `document_formatting_provider: Some(OneOf::Left(true))` —
//!   whole-document formatting via `tarn::format::format_document`.
//!   Range formatting is deliberately **not** advertised; the parser
//!   re-renders the whole buffer so a range-only edit cannot be produced
//!   without touching the surrounding YAML. Shipped.
//! - L3.2 (NAZ-303): `code_action_provider: CodeActionOptions` —
//!   `textDocument/codeAction` dispatcher with the first provider
//!   (**extract env var**) wired in. Advertises `refactor.extract`
//!   now plus `refactor` and `quickfix` reserved for L3.3 / L3.4 so
//!   the capability struct is stable from this ticket forward.
//!   `resolve_provider: false` — actions come back fully resolved
//!   with their `WorkspaceEdit` already populated so there is no
//!   `codeAction/resolve` round trip. Shipped.
//! - L3.6 (NAZ-307): `execute_command_provider: ExecuteCommandOptions` —
//!   `workspace/executeCommand` with one stable command
//!   `tarn.evaluateJsonpath`. Accepts an inline response + JSONPath
//!   or a `(file, test, step)` triple that resolves through the
//!   sidecar reader, and returns `{ "matches": [...] }`. The LSP
//!   hover provider also grows a fifth token class, **JSONPath
//!   literal**, that evaluates `$.foo` against the step's last
//!   recorded response inline in the hover markdown. Shipping L3.6
//!   completes **Phase L3** — tarn-lsp editing is now done.
//!
//! Nothing in this file should ever grow conditional logic — if a capability
//! is on, it is on for every client and every workspace.

use lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionProviderCapability, CodeLensOptions,
    CompletionOptions, ExecuteCommandOptions, HoverProviderCapability, OneOf, RenameOptions,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, WorkDoneProgressOptions,
};

use crate::commands::ALL_COMMAND_IDS;

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

        // L3.1: the server answers `textDocument/formatting` requests
        // for `.tarn.yaml` buffers by routing through
        // `tarn::format::format_document` — the same library function
        // the `tarn fmt` CLI calls. Range formatting is deliberately
        // **not** advertised (see module doc comment above): the Tarn
        // formatter re-renders the whole buffer, so a range edit
        // cannot be produced without touching surrounding YAML.
        // `document_range_formatting_provider` therefore stays unset.
        document_formatting_provider: Some(OneOf::Left(true)),

        // L3.2: the server answers `textDocument/codeAction` requests
        // with a dispatcher that walks a list of providers. The only
        // provider shipped right now is **extract env var**
        // (`refactor.extract`); L3.3 (capture-field refactor) will
        // plug into `refactor` and L3.4 (fix-plan quick fix) will
        // plug into `quickfix`. Declaring all three kinds now keeps
        // the capability struct stable from this ticket forward so a
        // client never sees a "new kind appeared" regression.
        // `resolve_provider: false` — the dispatcher returns fully
        // resolved actions (WorkspaceEdit already populated), so no
        // `codeAction/resolve` round trip is needed for the MVP.
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::REFACTOR_EXTRACT,
                CodeActionKind::REFACTOR,
                CodeActionKind::QUICKFIX,
            ]),
            resolve_provider: Some(false),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        })),

        // L3.6 (NAZ-307): the server answers `workspace/executeCommand`
        // for a single stable command ID, `tarn.evaluateJsonpath`. The
        // command accepts `{ "path": "<jsonpath>", "response": <inline> }`
        // or `{ "path": "<jsonpath>", "step": { "file": ..., "test": ..., "step": ... } }`
        // and returns `{ "matches": [...] }`. Shipping L3.6 completes
        // Phase L3 — the tarn-lsp editing surface is now done; Phase V
        // (VS Code extension migration onto tarn-lsp) is the next
        // coordinated initiative.
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: ALL_COMMAND_IDS.iter().map(|s| (*s).to_owned()).collect(),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        }),

        // All other capabilities are intentionally left unset. See the module
        // docs for the ticket that turns each one on.
        ..ServerCapabilities::default()
    }
}
