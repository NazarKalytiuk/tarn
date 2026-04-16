//! `tarn-lsp` library surface.
//!
//! The crate ships as a binary (`tarn-lsp`) that speaks LSP over stdio, but
//! we also expose a thin library so the integration tests in `tarn-lsp/tests`
//! can drive the same handshake and lifecycle over an in-memory
//! `lsp_server::Connection` without spawning a subprocess.
//!
//! Nothing here is considered stable public API — consumers should depend on
//! the `tarn-lsp` binary, not this library.

pub mod capabilities;
pub mod code_actions;
pub mod code_lens;
pub mod completion;
pub mod debounce;
pub mod definition;
pub mod diagnostics;
pub mod formatting;
pub mod hover;
pub mod identifier;
pub mod jsonpath_eval;
pub mod references;
pub mod rename;
pub mod schema;
pub mod server;
pub mod symbols;
pub mod token;
pub mod workspace;

pub use capabilities::server_capabilities;
pub use server::{
    is_tarn_file_uri, run, run_with_connection, DocumentStore, ServerState, SERVER_NAME,
    SERVER_VERSION,
};
