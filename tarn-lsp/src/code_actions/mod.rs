//! `textDocument/codeAction` dispatcher and renderers.
//!
//! Phase L3.2 (NAZ-303) introduces a pure dispatcher over a flat list of
//! "provider" free functions. Each provider inspects the cursor / range
//! on behalf of a concrete refactor (extract env var, scaffold assert,
//! quick fix from `tarn_fix_plan`, …) and returns zero or more fully
//! resolved [`CodeAction`]s with [`WorkspaceEdit`]s already populated —
//! no `codeAction/resolve` round-trip is needed.
//!
//! The dispatcher pattern is deliberately flat: a `Vec<fn(...) -> Vec<CodeAction>>`
//! would be premature abstraction for a three-to-five-provider list.
//! [`code_actions_for_range`] calls each provider inline; future L3
//! tickets (L3.3, L3.4) plug their renderers into the same function
//! without touching the server wiring.
//!
//! # Architecture
//!
//! Two layers, mirroring every other L2/L3 feature in this crate:
//!
//!   1. A pure renderer layer — [`code_actions_for_range`] and each
//!      provider's helper function — takes a [`CodeActionContext`]
//!      built by the caller. Pure, filesystem-free, and unit-testable
//!      with a synthetic context literal.
//!
//!   2. A thin connection-facing wrapper — [`text_document_code_action`]
//!      — builds the context from [`crate::server::ServerState`] and
//!      dispatches. This is the only place that touches the document
//!      store, the workspace index, or the filesystem env loader.
//!
//! # Stable action kinds
//!
//! The capability struct in [`crate::capabilities`] advertises three
//! kinds right now:
//!
//!   * [`CodeActionKind::REFACTOR_EXTRACT`] — used by **extract env var**.
//!     Every "lift a value into a named reference" refactor goes here.
//!   * [`CodeActionKind::REFACTOR`] — reserved for L3.3 (capture-field
//!     refactor). Declared now so the capability struct is stable from
//!     this ticket forward and clients do not see a capability
//!     regression when L3.3 ships.
//!   * [`CodeActionKind::QUICKFIX`] — reserved for L3.4 (fix-plan
//!     quick fix). Same reasoning.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lsp_types::{
    CodeAction, CodeActionContext as LspCodeActionContext, CodeActionOrCommand, CodeActionParams,
    CodeActionResponse, Range, Url,
};
use tarn::env::{self, EnvEntry};
use tarn::parser;

use crate::server::ServerState;

pub mod extract_env;

/// Context every code-action provider receives.
///
/// Built once per request by [`text_document_code_action`] and reused
/// across every provider the dispatcher walks. Holds only borrowed
/// references so providers can read the document without cloning it.
pub struct CodeActionContext<'a> {
    /// LSP URI of the current buffer.
    pub uri: &'a Url,
    /// Full text of the current buffer — borrowed from the document
    /// store.
    pub source: &'a str,
    /// Resolved env map as of the current buffer's position on disk.
    /// Used for collision detection when coining new env keys.
    pub env: &'a std::collections::BTreeMap<String, EnvEntry>,
    /// The `CodeActionContext` supplied by the LSP request. Empty by
    /// default — clients send `only` / `diagnostics` when they want to
    /// filter the returned actions.
    pub lsp_ctx: &'a LspCodeActionContext,
}

/// Pure dispatcher: walk every provider and collect their results.
///
/// Providers are called in registration order and each returns a
/// `Vec<CodeAction>` the dispatcher concatenates. The function is
/// synchronous, filesystem-free, and entirely unit-testable against a
/// synthetic [`CodeActionContext`].
pub fn code_actions_for_range(
    uri: &Url,
    source: &str,
    range: Range,
    ctx: &CodeActionContext<'_>,
) -> Vec<CodeAction> {
    let mut out: Vec<CodeAction> = Vec::new();

    // Provider 1: extract env var (NAZ-303).
    if let Some(action) = extract_env::extract_env_code_action(uri, source, range, ctx) {
        out.push(action);
    }

    // Future providers (NAZ-304, NAZ-305) plug in here.

    out
}

/// `textDocument/codeAction` request entry point.
///
/// Builds a [`CodeActionContext`] from the server state and delegates
/// to [`code_actions_for_range`]. Returns an empty vector for unknown
/// URIs, unparseable buffers, and every other soft failure so clients
/// always get a well-formed JSON array.
pub fn text_document_code_action(
    state: &mut ServerState,
    params: CodeActionParams,
) -> CodeActionResponse {
    let uri = params.text_document.uri.clone();
    let range = params.range;
    let lsp_ctx = params.context;

    let Some(source) = state.documents.get(&uri).map(|s| s.to_owned()) else {
        return Vec::new();
    };

    let env_map = build_env_map(&uri, &source);

    let ctx = CodeActionContext {
        uri: &uri,
        source: &source,
        env: &env_map,
        lsp_ctx: &lsp_ctx,
    };

    let actions = code_actions_for_range(&uri, &source, range, &ctx);
    actions
        .into_iter()
        .map(CodeActionOrCommand::CodeAction)
        .collect()
}

/// Build the resolved env map for `uri` / `source` using the same
/// priority chain `references` and `rename` already consult.
///
/// Parse failures and filesystem errors fold gracefully to an empty
/// map — the renderer only needs collision detection, so a missing
/// env layer just means fewer names to avoid.
fn build_env_map(uri: &Url, source: &str) -> std::collections::BTreeMap<String, EnvEntry> {
    let path = uri_to_path(uri);
    let parse_result = parser::parse_str(source, &path);
    let inline_env = match &parse_result {
        Ok(tf) => tf.env.clone(),
        Err(_) => HashMap::new(),
    };
    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    env::resolve_env_with_sources(
        &inline_env,
        None,
        &[],
        &base_dir,
        "tarn.env.yaml",
        &HashMap::new(),
    )
    .unwrap_or_default()
}

fn uri_to_path(uri: &Url) -> PathBuf {
    uri.to_file_path()
        .unwrap_or_else(|_| PathBuf::from(uri.path()))
}
