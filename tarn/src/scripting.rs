use crate::assert::types::AssertionResult;
use crate::error::TarnError;
use crate::http::HttpResponse;
use mlua::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
    let lua = Lua::new();

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
        HttpResponse {
            status,
            headers: HashMap::new(),
            raw_headers: vec![],
            body,
            duration_ms: 50,
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
}
