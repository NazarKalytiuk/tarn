//! Convert Tarn validation messages into LSP diagnostics and push them to
//! the client.
//!
//! This module owns three things:
//!
//! 1. [`tarn_messages_to_diagnostics`] — a **pure** conversion from the
//!    `Vec<ValidationMessage>` returned by `tarn::validation::validate_document`
//!    into the `lsp_types::Diagnostic` shape. It has no side effects and is
//!    exhaustively unit-tested.
//! 2. [`validate_and_publish`] — the orchestrator called from `server.rs`.
//!    It reads the current buffer out of the `DocumentStore`, feeds it through
//!    Tarn's in-process validator, converts the result, and pushes a
//!    `publishDiagnostics` notification onto the connection's sender.
//! 3. [`publish_empty`] — used from `didClose` to clear any stale
//!    diagnostics for a closed URI.
//!
//! The module also pins the LSP `source` field (`"tarn"`) and centralises
//! the 1-based → 0-based line/column conversion so there is exactly one place
//! that can get off-by-one wrong.

use std::error::Error;

use lsp_server::{Connection, Message, Notification};
use lsp_types::notification::{Notification as _, PublishDiagnostics};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, NumberOrString, Position, PublishDiagnosticsParams, Range, Url,
};
use tarn::model::Location;
use tarn::validation::{self, Severity as TarnSeverity, ValidationMessage};

use crate::server::{is_tarn_file_uri, DocumentStore};

/// LSP `Diagnostic.source` value. Keep this as a single constant so editors
/// that filter by source (Claude Code does) can anchor on one stable string.
pub const DIAGNOSTIC_SOURCE: &str = "tarn";

/// Read the current in-memory text for `uri`, run Tarn's validator, and push
/// a `publishDiagnostics` notification to the connected client.
///
/// If `uri` has no open document in `store` this is a no-op — the server
/// would otherwise publish diagnostics for a buffer the client has already
/// closed, which some clients treat as a protocol violation.
///
/// Non-`*.tarn.yaml` URIs are silently skipped: tarn-lsp is attached to
/// every `.yaml` file by Claude Code's extension-based LSP matcher, so
/// this guard keeps us from running the Tarn validator against foreign
/// YAML (Kubernetes manifests, Compose files, etc.) and polluting the
/// client's diagnostic panel.
pub fn validate_and_publish(
    store: &DocumentStore,
    uri: &Url,
    connection: &Connection,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    if !is_tarn_file_uri(uri) {
        return Ok(());
    }
    let Some(source) = store.get(uri) else {
        return Ok(());
    };
    let path = uri_to_path(uri);
    let messages = validation::validate_document(&path, source);
    let diagnostics = tarn_messages_to_diagnostics(&messages);
    publish(connection, uri, diagnostics)
}

/// Push an empty `publishDiagnostics` for `uri` so any previously reported
/// diagnostics disappear from the client.
///
/// Non-`*.tarn.yaml` URIs are a no-op for the same reason
/// [`validate_and_publish`] is: we never published anything for them, so
/// there's nothing to clear.
pub fn publish_empty(
    connection: &Connection,
    uri: &Url,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    if !is_tarn_file_uri(uri) {
        return Ok(());
    }
    publish(connection, uri, Vec::new())
}

/// Convert a slice of `tarn::validation::ValidationMessage` into LSP
/// diagnostics. This function is pure — it takes messages, returns
/// diagnostics, and talks to nothing else. Tests drive it directly.
///
/// Range computation:
///   - If the message carries a [`Location`], we use its 1-based
///     `(line, column)` and convert to 0-based LSP `Position`. The range is
///     zero-width (same start and end) because Tarn does not currently
///     report a span, only a point. This still underlines the target token
///     in every major LSP client we tested.
///   - If the message has no location (e.g. a semantic "file must have
///     steps or tests" error), we fall back to a zero-zero range so the
///     diagnostic still surfaces rather than being dropped.
///
/// Field mapping:
///   - `severity`  → `Error` | `Warning` (we never emit Hint or Information)
///   - `source`    → `"tarn"` (always)
///   - `code`      → `NumberOrString::String(msg.code.as_str().to_owned())`
///   - `message`   → `msg.message`
pub fn tarn_messages_to_diagnostics(msgs: &[ValidationMessage]) -> Vec<Diagnostic> {
    msgs.iter().map(message_to_diagnostic).collect()
}

fn message_to_diagnostic(msg: &ValidationMessage) -> Diagnostic {
    Diagnostic {
        range: location_to_range(msg.location.as_ref()),
        severity: Some(severity_to_lsp(msg.severity)),
        code: Some(NumberOrString::String(msg.code.as_str().to_owned())),
        code_description: None,
        source: Some(DIAGNOSTIC_SOURCE.to_owned()),
        message: msg.message.clone(),
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Map Tarn's [`Severity`](TarnSeverity) onto the LSP
/// [`DiagnosticSeverity`] enum.
fn severity_to_lsp(severity: TarnSeverity) -> DiagnosticSeverity {
    match severity {
        TarnSeverity::Error => DiagnosticSeverity::ERROR,
        TarnSeverity::Warning => DiagnosticSeverity::WARNING,
    }
}

/// Convert an optional 1-based `Location` into a (zero-width) LSP `Range`.
///
/// This is the single chokepoint for 1-based → 0-based conversion. Be
/// defensive against the pathological `line == 0` / `column == 0` edge case:
/// saturating subtraction keeps us at `0` so we never underflow.
fn location_to_range(location: Option<&Location>) -> Range {
    match location {
        Some(loc) => {
            let line = loc.line.saturating_sub(1) as u32;
            let character = loc.column.saturating_sub(1) as u32;
            let pos = Position::new(line, character);
            Range::new(pos, pos)
        }
        None => Range::new(Position::new(0, 0), Position::new(0, 0)),
    }
}

/// Serialize and dispatch a `textDocument/publishDiagnostics` notification.
fn publish(
    connection: &Connection,
    uri: &Url,
    diagnostics: Vec<Diagnostic>,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    };
    let note = Notification {
        method: PublishDiagnostics::METHOD.to_owned(),
        params: serde_json::to_value(params)?,
    };
    connection.sender.send(Message::Notification(note))?;
    Ok(())
}

/// Convert an LSP `Url` to a `PathBuf` for the validator.
///
/// LSP clients send `file://` URIs. `Url::to_file_path` handles the common
/// case on every platform we care about. When the URI is not a file URI
/// (e.g. `untitled:` from some clients), we fall back to treating the URI
/// path segment as the display path — the resulting diagnostics will still
/// render, they just won't be anchored to a real file on disk.
fn uri_to_path(uri: &Url) -> std::path::PathBuf {
    uri.to_file_path()
        .unwrap_or_else(|_| std::path::PathBuf::from(uri.path()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tarn::validation::{Severity, ValidationCode};

    fn loc(line: usize, column: usize) -> Location {
        Location {
            file: "test.tarn.yaml".to_owned(),
            line,
            column,
        }
    }

    #[test]
    fn empty_messages_produce_empty_diagnostics() {
        let diagnostics = tarn_messages_to_diagnostics(&[]);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn parse_error_maps_to_error_diagnostic_with_one_based_location_converted() {
        let msg = ValidationMessage {
            severity: Severity::Error,
            code: ValidationCode::TarnParse,
            message: "something broke".to_owned(),
            location: Some(loc(3, 5)),
        };
        let diagnostics = tarn_messages_to_diagnostics(&[msg]);
        assert_eq!(diagnostics.len(), 1);
        let d = &diagnostics[0];
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(d.source.as_deref(), Some("tarn"));
        assert_eq!(d.message, "something broke");
        assert_eq!(
            d.code,
            Some(NumberOrString::String("tarn_parse".to_owned()))
        );
        // 1-based (3, 5) must convert to 0-based (2, 4).
        assert_eq!(d.range.start.line, 2);
        assert_eq!(d.range.start.character, 4);
        assert_eq!(d.range.end.line, 2);
        assert_eq!(d.range.end.character, 4);
    }

    #[test]
    fn schema_violation_also_maps_to_error_severity() {
        let msg = ValidationMessage {
            severity: Severity::Error,
            code: ValidationCode::TarnValidation,
            message: "unknown field 'requestt'".to_owned(),
            location: Some(loc(7, 9)),
        };
        let d = &tarn_messages_to_diagnostics(&[msg])[0];
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("tarn_validation".to_owned()))
        );
        assert_eq!(d.range.start.line, 6);
        assert_eq!(d.range.start.character, 8);
    }

    #[test]
    fn warning_severity_flips_to_lsp_warning() {
        // No current tarn check emits a warning, but the pipeline must be
        // ready for one. When `Severity::Warning` is produced, it must
        // surface as `DiagnosticSeverity::WARNING`, not ERROR.
        let msg = ValidationMessage {
            severity: Severity::Warning,
            code: ValidationCode::TarnValidation,
            message: "deprecated field".to_owned(),
            location: Some(loc(1, 1)),
        };
        let d = &tarn_messages_to_diagnostics(&[msg])[0];
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
    }

    #[test]
    fn missing_location_falls_back_to_zero_zero_range() {
        let msg = ValidationMessage {
            severity: Severity::Error,
            code: ValidationCode::TarnParse,
            message: "Test file must have either 'steps' or 'tests'".to_owned(),
            location: None,
        };
        let d = &tarn_messages_to_diagnostics(&[msg])[0];
        // Still published — the diagnostic is not dropped.
        assert_eq!(d.range.start, Position::new(0, 0));
        assert_eq!(d.range.end, Position::new(0, 0));
    }

    #[test]
    fn location_with_zero_line_or_column_does_not_underflow() {
        // Defensive: if some future code path produced 1-based = 0 (illegal
        // but previously crashed the server), we must saturate, not panic.
        let msg = ValidationMessage {
            severity: Severity::Error,
            code: ValidationCode::YamlSyntax,
            message: "bad".to_owned(),
            location: Some(loc(0, 0)),
        };
        let d = &tarn_messages_to_diagnostics(&[msg])[0];
        assert_eq!(d.range.start, Position::new(0, 0));
    }

    #[test]
    fn yaml_syntax_code_is_preserved_on_diagnostic() {
        let msg = ValidationMessage {
            severity: Severity::Error,
            code: ValidationCode::YamlSyntax,
            message: "unexpected '['".to_owned(),
            location: Some(loc(2, 8)),
        };
        let d = &tarn_messages_to_diagnostics(&[msg])[0];
        assert_eq!(
            d.code,
            Some(NumberOrString::String("yaml_syntax".to_owned()))
        );
    }

    #[test]
    fn multiple_messages_preserve_order_and_count() {
        let msgs = vec![
            ValidationMessage {
                severity: Severity::Error,
                code: ValidationCode::YamlSyntax,
                message: "first".to_owned(),
                location: Some(loc(1, 1)),
            },
            ValidationMessage {
                severity: Severity::Warning,
                code: ValidationCode::TarnValidation,
                message: "second".to_owned(),
                location: None,
            },
            ValidationMessage {
                severity: Severity::Error,
                code: ValidationCode::TarnParse,
                message: "third".to_owned(),
                location: Some(loc(9, 2)),
            },
        ];
        let diagnostics = tarn_messages_to_diagnostics(&msgs);
        assert_eq!(diagnostics.len(), 3);
        assert_eq!(diagnostics[0].message, "first");
        assert_eq!(diagnostics[1].message, "second");
        assert_eq!(diagnostics[2].message, "third");
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diagnostics[1].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(diagnostics[2].severity, Some(DiagnosticSeverity::ERROR));
    }
}
