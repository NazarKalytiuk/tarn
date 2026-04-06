use crate::assert::types::AssertionResult;
use crate::error::TarnError;
use crate::http::HttpResponse;
use mlua::prelude::*;
use mlua::{Error as LuaError, HookTriggers, LuaOptions, StdLib, VmState};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const SCRIPT_MEMORY_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const SCRIPT_HOOK_GRANULARITY: u32 = 1_000;
const SCRIPT_MAX_INSTRUCTIONS: usize = 100_000;

/// Result of running a Lua script.
#[derive(Debug)]
pub struct ScriptResult {
    pub captures: HashMap<String, serde_json::Value>,
    pub assertion_results: Vec<AssertionResult>,
}

/// Execute a Lua script with access to the HTTP response and current captures.
pub fn run_script(
    script: &str,
    response: &HttpResponse,
    captures: &HashMap<String, serde_json::Value>,
    step_name: &str,
) -> Result<ScriptResult, TarnError> {
    let lua = create_sandboxed_lua()?;

    // Build response table
    let response_table = lua
        .create_table()
        .map_err(|e| TarnError::Script(e.to_string()))?;
    response_table
        .set("status", response.status)
        .map_err(|e| TarnError::Script(e.to_string()))?;

    // Headers table
    let headers_table = lua
        .create_table()
        .map_err(|e| TarnError::Script(e.to_string()))?;
    for (k, v) in &response.headers {
        headers_table
            .set(k.as_str(), v.as_str())
            .map_err(|e| TarnError::Script(e.to_string()))?;
    }
    response_table
        .set("headers", headers_table)
        .map_err(|e| TarnError::Script(e.to_string()))?;

    // Body as Lua value (serde -> Lua conversion)
    let body_lua = lua
        .to_value(&response.body)
        .map_err(|e| TarnError::Script(format!("Failed to convert body to Lua: {}", e)))?;
    response_table
        .set("body", body_lua)
        .map_err(|e| TarnError::Script(e.to_string()))?;

    lua.globals()
        .set("response", response_table)
        .map_err(|e| TarnError::Script(e.to_string()))?;

    // Captures table — push typed JSON values to Lua
    let captures_table = lua
        .create_table()
        .map_err(|e| TarnError::Script(e.to_string()))?;
    for (k, v) in captures {
        let lua_val = lua.to_value(v).map_err(|e| {
            TarnError::Script(format!("Failed to convert capture '{}' to Lua: {}", k, e))
        })?;
        captures_table
            .set(k.as_str(), lua_val)
            .map_err(|e| TarnError::Script(e.to_string()))?;
    }
    lua.globals()
        .set("captures", captures_table)
        .map_err(|e| TarnError::Script(e.to_string()))?;

    // Collect assertion results via overridden assert()
    let assertions: Arc<Mutex<Vec<AssertionResult>>> = Arc::new(Mutex::new(Vec::new()));
    let assertions_clone = assertions.clone();
    let step_name_owned = step_name.to_string();

    let assert_fn = lua
        .create_function(move |_, (condition, message): (bool, Option<String>)| {
            let msg = message.unwrap_or_else(|| "script assertion".to_string());
            let result = if condition {
                AssertionResult::pass(format!("script: {}", msg), "true", "true")
            } else {
                AssertionResult::fail(
                    format!("script: {}", msg),
                    "true",
                    "false",
                    format!("Script assertion failed in '{}': {}", step_name_owned, msg),
                )
            };
            assertions_clone.lock().unwrap().push(result);
            Ok(())
        })
        .map_err(|e| TarnError::Script(e.to_string()))?;

    lua.globals()
        .set("assert", assert_fn)
        .map_err(|e| TarnError::Script(e.to_string()))?;

    // Register json global (json.encode / json.decode)
    register_json_module(&lua)?;

    // Execute script
    lua.load(script)
        .exec()
        .map_err(|e| TarnError::Script(format!("Lua error in step '{}': {}", step_name, e)))?;

    // Extract modified captures — convert Lua types to serde_json::Value
    let final_captures: HashMap<String, serde_json::Value> = {
        let captures_table: LuaTable = lua
            .globals()
            .get("captures")
            .map_err(|e| TarnError::Script(e.to_string()))?;
        let mut result = HashMap::new();
        for pair in captures_table.pairs::<String, LuaValue>() {
            let (k, v) = pair.map_err(|e| TarnError::Script(e.to_string()))?;
            let v_json = lua_value_to_json(v);
            result.insert(k, v_json);
        }
        result
    };

    let assertion_results = assertions.lock().unwrap().clone();

    Ok(ScriptResult {
        captures: final_captures,
        assertion_results,
    })
}

/// Register the `json` global table with `encode` and `decode` functions.
fn register_json_module(lua: &Lua) -> Result<(), TarnError> {
    let json_table = lua
        .create_table()
        .map_err(|e| TarnError::Script(e.to_string()))?;

    // json.decode(string) -> Lua value
    let decode_fn = lua
        .create_function(|lua, s: String| {
            let value: serde_json::Value =
                serde_json::from_str(&s).map_err(|e| LuaError::runtime(e.to_string()))?;
            lua.to_value(&value)
                .map_err(|e| LuaError::runtime(e.to_string()))
        })
        .map_err(|e| TarnError::Script(e.to_string()))?;

    // json.encode(value) -> string
    let encode_fn = lua
        .create_function(|lua, value: LuaValue| {
            let json_value: serde_json::Value = lua
                .from_value(value)
                .map_err(|e| LuaError::runtime(e.to_string()))?;
            serde_json::to_string(&json_value).map_err(|e| LuaError::runtime(e.to_string()))
        })
        .map_err(|e| TarnError::Script(e.to_string()))?;

    json_table
        .set("decode", decode_fn)
        .map_err(|e| TarnError::Script(e.to_string()))?;
    json_table
        .set("encode", encode_fn)
        .map_err(|e| TarnError::Script(e.to_string()))?;

    lua.globals()
        .set("json", json_table)
        .map_err(|e| TarnError::Script(e.to_string()))?;

    Ok(())
}

fn create_sandboxed_lua() -> Result<Lua, TarnError> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(|e| TarnError::Script(format!("Failed to initialize Lua sandbox: {}", e)))?;

    lua.set_memory_limit(SCRIPT_MEMORY_LIMIT_BYTES)
        .map_err(|e| TarnError::Script(format!("Failed to configure Lua memory limit: {}", e)))?;

    let executed = Arc::new(AtomicUsize::new(0));
    let executed_clone = executed.clone();
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(SCRIPT_HOOK_GRANULARITY),
        move |_lua, _debug| {
            let total = executed_clone
                .fetch_add(SCRIPT_HOOK_GRANULARITY as usize, Ordering::Relaxed)
                + SCRIPT_HOOK_GRANULARITY as usize;
            if total > SCRIPT_MAX_INSTRUCTIONS {
                Err(LuaError::runtime(
                    "script exceeded the instruction limit and was terminated",
                ))
            } else {
                Ok(VmState::Continue)
            }
        },
    );

    let globals = lua.globals();
    for name in ["dofile", "loadfile", "collectgarbage"] {
        globals
            .set(name, LuaValue::Nil)
            .map_err(|e| TarnError::Script(format!("Failed to harden Lua globals: {}", e)))?;
    }

    Ok(lua)
}

/// Convert a Lua value to a serde_json::Value.
fn lua_value_to_json(v: LuaValue) -> serde_json::Value {
    match v {
        LuaValue::String(s) => serde_json::Value::String(s.to_string_lossy().to_string()),
        LuaValue::Integer(i) => serde_json::json!(i),
        LuaValue::Number(n) => serde_json::Number::from_f64(n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        LuaValue::Boolean(b) => serde_json::Value::Bool(b),
        LuaValue::Nil => serde_json::Value::Null,
        _ => serde_json::Value::String(format!("{:?}", v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_response(status: u16, body: serde_json::Value) -> HttpResponse {
        let body_bytes = match &body {
            serde_json::Value::Null => Vec::new(),
            serde_json::Value::String(text) => text.as_bytes().to_vec(),
            other => serde_json::to_vec(other).unwrap(),
        };
        HttpResponse {
            status,
            url: "https://example.com/".to_string(),
            redirect_count: 0,
            headers: HashMap::new(),
            raw_headers: vec![],
            body_bytes,
            body,
            duration_ms: 50,
            timings: crate::http::ResponseTimings {
                total_ms: 50,
                ttfb_ms: 25,
                body_read_ms: 25,
                connect_ms: None,
                tls_ms: None,
            },
        }
    }

    #[test]
    fn script_accesses_response_status() {
        let resp = make_response(200, json!({}));
        let result = run_script(
            "assert(response.status == 200, 'status ok')",
            &resp,
            &HashMap::new(),
            "test",
        )
        .unwrap();
        assert_eq!(result.assertion_results.len(), 1);
        assert!(result.assertion_results[0].passed);
    }

    #[test]
    fn script_accesses_response_body() {
        let resp = make_response(200, json!({"name": "Alice"}));
        let result = run_script(
            "assert(response.body.name == 'Alice', 'name check')",
            &resp,
            &HashMap::new(),
            "test",
        )
        .unwrap();
        assert!(result.assertion_results[0].passed);
    }

    #[test]
    fn script_sets_captures() {
        let resp = make_response(200, json!({"id": "usr_123"}));
        let result = run_script(
            "captures.user_id = response.body.id",
            &resp,
            &HashMap::new(),
            "test",
        )
        .unwrap();
        assert_eq!(result.captures.get("user_id").unwrap(), &json!("usr_123"));
    }

    #[test]
    fn script_sets_typed_captures() {
        let resp = make_response(200, json!({"count": 42}));
        let result = run_script(
            "captures.count = response.body.count",
            &resp,
            &HashMap::new(),
            "test",
        )
        .unwrap();
        assert_eq!(result.captures.get("count").unwrap(), &json!(42));
    }

    #[test]
    fn script_failed_assertion() {
        let resp = make_response(404, json!({}));
        let result = run_script(
            "assert(response.status == 200, 'expected 200')",
            &resp,
            &HashMap::new(),
            "test",
        )
        .unwrap();
        assert_eq!(result.assertion_results.len(), 1);
        assert!(!result.assertion_results[0].passed);
        assert!(result.assertion_results[0].message.contains("expected 200"));
    }

    #[test]
    fn script_syntax_error() {
        let resp = make_response(200, json!({}));
        let result = run_script("this is not valid lua!!!", &resp, &HashMap::new(), "test");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, TarnError::Script(_)));
    }

    #[test]
    fn script_reads_existing_captures() {
        let resp = make_response(200, json!({}));
        let mut caps = HashMap::new();
        caps.insert("token".to_string(), json!("abc123"));
        let result = run_script(
            "assert(captures.token == 'abc123', 'token check')",
            &resp,
            &caps,
            "test",
        )
        .unwrap();
        assert!(result.assertion_results[0].passed);
    }

    #[test]
    fn script_reads_typed_captures() {
        let resp = make_response(200, json!({}));
        let mut caps = HashMap::new();
        caps.insert("count".to_string(), json!(42));
        let result = run_script(
            "assert(captures.count == 42, 'number preserved')",
            &resp,
            &caps,
            "test",
        )
        .unwrap();
        assert!(result.assertion_results[0].passed);
    }

    #[test]
    fn script_cannot_access_os_library() {
        let resp = make_response(200, json!({}));
        let result = run_script(
            "assert(os == nil, 'os hidden')",
            &resp,
            &HashMap::new(),
            "test",
        )
        .unwrap();
        assert!(result.assertion_results[0].passed);
    }

    #[test]
    fn script_cannot_load_files() {
        let resp = make_response(200, json!({}));
        let result = run_script("dofile('secret.lua')", &resp, &HashMap::new(), "test");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("attempt to call a nil value"));
    }

    #[test]
    fn script_instruction_limit_is_enforced() {
        let resp = make_response(200, json!({}));
        let result = run_script("while true do end", &resp, &HashMap::new(), "test");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("instruction limit"));
    }

    #[test]
    fn script_json_decode() {
        let resp = make_response(200, json!({}));
        let result = run_script(
            r#"
            local data = json.decode('{"name":"Alice","age":30}')
            assert(data.name == 'Alice', 'name decoded')
            assert(data.age == 30, 'age decoded')
            "#,
            &resp,
            &HashMap::new(),
            "test",
        )
        .unwrap();
        assert_eq!(result.assertion_results.len(), 2);
        assert!(result.assertion_results.iter().all(|a| a.passed));
    }

    #[test]
    fn script_json_encode() {
        let resp = make_response(200, json!({}));
        let result = run_script(
            r#"
            local encoded = json.encode({name = 'Bob'})
            assert(type(encoded) == 'string', 'encode returns string')
            local decoded = json.decode(encoded)
            assert(decoded.name == 'Bob', 'roundtrip works')
            "#,
            &resp,
            &HashMap::new(),
            "test",
        )
        .unwrap();
        assert!(result.assertion_results.iter().all(|a| a.passed));
    }

    #[test]
    fn script_json_decode_invalid() {
        let resp = make_response(200, json!({}));
        let result = run_script(
            "json.decode('not valid json')",
            &resp,
            &HashMap::new(),
            "test",
        );
        assert!(result.is_err());
    }

    #[test]
    fn script_json_global_exists() {
        let resp = make_response(200, json!({}));
        let result = run_script(
            "assert(json ~= nil, 'json exists')\nassert(type(json.decode) == 'function', 'decode is function')\nassert(type(json.encode) == 'function', 'encode is function')",
            &resp,
            &HashMap::new(),
            "test",
        )
        .unwrap();
        assert!(result.assertion_results.iter().all(|a| a.passed));
    }
}
