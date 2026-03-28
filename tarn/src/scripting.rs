use crate::assert::types::AssertionResult;
use crate::error::TarnError;
use crate::http::HttpResponse;
use mlua::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Result of running a Lua script.
#[derive(Debug)]
pub struct ScriptResult {
    pub captures: HashMap<String, String>,
    pub assertion_results: Vec<AssertionResult>,
}

/// Execute a Lua script with access to the HTTP response and current captures.
pub fn run_script(
    script: &str,
    response: &HttpResponse,
    captures: &HashMap<String, String>,
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

    // Captures table
    let captures_table = lua
        .create_table()
        .map_err(|e| TarnError::Script(e.to_string()))?;
    for (k, v) in captures {
        captures_table
            .set(k.as_str(), v.as_str())
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

    // Extract modified captures
    let final_captures: HashMap<String, String> = {
        let captures_table: LuaTable = lua
            .globals()
            .get("captures")
            .map_err(|e| TarnError::Script(e.to_string()))?;
        let mut result = HashMap::new();
        for pair in captures_table.pairs::<String, LuaValue>() {
            let (k, v) = pair.map_err(|e| TarnError::Script(e.to_string()))?;
            let v_str = match v {
                LuaValue::String(s) => s.to_string_lossy().to_string(),
                LuaValue::Integer(i) => i.to_string(),
                LuaValue::Number(n) => n.to_string(),
                LuaValue::Boolean(b) => b.to_string(),
                LuaValue::Nil => "null".to_string(),
                _ => format!("{:?}", v),
            };
            result.insert(k, v_str);
        }
        result
    };

    let assertion_results = assertions.lock().unwrap().clone();

    Ok(ScriptResult {
        captures: final_captures,
        assertion_results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_response(status: u16, body: serde_json::Value) -> HttpResponse {
        HttpResponse {
            status,
            headers: HashMap::new(),
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
        assert_eq!(result.captures.get("user_id").unwrap(), "usr_123");
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
        caps.insert("token".to_string(), "abc123".to_string());
        let result = run_script(
            "assert(captures.token == 'abc123', 'token check')",
            &resp,
            &caps,
            "test",
        )
        .unwrap();
        assert!(result.assertion_results[0].passed);
    }
}
