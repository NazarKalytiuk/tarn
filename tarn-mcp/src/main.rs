use serde_json::Value;
use std::io::{self, BufRead, Write};
use tarn_mcp::protocol::{self, JsonRpcRequest, JsonRpcResponse};
use tarn_mcp::tools::{self, ToolError};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::error(None, -32700, format!("Parse error: {}", e));
                write_response(&mut stdout, &resp);
                continue;
            }
        };

        let response = dispatch(&request);

        if request.id.is_none() {
            continue;
        }

        write_response(&mut stdout, &response);
    }
}

fn write_response(out: &mut impl Write, response: &JsonRpcResponse) {
    let json = serde_json::to_string(response).unwrap();
    let _ = writeln!(out, "{}", json);
    let _ = out.flush();
}

fn dispatch(req: &JsonRpcRequest) -> JsonRpcResponse {
    match req.method.as_str() {
        "initialize" => {
            protocol::capture_workspace_root(&req.params);
            JsonRpcResponse::success(req.id.clone(), protocol::server_info())
        }

        "notifications/initialized" => JsonRpcResponse::success(req.id.clone(), Value::Null),

        "tools/list" => JsonRpcResponse::success(req.id.clone(), protocol::tools_list()),

        "tools/call" => handle_tool_call(req),

        _ => JsonRpcResponse::error(
            req.id.clone(),
            -32601,
            format!("Method not found: {}", req.method),
        ),
    }
}

fn handle_tool_call(req: &JsonRpcRequest) -> JsonRpcResponse {
    let tool_name = req
        .params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let arguments = req
        .params
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(serde_json::Map::new()));

    let result = match tool_name {
        "tarn_run" => tools::tarn_run(&arguments),
        "tarn_validate" => tools::tarn_validate(&arguments),
        "tarn_list" => tools::tarn_list(&arguments),
        "tarn_fix_plan" => tools::tarn_fix_plan(&arguments),
        "tarn_last_failures" => tools::tarn_last_failures(&arguments),
        "tarn_get_run_artifacts" => tools::tarn_get_run_artifacts(&arguments),
        "tarn_rerun_failed" => tools::tarn_rerun_failed(&arguments),
        "tarn_report" => tools::tarn_report(&arguments),
        "tarn_inspect" => tools::tarn_inspect(&arguments),
        _ => Err(ToolError::new(
            -32601,
            format!("Unknown tool: {}", tool_name),
        )),
    };

    match result {
        Ok(value) => {
            let content = serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
                }]
            });
            JsonRpcResponse::success(req.id.clone(), content)
        }
        Err(err) => {
            // Surface the structured error in two compatible ways:
            //   - `isError: true` content block (MCP `tools/call` convention)
            //   - the same `{code, message, data}` object embedded in the
            //     `text` so agents that parse the block can read it.
            let payload = err.to_tool_call_json();
            let content = serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
                }],
                "isError": true,
                "error": payload
            });
            JsonRpcResponse::success(req.id.clone(), content)
        }
    }
}
