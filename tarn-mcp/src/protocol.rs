use serde::{Deserialize, Serialize};
use serde_json::Value;

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
                "description": "Run API tests defined in .tarn.yaml files and return structured JSON results. Use this to execute API tests and get detailed pass/fail information with assertion details.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to a .tarn.yaml test file or directory containing test files"
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
                            "description": "Path to a .tarn.yaml file or directory"
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
                            "description": "Path to directory (defaults to current directory)"
                        }
                    }
                }
            }
        ]
    })
}
