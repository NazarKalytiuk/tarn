//! Library half of the `tarn-mcp` crate.
//!
//! Exposing the MCP tool handlers and the JSON-RPC protocol helpers as
//! a library lets integration tests (under `tarn-mcp/tests/`) exercise
//! them directly, without spinning up the server binary and driving it
//! over stdio.
//!
//! The binary entry point lives in `src/main.rs` and simply wires
//! these modules to stdin/stdout.

pub mod protocol;
pub mod tools;
