use crate::error::TarnError;
use crate::model::{CaptureSpec, ExtendedCapture};
use regex::Regex;
use serde_json::Value;
use serde_json_path::JsonPath;
use std::collections::HashMap;

/// Extract captures from an HTTP response using JSONPath or header extraction.
/// Returns a map of capture_name -> extracted JSON value (type-preserving).
pub fn extract_captures(
    body: &Value,
    headers: &HashMap<String, String>,
    capture_map: &HashMap<String, CaptureSpec>,
) -> Result<HashMap<String, Value>, TarnError> {
    let mut captures = HashMap::new();

    for (name, spec) in capture_map {
        let value = match spec {
            CaptureSpec::JsonPath(path_str) => extract_jsonpath(body, path_str).map_err(|e| {
                TarnError::Capture(format!(
                    "Failed to capture '{}' with path '{}': {}",
                    name, path_str, e
                ))
            })?,
            CaptureSpec::Extended(ext) => extract_extended(body, headers, ext).map_err(|e| {
                TarnError::Capture(format!("Failed to capture '{}': {}", name, e))
            })?,
        };
        captures.insert(name.clone(), value);
    }

    Ok(captures)
}

/// Extract a value using an extended capture spec (header or JSONPath with optional regex).
fn extract_extended(
    body: &Value,
    headers: &HashMap<String, String>,
    ext: &ExtendedCapture,
) -> Result<Value, String> {
    // Determine the source string to work with
    let source = if let Some(ref header_name) = ext.header {
        // Case-insensitive header lookup
        let value = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(header_name))
            .map(|(_, v)| v.clone());
        match value {
            Some(v) => v,
            None => {
                let available: Vec<&str> = headers.keys().map(|k| k.as_str()).collect();
                return Err(format!(
                    "Header '{}' not found in response. Available: {}",
                    header_name,
                    if available.is_empty() {
                        "(none)".to_string()
                    } else {
                        available.join(", ")
                    }
                ));
            }
        }
    } else if let Some(ref jsonpath) = ext.jsonpath {
        // Extract via JSONPath, convert to string for regex processing
        let value = extract_jsonpath(body, jsonpath)?;
        value_to_string(&value)
    } else {
        return Err(
            "Extended capture must specify either 'header' or 'jsonpath' as the source".to_string(),
        );
    };

    // Apply regex if specified
    if let Some(ref regex_str) = ext.regex {
        let re = Regex::new(regex_str)
            .map_err(|e| format!("Invalid regex '{}': {}", regex_str, e))?;
        match re.captures(&source) {
            Some(caps) => {
                // Use capture group 1 if it exists, otherwise the full match
                let matched = caps
                    .get(1)
                    .or_else(|| caps.get(0))
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                Ok(Value::String(matched))
            }
            None => Err(format!(
                "Regex '{}' did not match value '{}'",
                regex_str, source
            )),
        }
    } else {
        // No regex — return the source string as-is
        Ok(Value::String(source))
    }
}

/// Extract a single value via JSONPath from a JSON body.
/// Returns the JSON value directly (type-preserving).
fn extract_jsonpath(body: &Value, path_str: &str) -> Result<Value, String> {
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

    // Take the first match — preserve the original type
    Ok(nodes[0].clone())
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

/// Convert a JSON value to a string representation.
pub fn value_to_string(value: &Value) -> String {
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
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("user_name".into(), CaptureSpec::JsonPath("$.name".into()));

        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert_eq!(captures.get("user_name").unwrap(), &json!("Alice"));
    }

    #[test]
    fn extract_number_field_preserves_type() {
        let body = json!({"age": 30});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("user_age".into(), CaptureSpec::JsonPath("$.age".into()));

        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert_eq!(captures.get("user_age").unwrap(), &json!(30));
    }

    #[test]
    fn extract_boolean_field_preserves_type() {
        let body = json!({"active": true});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("is_active".into(), CaptureSpec::JsonPath("$.active".into()));

        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert_eq!(captures.get("is_active").unwrap(), &json!(true));
    }

    #[test]
    fn extract_null_field() {
        let body = json!({"deleted": null});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("deleted".into(), CaptureSpec::JsonPath("$.deleted".into()));

        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert_eq!(captures.get("deleted").unwrap(), &json!(null));
    }

    #[test]
    fn extract_nested_field() {
        let body = json!({"user": {"profile": {"email": "alice@test.com"}}});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "email".into(),
            CaptureSpec::JsonPath("$.user.profile.email".into()),
        );

        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert_eq!(captures.get("email").unwrap(), &json!("alice@test.com"));
    }

    #[test]
    fn extract_array_element() {
        let body = json!({"items": [{"id": "first"}, {"id": "second"}]});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "first_id".into(),
            CaptureSpec::JsonPath("$.items[0].id".into()),
        );

        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert_eq!(captures.get("first_id").unwrap(), &json!("first"));
    }

    #[test]
    fn extract_missing_path_returns_error() {
        let body = json!({"name": "Alice"});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "missing".into(),
            CaptureSpec::JsonPath("$.nonexistent".into()),
        );

        let result = extract_captures(&body, &headers, &map);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("matched no values"));
    }

    #[test]
    fn extract_invalid_jsonpath_returns_error() {
        let body = json!({"name": "Alice"});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("bad".into(), CaptureSpec::JsonPath("$[invalid".into()));

        let result = extract_captures(&body, &headers, &map);
        assert!(result.is_err());
    }

    #[test]
    fn extract_multiple_captures() {
        let body = json!({"id": "usr_123", "token": "abc", "status": 200});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("id".into(), CaptureSpec::JsonPath("$.id".into()));
        map.insert("tok".into(), CaptureSpec::JsonPath("$.token".into()));
        map.insert("code".into(), CaptureSpec::JsonPath("$.status".into()));

        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert_eq!(captures.len(), 3);
        assert_eq!(captures.get("id").unwrap(), &json!("usr_123"));
        assert_eq!(captures.get("tok").unwrap(), &json!("abc"));
        assert_eq!(captures.get("code").unwrap(), &json!(200));
    }

    #[test]
    fn extract_array_value() {
        let body = json!({"tags": ["a", "b"]});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("tags".into(), CaptureSpec::JsonPath("$.tags".into()));

        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert_eq!(captures.get("tags").unwrap(), &json!(["a", "b"]));
    }

    #[test]
    fn value_to_string_object() {
        let val = json!({"key": "value"});
        assert_eq!(value_to_string(&val), "{\"key\":\"value\"}");
    }

    #[test]
    fn empty_capture_map() {
        let body = json!({"name": "Alice"});
        let headers = HashMap::new();
        let map = HashMap::new();
        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert!(captures.is_empty());
    }

    // --- Header capture tests ---

    #[test]
    fn capture_from_header() {
        let body = json!({});
        let mut headers = HashMap::new();
        headers.insert(
            "set-cookie".to_string(),
            "session=abc123; Path=/; HttpOnly".to_string(),
        );

        let mut map = HashMap::new();
        map.insert(
            "session".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: Some("set-cookie".to_string()),
                jsonpath: None,
                regex: Some("session=([^;]+)".to_string()),
            }),
        );

        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert_eq!(captures.get("session").unwrap(), &json!("abc123"));
    }

    #[test]
    fn capture_from_header_without_regex() {
        let body = json!({});
        let mut headers = HashMap::new();
        headers.insert("x-request-id".to_string(), "req-12345".to_string());

        let mut map = HashMap::new();
        map.insert(
            "req_id".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: Some("x-request-id".to_string()),
                jsonpath: None,
                regex: None,
            }),
        );

        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert_eq!(captures.get("req_id").unwrap(), &json!("req-12345"));
    }

    #[test]
    fn capture_from_header_case_insensitive() {
        let body = json!({});
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let mut map = HashMap::new();
        map.insert(
            "ct".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: Some("content-type".to_string()),
                jsonpath: None,
                regex: None,
            }),
        );

        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert_eq!(captures.get("ct").unwrap(), &json!("application/json"));
    }

    #[test]
    fn capture_from_missing_header_fails() {
        let body = json!({});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "missing".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: Some("x-nonexistent".to_string()),
                jsonpath: None,
                regex: None,
            }),
        );

        let result = extract_captures(&body, &headers, &map);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn capture_from_header_regex_no_match_fails() {
        let body = json!({});
        let mut headers = HashMap::new();
        headers.insert("set-cookie".to_string(), "other=value".to_string());

        let mut map = HashMap::new();
        map.insert(
            "session".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: Some("set-cookie".to_string()),
                jsonpath: None,
                regex: Some("session=([^;]+)".to_string()),
            }),
        );

        let result = extract_captures(&body, &headers, &map);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("did not match"));
    }

    #[test]
    fn capture_jsonpath_with_regex() {
        let body = json!({"message": "User created with ID: usr_42"});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "user_id".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: None,
                jsonpath: Some("$.message".to_string()),
                regex: Some("ID: (\\w+)".to_string()),
            }),
        );

        let captures = extract_captures(&body, &headers, &map).unwrap();
        assert_eq!(captures.get("user_id").unwrap(), &json!("usr_42"));
    }
}
