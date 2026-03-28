use crate::error::TarnError;
use serde_json::Value;
use serde_json_path::JsonPath;
use std::collections::HashMap;

/// Extract captures from a JSON response body using JSONPath expressions.
/// Returns a map of capture_name -> extracted_string_value.
pub fn extract_captures(
    body: &Value,
    capture_map: &HashMap<String, String>,
) -> Result<HashMap<String, String>, TarnError> {
    let mut captures = HashMap::new();

    for (name, path_str) in capture_map {
        let value = extract_jsonpath(body, path_str).map_err(|e| {
            TarnError::Capture(format!(
                "Failed to capture '{}' with path '{}': {}",
                name, path_str, e
            ))
        })?;
        captures.insert(name.clone(), value);
    }

    Ok(captures)
}

/// Extract a single value via JSONPath from a JSON body.
/// Returns the value as a string.
fn extract_jsonpath(body: &Value, path_str: &str) -> Result<String, String> {
    let json_path =
        JsonPath::parse(path_str).map_err(|e| format!("Invalid JSONPath '{}': {}", path_str, e))?;

    let node_list = json_path.query(body);
    let nodes: Vec<&Value> = node_list.all();

    if nodes.is_empty() {
        let hint = suggest_jsonpath_fix(body, path_str);
        return Err(format!(
            "JSONPath '{}' matched no values in response body{}",
            path_str, hint
        ));
    }

    // Take the first match
    let value = nodes[0];
    Ok(value_to_string(value))
}

/// Suggest fixes when a JSONPath doesn't match.
fn suggest_jsonpath_fix(body: &Value, path_str: &str) -> String {
    // Extract the first key from the path (e.g., "$.users" -> "users")
    let first_key = path_str
        .strip_prefix("$.")
        .and_then(|rest| rest.split('.').next())
        .and_then(|k| k.split('[').next());

    if let (Some(key), Some(obj)) = (first_key, body.as_object()) {
        let available: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        if available.is_empty() {
            return ". Response body is an empty object.".to_string();
        }

        // Check for close matches
        for avail_key in &available {
            if avail_key.eq_ignore_ascii_case(key) && *avail_key != key {
                return format!(". Did you mean `$.{}`? (case mismatch)", avail_key);
            }
        }

        // Show available keys (up to 10)
        let shown: Vec<&str> = available.iter().take(10).copied().collect();
        format!(". Available keys: {}", shown.join(", "))
    } else if body.is_array() {
        let len = body.as_array().map(|a| a.len()).unwrap_or(0);
        format!(
            ". Response body is an array with {} elements. Use $[0] to access elements.",
            len
        )
    } else {
        String::new()
    }
}

/// Convert a JSON value to a string for use as a captured variable.
fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        // Arrays and objects are serialized as JSON strings
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_string_field() {
        let body = json!({"name": "Alice"});
        let mut map = HashMap::new();
        map.insert("user_name".into(), "$.name".into());

        let captures = extract_captures(&body, &map).unwrap();
        assert_eq!(captures.get("user_name").unwrap(), "Alice");
    }

    #[test]
    fn extract_number_field() {
        let body = json!({"age": 30});
        let mut map = HashMap::new();
        map.insert("user_age".into(), "$.age".into());

        let captures = extract_captures(&body, &map).unwrap();
        assert_eq!(captures.get("user_age").unwrap(), "30");
    }

    #[test]
    fn extract_boolean_field() {
        let body = json!({"active": true});
        let mut map = HashMap::new();
        map.insert("is_active".into(), "$.active".into());

        let captures = extract_captures(&body, &map).unwrap();
        assert_eq!(captures.get("is_active").unwrap(), "true");
    }

    #[test]
    fn extract_null_field() {
        let body = json!({"deleted": null});
        let mut map = HashMap::new();
        map.insert("deleted".into(), "$.deleted".into());

        let captures = extract_captures(&body, &map).unwrap();
        assert_eq!(captures.get("deleted").unwrap(), "null");
    }

    #[test]
    fn extract_nested_field() {
        let body = json!({"user": {"profile": {"email": "alice@test.com"}}});
        let mut map = HashMap::new();
        map.insert("email".into(), "$.user.profile.email".into());

        let captures = extract_captures(&body, &map).unwrap();
        assert_eq!(captures.get("email").unwrap(), "alice@test.com");
    }

    #[test]
    fn extract_array_element() {
        let body = json!({"items": [{"id": "first"}, {"id": "second"}]});
        let mut map = HashMap::new();
        map.insert("first_id".into(), "$.items[0].id".into());

        let captures = extract_captures(&body, &map).unwrap();
        assert_eq!(captures.get("first_id").unwrap(), "first");
    }

    #[test]
    fn extract_missing_path_returns_error() {
        let body = json!({"name": "Alice"});
        let mut map = HashMap::new();
        map.insert("missing".into(), "$.nonexistent".into());

        let result = extract_captures(&body, &map);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("matched no values"));
    }

    #[test]
    fn extract_invalid_jsonpath_returns_error() {
        let body = json!({"name": "Alice"});
        let mut map = HashMap::new();
        map.insert("bad".into(), "$[invalid".into());

        let result = extract_captures(&body, &map);
        assert!(result.is_err());
    }

    #[test]
    fn extract_multiple_captures() {
        let body = json!({"id": "usr_123", "token": "abc", "status": 200});
        let mut map = HashMap::new();
        map.insert("id".into(), "$.id".into());
        map.insert("tok".into(), "$.token".into());
        map.insert("code".into(), "$.status".into());

        let captures = extract_captures(&body, &map).unwrap();
        assert_eq!(captures.len(), 3);
        assert_eq!(captures.get("id").unwrap(), "usr_123");
        assert_eq!(captures.get("tok").unwrap(), "abc");
        assert_eq!(captures.get("code").unwrap(), "200");
    }

    #[test]
    fn extract_array_value() {
        let body = json!({"tags": ["a", "b"]});
        let mut map = HashMap::new();
        map.insert("tags".into(), "$.tags".into());

        let captures = extract_captures(&body, &map).unwrap();
        // Array serialized as JSON string
        assert_eq!(captures.get("tags").unwrap(), "[\"a\",\"b\"]");
    }

    #[test]
    fn value_to_string_object() {
        let val = json!({"key": "value"});
        assert_eq!(value_to_string(&val), "{\"key\":\"value\"}");
    }

    #[test]
    fn empty_capture_map() {
        let body = json!({"name": "Alice"});
        let map = HashMap::new();
        let captures = extract_captures(&body, &map).unwrap();
        assert!(captures.is_empty());
    }
}
