//! `workspace/executeCommand` handler for `tarn.evaluateJsonpath`.
//!
//! Ships with L3.6 (NAZ-307) as the companion to the JSONPath hover
//! class in [`crate::hover`]. LSP clients (Claude Code, the upcoming
//! VS Code migration under Phase V, any generic LSP consumer) can
//! invoke this command to evaluate a JSONPath against either:
//!
//!   * an **inline response** — the client hands over the full
//!     response body as a JSON value, and the handler returns the
//!     matches without touching the filesystem.
//!   * a **step reference** — the client identifies a step in an
//!     open buffer by `(file, test, step)` triple, and the handler
//!     looks up the sidecar response via the same
//!     [`RecordedResponseSource`] trait the scaffold-assert code
//!     action already consumes.
//!
//! ## Return shape (NAZ-254)
//!
//! Every response is wrapped in [`crate::envelope`] and carries a
//! `result` discriminator in the inner payload:
//!
//!   * `"match"` — the JSONPath parsed and produced at least one
//!     value. `value` is the first match (or the sole match); the
//!     full list is also returned under `values` when there are
//!     multiple so the renderer can show array-flavoured output.
//!   * `"no_match"` — the JSONPath parsed but did not select
//!     anything. `available_top_keys` helps the LLM guess what the
//!     response actually looks like.
//!   * `"no_fixture"` — the step reference could not be resolved to a
//!     recorded response. `message` explains why.
//!
//! Parse errors still bubble up as `InvalidParams` so callers get
//! an explicit RPC error they can surface in diagnostics; the three
//! success-style variants above are the only ones the handler
//! returns inside `data`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_server::{ErrorCode, ResponseError};
use lsp_types::{ExecuteCommandParams, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tarn::jsonpath::{evaluate_path, JsonPathError};

use crate::code_actions::response_source::{DiskResponseSource, RecordedResponseSource};
use crate::envelope;

/// Stable LSP command id advertised in [`crate::capabilities`] and
/// dispatched by [`crate::server::dispatch_request`]. Exposed as a
/// constant so the tests, the capability advertisement, and the
/// server wiring all reference one source of truth.
pub const EVALUATE_JSONPATH_COMMAND: &str = "tarn.evaluateJsonpath";

/// Arguments accepted by `tarn.evaluateJsonpath`.
///
/// Uses an untagged enum so the client can pick between the two
/// shapes based on whichever context it has available. Clients that
/// are already sitting on a recorded response choose
/// [`EvaluateArgs::Inline`]; clients that only know the enclosing
/// step choose [`EvaluateArgs::StepRef`].
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum EvaluateArgs {
    /// `{ "path": "<jsonpath>", "response": <inline-json-value> }`
    Inline {
        /// JSONPath expression to evaluate.
        path: String,
        /// Inline JSON response body to evaluate against. Any valid
        /// JSON value is accepted — object, array, scalar, `null`.
        response: Value,
    },
    /// `{ "path": "<jsonpath>", "step": { "file": "...", "test": "...", "step": "..." } }`
    StepRef {
        /// JSONPath expression to evaluate.
        path: String,
        /// Step reference used to look up the sidecar response.
        step: StepRef,
    },
}

/// A step reference used by [`EvaluateArgs::StepRef`] to resolve the
/// recorded response through the sidecar convention (NAZ-304).
#[derive(Debug, Clone, Deserialize)]
pub struct StepRef {
    /// Absolute filesystem path of the `.tarn.yaml` buffer. An LSP
    /// `file://` URI is also accepted — the handler converts it.
    pub file: String,
    /// Enclosing test group's name, or the sentinel `"setup"` /
    /// `"teardown"` / `"<flat>"` for steps outside any test.
    pub test: String,
    /// Step's `name:` value.
    pub step: String,
}

/// Return payload carried inside the [`crate::envelope`] wrapper.
///
/// The three variants correspond to the NAZ-254 contract:
///
///   * `Match` — the JSONPath parsed and matched at least one value.
///   * `NoMatch` — the path parsed but no values were selected.
///   * `NoFixture` — no recorded response was found for the step
///     reference. Parse errors on the path itself do **not** fall
///     into this variant; they come back as `InvalidParams`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum EvaluationResult {
    Match {
        /// The first match (equivalent to the sole match when there
        /// is exactly one). Kept as a top-level field so
        /// single-value JSONPath consumers don't need to unwrap an
        /// array.
        value: Value,
        /// Full list of matches, in document order. `[value]` for a
        /// single match.
        values: Vec<Value>,
    },
    NoMatch {
        /// Top-level keys of the evaluated response, so the LLM
        /// can guide the user towards a valid path. Empty for
        /// non-object responses.
        available_top_keys: Vec<String>,
    },
    NoFixture {
        /// Human-readable explanation of why the fixture is missing.
        message: String,
    },
}

/// Parse and validate the raw `ExecuteCommandParams.arguments`
/// payload into an [`EvaluateArgs`].
///
/// Callers that only need the argument-parse step (unit tests,
/// future command providers that want to reuse the shape) can call
/// this without invoking the full command dispatch.
pub fn parse_evaluate_args(args: &[Value]) -> Result<EvaluateArgs, ResponseError> {
    let first = args
        .first()
        .ok_or_else(|| invalid_params("tarn.evaluateJsonpath requires one argument object"))?;
    serde_json::from_value::<EvaluateArgs>(first.clone()).map_err(|e| {
        invalid_params(format!(
            "tarn.evaluateJsonpath: invalid argument shape: {e}. Expected {{\"path\": ..., \"response\": ...}} or {{\"path\": ..., \"step\": {{\"file\": ..., \"test\": ..., \"step\": ...}}}}"
        ))
    })
}

/// Resolution outcome for [`resolve_response`].
///
/// Mirrors the three-variant return shape so the handler can
/// propagate a missing sidecar as `NoFixture` without inventing a
/// custom error code or erroring the RPC.
#[derive(Debug, Clone)]
pub enum ResolvedResponse {
    /// Body is available — either inline or read from the sidecar.
    Body(Value),
    /// The step reference did not resolve to a recorded response.
    NoFixture(String),
}

/// Resolve an [`EvaluateArgs`] to its underlying response body,
/// pulling the sidecar JSON through `source` for the step-ref branch
/// and passing inline values straight through otherwise.
///
/// Unlike the original (L3.6) implementation, a missing sidecar is
/// **not** an RPC error — it is returned as [`ResolvedResponse::NoFixture`]
/// so the NAZ-254 three-variant payload can surface it to the
/// client inline with the rest of the evaluation outcome.
pub fn resolve_response(
    args: &EvaluateArgs,
    source: &dyn RecordedResponseSource,
) -> Result<ResolvedResponse, ResponseError> {
    match args {
        EvaluateArgs::Inline { response, .. } => Ok(ResolvedResponse::Body(response.clone())),
        EvaluateArgs::StepRef { step, .. } => {
            let path = step_file_to_pathbuf(&step.file);
            match source.read(&path, &step.test, &step.step) {
                Some(body) => Ok(ResolvedResponse::Body(body)),
                None => Ok(ResolvedResponse::NoFixture(format!(
                    "no fixture recorded for step `{}` in test `{}` at `{}`; run the test once.",
                    step.step,
                    step.test,
                    path.display()
                ))),
            }
        }
    }
}

/// Convert a step-ref `file` field into a filesystem path. Accepts
/// both bare filesystem strings and `file://` URIs — the latter are
/// normalised through [`Url::to_file_path`] so Windows drive letters
/// come through intact.
fn step_file_to_pathbuf(file: &str) -> PathBuf {
    if let Ok(url) = Url::parse(file) {
        if let Ok(p) = url.to_file_path() {
            return p;
        }
    }
    PathBuf::from(file)
}

/// Entry point for the `tarn.evaluateJsonpath` dispatch used by the
/// server's `workspace/executeCommand` router. Always returns the
/// [`crate::envelope`]-wrapped payload on success.
pub fn dispatch_evaluate_jsonpath(
    params: ExecuteCommandParams,
) -> Result<Value, ResponseError> {
    let source: Arc<dyn RecordedResponseSource> = Arc::new(DiskResponseSource);
    let result = execute_evaluate_jsonpath(&params.arguments, source.as_ref())?;
    envelope::wrap(result).map_err(internal_error_from_serde)
}

/// Parse + dispatch + evaluate a `tarn.evaluateJsonpath` command.
///
/// Pure apart from the `source` parameter, so unit tests can wire an
/// [`crate::code_actions::response_source::InMemoryResponseSource`]
/// and exercise every branch without touching disk.
pub fn execute_evaluate_jsonpath(
    args: &[Value],
    source: &dyn RecordedResponseSource,
) -> Result<EvaluationResult, ResponseError> {
    let parsed = parse_evaluate_args(args)?;
    let path = match &parsed {
        EvaluateArgs::Inline { path, .. } | EvaluateArgs::StepRef { path, .. } => path.clone(),
    };
    let resolved = resolve_response(&parsed, source)?;
    let response = match resolved {
        ResolvedResponse::Body(value) => value,
        ResolvedResponse::NoFixture(message) => {
            return Ok(EvaluationResult::NoFixture { message });
        }
    };
    let matches = evaluate_path(&path, &response).map_err(|JsonPathError::Parse(msg)| {
        invalid_params(format!(
            "tarn.evaluateJsonpath: invalid JSONPath expression `{path}`: {msg}"
        ))
    })?;
    Ok(classify_matches(matches, &response))
}

/// Turn a JSONPath match vector into one of the three
/// [`EvaluationResult`] variants, populating `available_top_keys`
/// for the no-match case.
fn classify_matches(matches: Vec<Value>, response: &Value) -> EvaluationResult {
    if matches.is_empty() {
        let available_top_keys = top_keys(response);
        return EvaluationResult::NoMatch { available_top_keys };
    }
    let value = matches[0].clone();
    EvaluationResult::Match {
        value,
        values: matches,
    }
}

fn top_keys(value: &Value) -> Vec<String> {
    match value {
        Value::Object(map) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

fn invalid_params(message: impl Into<String>) -> ResponseError {
    ResponseError {
        code: ErrorCode::InvalidParams as i32,
        message: message.into(),
        data: None,
    }
}

fn internal_error_from_serde(err: serde_json::Error) -> ResponseError {
    ResponseError {
        code: ErrorCode::InternalError as i32,
        message: format!("tarn.evaluateJsonpath: failed to serialise result: {err}"),
        data: None,
    }
}

/// Canonical absolute path used by tests — exposed so integration
/// tests and unit tests resolve against the same location.
#[doc(hidden)]
pub fn _test_file_path(name: &str) -> PathBuf {
    Path::new("/tmp/jsonpath-eval").join(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_actions::response_source::InMemoryResponseSource;
    use serde_json::json;

    #[test]
    fn parse_args_inline_happy_path() {
        let raw = vec![json!({"path": "$.x", "response": {"x": 1}})];
        let args = parse_evaluate_args(&raw).expect("parse ok");
        match args {
            EvaluateArgs::Inline { path, response } => {
                assert_eq!(path, "$.x");
                assert_eq!(response, json!({"x": 1}));
            }
            EvaluateArgs::StepRef { .. } => panic!("expected Inline"),
        }
    }

    #[test]
    fn parse_args_step_ref_happy_path() {
        let raw = vec![json!({
            "path": "$.id",
            "step": { "file": "/tmp/f.tarn.yaml", "test": "main", "step": "list" }
        })];
        let args = parse_evaluate_args(&raw).expect("parse ok");
        match args {
            EvaluateArgs::StepRef { path, step } => {
                assert_eq!(path, "$.id");
                assert_eq!(step.file, "/tmp/f.tarn.yaml");
                assert_eq!(step.test, "main");
                assert_eq!(step.step, "list");
            }
            EvaluateArgs::Inline { .. } => panic!("expected StepRef"),
        }
    }

    #[test]
    fn parse_args_missing_arg_returns_invalid_params() {
        let raw: Vec<Value> = Vec::new();
        let err = parse_evaluate_args(&raw).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams as i32);
        assert!(err.message.contains("one argument object"));
    }

    #[test]
    fn parse_args_malformed_object_returns_invalid_params() {
        let raw = vec![json!({"nonsense": true})];
        let err = parse_evaluate_args(&raw).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams as i32);
        assert!(err.message.contains("invalid argument shape"));
    }

    #[test]
    fn execute_inline_happy_path_returns_match_variant_with_values() {
        let source = InMemoryResponseSource::empty();
        let raw =
            vec![json!({"path": "$.items[*].id", "response": {"items": [{"id": 1}, {"id": 2}]}})];
        let result = execute_evaluate_jsonpath(&raw, &source).expect("ok");
        match result {
            EvaluationResult::Match { value, values } => {
                assert_eq!(value, json!(1));
                assert_eq!(values, vec![json!(1), json!(2)]);
            }
            other => panic!("expected Match variant, got {other:?}"),
        }
    }

    #[test]
    fn execute_inline_no_match_returns_no_match_with_top_keys() {
        let source = InMemoryResponseSource::empty();
        let raw = vec![json!({"path": "$.missing", "response": {"present": 1, "other": 2}})];
        let result = execute_evaluate_jsonpath(&raw, &source).expect("ok");
        match result {
            EvaluationResult::NoMatch { available_top_keys } => {
                let mut keys = available_top_keys.clone();
                keys.sort();
                assert_eq!(keys, vec!["other", "present"]);
            }
            other => panic!("expected NoMatch variant, got {other:?}"),
        }
    }

    #[test]
    fn execute_step_ref_happy_path_uses_in_memory_source() {
        let response = json!({"items": [{"id": 42}]});
        let source = InMemoryResponseSource::new(response);
        let raw = vec![json!({
            "path": "$.items[0].id",
            "step": { "file": "/tmp/any.tarn.yaml", "test": "main", "step": "list" }
        })];
        let result = execute_evaluate_jsonpath(&raw, &source).expect("ok");
        match result {
            EvaluationResult::Match { value, .. } => assert_eq!(value, json!(42)),
            other => panic!("expected Match, got {other:?}"),
        }
    }

    #[test]
    fn execute_step_ref_missing_sidecar_returns_no_fixture_variant() {
        let source = InMemoryResponseSource::empty();
        let raw = vec![json!({
            "path": "$.x",
            "step": { "file": "/tmp/any.tarn.yaml", "test": "main", "step": "list" }
        })];
        let result = execute_evaluate_jsonpath(&raw, &source).expect("ok");
        match result {
            EvaluationResult::NoFixture { message } => {
                assert!(message.contains("no fixture"), "message: {message}");
            }
            other => panic!("expected NoFixture, got {other:?}"),
        }
    }

    #[test]
    fn execute_inline_bad_jsonpath_returns_invalid_params_error() {
        let source = InMemoryResponseSource::empty();
        let raw = vec![json!({"path": "$.[not valid", "response": {}})];
        let err = execute_evaluate_jsonpath(&raw, &source).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams as i32);
        assert!(err.message.contains("invalid JSONPath expression"));
    }

    #[test]
    fn execute_inline_with_scalar_response_matches_root() {
        let source = InMemoryResponseSource::empty();
        let raw = vec![json!({"path": "$", "response": 42})];
        let result = execute_evaluate_jsonpath(&raw, &source).expect("ok");
        match result {
            EvaluationResult::Match { value, values } => {
                assert_eq!(value, json!(42));
                assert_eq!(values, vec![json!(42)]);
            }
            other => panic!("expected Match, got {other:?}"),
        }
    }

    #[test]
    fn step_file_to_pathbuf_accepts_plain_path() {
        let p = step_file_to_pathbuf("/tmp/x.tarn.yaml");
        assert_eq!(p, PathBuf::from("/tmp/x.tarn.yaml"));
    }

    #[test]
    fn step_file_to_pathbuf_accepts_file_url() {
        let p = step_file_to_pathbuf("file:///tmp/x.tarn.yaml");
        assert_eq!(p, PathBuf::from("/tmp/x.tarn.yaml"));
    }

    #[test]
    fn evaluation_result_match_serialises_with_result_discriminator() {
        let r = EvaluationResult::Match {
            value: json!("alpha"),
            values: vec![json!("alpha"), json!(true)],
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["result"], json!("match"));
        assert_eq!(v["value"], json!("alpha"));
        assert_eq!(v["values"], json!(["alpha", true]));
    }

    #[test]
    fn evaluation_result_no_match_serialises_with_top_keys() {
        let r = EvaluationResult::NoMatch {
            available_top_keys: vec!["a".into(), "b".into()],
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["result"], json!("no_match"));
        assert_eq!(v["available_top_keys"], json!(["a", "b"]));
    }

    #[test]
    fn evaluation_result_no_fixture_serialises_with_message() {
        let r = EvaluationResult::NoFixture {
            message: "nothing recorded yet".into(),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["result"], json!("no_fixture"));
        assert_eq!(v["message"], json!("nothing recorded yet"));
    }
}
