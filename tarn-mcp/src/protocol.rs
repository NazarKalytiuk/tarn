use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::OnceLock;

/// JSON-RPC 2.0 request.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    /// Build a JSON-RPC error response whose `data` carries a structured
    /// payload. Agents consume `{code, message, data}` verbatim instead
    /// of parsing free-form error text, which is the NAZ-407 contract.
    pub fn error_with_data(
        id: Option<Value>,
        code: i32,
        message: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: Some(data),
            }),
        }
    }
}

/// Workspace root captured from the MCP `initialize` handshake.
///
/// MCP clients may announce their active workspace in the initialization
/// `params` as either:
///   - `rootUri` / `rootPath` (legacy LSP-style single root), or
///   - `workspaceFolders` (an array of `{ uri, name }` objects, where the
///     first entry is the primary workspace).
///
/// We capture *one* path here — the first usable value we can extract —
/// and surface it from [`workspace_root`] so [`tools::resolve_cwd`] can
/// default to it when the caller does not pass an explicit `cwd`. This is
/// requirement #3 of NAZ-248.
///
/// `OnceLock` keeps the API read-only after `initialize` — an MCP client
/// only initializes once per session, and storing the root immutably
/// prevents mid-session drift if a misbehaving client were to call
/// `initialize` again.
static WORKSPACE_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Capture the workspace root from an MCP `initialize` request's `params`.
///
/// Extracts the first path-like value it finds among `rootUri`,
/// `rootPath`, and `workspaceFolders[0].uri`. URIs with a `file://`
/// scheme are stripped to their filesystem path so downstream code can
/// use them unchanged. Non-absolute or non-existent paths are *not*
/// rejected here — [`tools::resolve_cwd`] performs that validation when
/// it reads the captured value. We do skip values that clearly cannot be
/// paths (empty strings, non-string JSON values) so a bad handshake does
/// not poison the cache.
pub fn capture_workspace_root(init_params: &Value) -> Option<PathBuf> {
    let candidate = workspace_root_from_params(init_params)?;
    // `set` returns Err if already initialized; that is fine — once a
    // workspace is captured it is final for the process lifetime.
    let _ = WORKSPACE_ROOT.set(candidate.clone());
    Some(candidate)
}

/// Read the workspace root captured during `initialize`, if any.
pub fn workspace_root() -> Option<PathBuf> {
    WORKSPACE_ROOT.get().cloned()
}

/// Pull the best available workspace root out of the `initialize` params.
/// Exposed as a free function (not behind `OnceLock`) so the unit tests
/// can exercise every branch without needing to reset process-global
/// state between cases.
fn workspace_root_from_params(init_params: &Value) -> Option<PathBuf> {
    // Priority 1: `workspaceFolders[0].uri` — the modern MCP/LSP field.
    if let Some(folders) = init_params
        .get("workspaceFolders")
        .and_then(|v| v.as_array())
    {
        for folder in folders {
            if let Some(uri) = folder.get("uri").and_then(|v| v.as_str()) {
                if let Some(path) = path_from_uri_or_path(uri) {
                    return Some(path);
                }
            }
        }
    }

    // Priority 2: `rootUri` (deprecated but still common).
    if let Some(root_uri) = init_params.get("rootUri").and_then(|v| v.as_str()) {
        if let Some(path) = path_from_uri_or_path(root_uri) {
            return Some(path);
        }
    }

    // Priority 3: `rootPath` (very old LSP-style hosts).
    if let Some(root_path) = init_params.get("rootPath").and_then(|v| v.as_str()) {
        if let Some(path) = path_from_uri_or_path(root_path) {
            return Some(path);
        }
    }

    None
}

/// Convert `file://…` URIs to a plain filesystem path, or accept the
/// string unchanged if it already looks like a bare path. Returns
/// `None` for empty strings so the fallback chain can continue to the
/// next candidate.
fn path_from_uri_or_path(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("file://") {
        // MCP clients sometimes send `file:///Users/foo` and sometimes
        // `file://localhost/Users/foo`. Strip an optional authority so
        // the path component starts with `/`.
        let without_authority = match rest.find('/') {
            Some(idx) => &rest[idx..],
            None => rest,
        };
        if without_authority.is_empty() {
            return None;
        }
        return Some(PathBuf::from(without_authority));
    }
    Some(PathBuf::from(trimmed))
}

/// MCP server info returned during initialization.
pub fn server_info() -> Value {
    serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "tarn-mcp",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

/// MCP tool definitions.
pub fn tools_list() -> Value {
    serde_json::json!({
        "tools": [
            {
                "name": "tarn_run",
                "description": "Run API tests defined in .tarn.yaml files. Writes artifacts under .tarn/runs/<run_id>/ and returns a compact agent report by default plus paths to the full artifacts so agents do not need to keep large JSON blobs in context.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to a .tarn.yaml test file or directory containing test files. Relative paths resolve against `cwd`."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Absolute path to the project root. tarn.config.yaml, tarn.env.yaml, and relative paths are resolved against this directory. Defaults to the workspace root captured during MCP `initialize`, or the server process's current directory."
                        },
                        "env": {
                            "type": "string",
                            "description": "Environment name (loads tarn.env.{name}.yaml)"
                        },
                        "vars": {
                            "type": "object",
                            "description": "Variable overrides as key-value pairs",
                            "additionalProperties": { "type": "string" }
                        },
                        "tag": {
                            "type": "string",
                            "description": "Filter tests by tag (comma-separated)"
                        },
                        "report_mode": {
                            "type": "string",
                            "enum": ["full", "summary", "failures", "agent"],
                            "description": "Which slice of the run to return inline. `agent` (default) is the compact root-cause-first payload; `summary` and `failures` return the NAZ-401 artifacts; `full` returns the verbose JSON report. The run still writes every artifact regardless of the chosen mode."
                        }
                    }
                }
            },
            {
                "name": "tarn_validate",
                "description": "Validate .tarn.yaml test files without executing them. Checks YAML syntax and schema validity.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to a .tarn.yaml file or directory. Relative paths resolve against `cwd`."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Absolute path to the project root. Defaults to the workspace root captured during MCP `initialize`, or the server process's current directory."
                        }
                    },
                    "required": ["path"]
                }
            },
            {
                "name": "tarn_list",
                "description": "List all available tests in .tarn.yaml files. Returns file names, test names, and step counts.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to directory (defaults to `cwd`). Relative paths resolve against `cwd`."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Absolute path to the project root. Defaults to the workspace root captured during MCP `initialize`, or the server process's current directory."
                        }
                    }
                }
            },
            {
                "name": "tarn_fix_plan",
                "description": "Analyze a Tarn JSON report and return a prioritized fix plan with next actions, evidence, and remediation hints. Accepts either a `report` object from `tarn_run` or the same inputs as `tarn_run` to execute first.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "report": {
                            "type": "object",
                            "description": "Structured JSON report from tarn_run"
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional .tarn.yaml path or directory to run before planning. Relative paths resolve against `cwd`."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Absolute path to the project root used when `path` is provided. Defaults to the workspace root captured during MCP `initialize`, or the server process's current directory."
                        },
                        "env": {
                            "type": "string",
                            "description": "Environment name used when `path` is provided"
                        },
                        "vars": {
                            "type": "object",
                            "description": "Variable overrides used when `path` is provided",
                            "additionalProperties": { "type": "string" }
                        },
                        "tag": {
                            "type": "string",
                            "description": "Tag filter used when `path` is provided"
                        },
                        "max_items": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Limit the number of failing steps included in the plan"
                        }
                    }
                }
            },
            {
                "name": "tarn_last_failures",
                "description": "Return the grouped failures (NAZ-402) for a specific run as structured JSON. Reads the persisted `failures.json` rather than re-running the tests. Useful for agents that want a failures-only view without loading the full report.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cwd": {
                            "type": "string",
                            "description": "Absolute path to the project root. Defaults to the workspace root captured during MCP `initialize`, or the server process's current directory."
                        },
                        "run_id": {
                            "type": "string",
                            "description": "Run identifier or alias (`last`, `prev`, `@latest`, or a literal `YYYYmmdd-HHMMSS-xxxxxx` id). Defaults to `last`."
                        }
                    }
                }
            },
            {
                "name": "tarn_get_run_artifacts",
                "description": "Return artifact paths plus existence flags for a specific run. Does not load any artifact payload — just tells the agent what is on disk for the given run.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cwd": {
                            "type": "string",
                            "description": "Absolute path to the project root. Defaults to the workspace root captured during MCP `initialize`, or the server process's current directory."
                        },
                        "run_id": {
                            "type": "string",
                            "description": "Run identifier or alias. Defaults to `last`."
                        }
                    }
                }
            },
            {
                "name": "tarn_rerun_failed",
                "description": "Rerun only the failing `(file, test)` pairs from a prior run. Response shape matches `tarn_run` (run_id, artifacts, report) so agents can loop run → inspect → rerun without switching tool surfaces.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cwd": {
                            "type": "string",
                            "description": "Absolute path to the project root. Defaults to the workspace root captured during MCP `initialize`, or the server process's current directory."
                        },
                        "run_id": {
                            "type": "string",
                            "description": "Source run identifier or alias to seed the selection from. Defaults to `last` (the workspace-level `.tarn/failures.json` pointer)."
                        },
                        "env_name": {
                            "type": "string",
                            "description": "Environment name to resolve for the rerun (loads tarn.env.{name}.yaml)."
                        },
                        "vars": {
                            "type": "object",
                            "description": "Variable overrides as key-value pairs.",
                            "additionalProperties": { "type": "string" }
                        },
                        "report_mode": {
                            "type": "string",
                            "enum": ["full", "summary", "failures", "agent"],
                            "description": "Which slice of the rerun's report to return inline. Defaults to `agent`."
                        }
                    }
                }
            },
            {
                "name": "tarn_report",
                "description": "Render the concise report (NAZ-404) for a persisted run: a tiny JSON envelope with totals, exit code, and grouped failures. No HTTP, no test execution — purely reads `summary.json` + `failures.json`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cwd": {
                            "type": "string",
                            "description": "Absolute path to the project root. Defaults to the workspace root captured during MCP `initialize`, or the server process's current directory."
                        },
                        "run_id": {
                            "type": "string",
                            "description": "Run identifier or alias. Defaults to `last`."
                        }
                    }
                }
            },
            {
                "name": "tarn_inspect",
                "description": "Inspect a prior run's archived report (NAZ-405) at run, file, test, or step granularity. Optional `filter_category` narrows the view to one FailureCategory. Response includes artifact paths for the run that seeded the view.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cwd": {
                            "type": "string",
                            "description": "Absolute path to the project root. Defaults to the workspace root captured during MCP `initialize`, or the server process's current directory."
                        },
                        "run_id": {
                            "type": "string",
                            "description": "Run identifier or alias (`last`, `prev`, etc.). Defaults to `last`."
                        },
                        "target": {
                            "type": "string",
                            "description": "Address of the entity to inspect: `FILE`, `FILE::TEST`, or `FILE::TEST::STEP`. Omit for the run-level view."
                        },
                        "filter_category": {
                            "type": "string",
                            "description": "Narrow the run-level view to steps whose `failure_category` matches this value."
                        }
                    }
                }
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_list_matches_golden_contract() {
        let actual: serde_json::Value =
            serde_json::from_str(&serde_json::to_string_pretty(&tools_list()).unwrap()).unwrap();
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("../tests/golden/tools-list.json.golden")).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn workspace_root_from_params_reads_workspace_folders_first() {
        let params = serde_json::json!({
            "rootUri": "file:///legacy/path",
            "workspaceFolders": [
                { "uri": "file:///modern/path", "name": "modern" }
            ]
        });
        assert_eq!(
            workspace_root_from_params(&params),
            Some(PathBuf::from("/modern/path"))
        );
    }

    #[test]
    fn workspace_root_from_params_falls_back_to_root_uri() {
        let params = serde_json::json!({ "rootUri": "file:///legacy/path" });
        assert_eq!(
            workspace_root_from_params(&params),
            Some(PathBuf::from("/legacy/path"))
        );
    }

    #[test]
    fn workspace_root_from_params_falls_back_to_root_path() {
        let params = serde_json::json!({ "rootPath": "/plain/path" });
        assert_eq!(
            workspace_root_from_params(&params),
            Some(PathBuf::from("/plain/path"))
        );
    }

    #[test]
    fn workspace_root_from_params_returns_none_without_hints() {
        let params = serde_json::json!({ "foo": "bar" });
        assert_eq!(workspace_root_from_params(&params), None);
    }

    #[test]
    fn workspace_root_from_params_skips_empty_values() {
        let params = serde_json::json!({
            "rootUri": "",
            "rootPath": "/real/path"
        });
        assert_eq!(
            workspace_root_from_params(&params),
            Some(PathBuf::from("/real/path"))
        );
    }

    #[test]
    fn path_from_uri_or_path_handles_file_scheme_with_authority() {
        assert_eq!(
            path_from_uri_or_path("file://localhost/var/foo"),
            Some(PathBuf::from("/var/foo"))
        );
    }

    #[test]
    fn path_from_uri_or_path_handles_file_scheme_without_authority() {
        assert_eq!(
            path_from_uri_or_path("file:///var/foo"),
            Some(PathBuf::from("/var/foo"))
        );
    }

    #[test]
    fn path_from_uri_or_path_handles_bare_path() {
        assert_eq!(
            path_from_uri_or_path("/already/absolute"),
            Some(PathBuf::from("/already/absolute"))
        );
    }

    #[test]
    fn path_from_uri_or_path_rejects_empty() {
        assert_eq!(path_from_uri_or_path(""), None);
        assert_eq!(path_from_uri_or_path("   "), None);
    }
}
