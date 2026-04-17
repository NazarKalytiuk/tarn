//! Shared response envelope for every `workspace/executeCommand`
//! handler registered by tarn-lsp.
//!
//! Every command returns the same outer shape:
//!
//! ```json
//! { "schema_version": 1, "data": <payload> }
//! ```
//!
//! Clients (Claude Code, Cursor, the upcoming VS Code rewrite) read
//! `schema_version` first and refuse to parse newer envelopes they do
//! not recognise. This lets us evolve individual command payloads
//! without shipping a breaking change to the whole surface.
//!
//! Bumping [`COMMAND_SCHEMA_VERSION`] requires coordinated updates in
//! `docs/commands.json`, the VS Code extension's manifest, and every
//! tarn-mcp client release note.

use serde::Serialize;
use serde_json::Value;

/// Stable envelope schema version. Bumped only when the envelope
/// itself changes shape — individual payload evolutions stay within
/// the same envelope version.
pub const COMMAND_SCHEMA_VERSION: u32 = 1;

/// Wrap `payload` in the standard `{ "schema_version", "data" }`
/// envelope. Used by every command handler that needs to return a
/// JSON value to the client.
pub fn wrap<T: Serialize>(payload: T) -> Result<Value, serde_json::Error> {
    Ok(serde_json::json!({
        "schema_version": COMMAND_SCHEMA_VERSION,
        "data": serde_json::to_value(payload)?,
    }))
}

/// Convenience: wrap an already-serialised `Value`.
pub fn wrap_value(payload: Value) -> Value {
    serde_json::json!({
        "schema_version": COMMAND_SCHEMA_VERSION,
        "data": payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wrap_emits_schema_version_and_data() {
        let v = wrap(json!({"x": 1})).unwrap();
        assert_eq!(v["schema_version"], json!(COMMAND_SCHEMA_VERSION));
        assert_eq!(v["data"], json!({"x": 1}));
    }

    #[test]
    fn wrap_value_keeps_payload_intact() {
        let v = wrap_value(json!([1, 2, 3]));
        assert_eq!(v["data"], json!([1, 2, 3]));
    }
}
