use crate::assert::types::AssertionResult;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value;
use serde_json_path::JsonPath;

/// Assert body fields via JSONPath expressions.
/// Each key is a JSONPath expression, value is the expected assertion.
pub fn assert_body(
    body: &Value,
    assertions: &IndexMap<String, serde_yaml::Value>,
) -> Vec<AssertionResult> {
    let mut results = Vec::new();

    for (path_str, expected) in assertions {
        let queried = query_jsonpath(body, path_str);

        match expected {
            // Simple equality: "$.field": "value" / 42 / true / null
            serde_yaml::Value::String(s) => {
                results.push(assert_eq_value(
                    path_str,
                    &queried,
                    &Value::String(s.clone()),
                ));
            }
            serde_yaml::Value::Number(n) => {
                let json_num = if let Some(i) = n.as_i64() {
                    Value::Number(serde_json::Number::from(i))
                } else if let Some(f) = n.as_f64() {
                    Value::Number(serde_json::Number::from_f64(f).unwrap())
                } else {
                    Value::Null
                };
                results.push(assert_eq_value(path_str, &queried, &json_num));
            }
            serde_yaml::Value::Bool(b) => {
                results.push(assert_eq_value(path_str, &queried, &Value::Bool(*b)));
            }
            serde_yaml::Value::Null => {
                results.push(assert_eq_value(path_str, &queried, &Value::Null));
            }
            // Operator map: "$.field": { type: string, contains: "sub", ... }
            serde_yaml::Value::Mapping(map) => {
                results.extend(assert_operator_map(path_str, &queried, map));
            }
            _ => {
                results.push(AssertionResult::fail(
                    format!("body {}", path_str),
                    "valid assertion",
                    format!("{:?}", expected),
                    format!("Unsupported assertion type for {}", path_str),
                ));
            }
        }
    }

    results
}

/// Query a JSONPath expression against a JSON body.
/// Returns the first matching value, or None if no match.
fn query_jsonpath(body: &Value, path_str: &str) -> Option<Value> {
    let json_path = match JsonPath::parse(path_str) {
        Ok(p) => p,
        Err(_) => return None,
    };

    let node_list = json_path.query(body);
    let nodes: Vec<&Value> = node_list.all();

    if nodes.is_empty() {
        None
    } else if nodes.len() == 1 {
        Some(nodes[0].clone())
    } else {
        // Multiple results: return as array
        Some(Value::Array(nodes.into_iter().cloned().collect()))
    }
}

/// Assert simple equality between JSONPath result and expected value.
fn assert_eq_value(path: &str, actual: &Option<Value>, expected: &Value) -> AssertionResult {
    let label = format!("body {}", path);

    match actual {
        None => AssertionResult::fail(
            &label,
            format_value(expected),
            "<path not found>",
            format!("JSONPath {} did not match any value", path),
        ),
        Some(actual_val) => {
            if values_equal(actual_val, expected) {
                AssertionResult::pass(&label, format_value(expected), format_value(actual_val))
            } else {
                AssertionResult::fail(
                    &label,
                    format_value(expected),
                    format_value(actual_val),
                    format!(
                        "JSONPath {}: expected {}, got {}",
                        path,
                        format_value(expected),
                        format_value(actual_val)
                    ),
                )
            }
        }
    }
}

/// Compare two JSON values for equality, handling number type coercion.
fn values_equal(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Number(a), Value::Number(e)) => {
            // Compare as f64 to handle integer/float mismatches
            a.as_f64() == e.as_f64()
        }
        _ => actual == expected,
    }
}

/// Process an operator map (e.g., { type: string, contains: "sub" }).
fn assert_operator_map(
    path: &str,
    actual: &Option<Value>,
    map: &serde_yaml::Mapping,
) -> Vec<AssertionResult> {
    let mut results = Vec::new();
    let label = format!("body {}", path);

    // Handle "exists" check first — it's special
    if let Some(exists_val) = map.get(serde_yaml::Value::String("exists".into())) {
        let should_exist = exists_val.as_bool().unwrap_or(true);
        let does_exist = actual.is_some();

        if should_exist == does_exist {
            results.push(AssertionResult::pass(
                &label,
                format!("exists: {}", should_exist),
                format!("exists: {}", does_exist),
            ));
        } else {
            results.push(AssertionResult::fail(
                &label,
                format!("exists: {}", should_exist),
                format!("exists: {}", does_exist),
                format!(
                    "JSONPath {}: expected field to {} exist",
                    path,
                    if should_exist { "" } else { "not " }
                ),
            ));
        }

        // If exists: false and field doesn't exist, skip other assertions
        if !should_exist && !does_exist {
            return results;
        }
    }

    // For all other operators, we need the actual value
    let actual_val = match actual {
        Some(v) => v,
        None => {
            // Only add error if we didn't already handle via exists
            if !map.contains_key(serde_yaml::Value::String("exists".into())) {
                results.push(AssertionResult::fail(
                    &label,
                    "value to exist",
                    "<path not found>",
                    format!("JSONPath {} did not match any value", path),
                ));
            }
            return results;
        }
    };

    for (key, val) in map {
        let op = match key.as_str() {
            Some(s) => s,
            None => continue,
        };

        match op {
            "exists" => {} // Already handled above
            "eq" => {
                let expected = yaml_to_json(val);
                if values_equal(actual_val, &expected) {
                    results.push(AssertionResult::pass(
                        &label,
                        format_value(&expected),
                        format_value(actual_val),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format_value(&expected),
                        format_value(actual_val),
                        format!(
                            "JSONPath {}: expected {}, got {}",
                            path,
                            format_value(&expected),
                            format_value(actual_val)
                        ),
                    ));
                }
            }
            "not_eq" => {
                let not_expected = yaml_to_json(val);
                if !values_equal(actual_val, &not_expected) {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("not {}", format_value(&not_expected)),
                        format_value(actual_val),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("not {}", format_value(&not_expected)),
                        format_value(actual_val),
                        format!(
                            "JSONPath {}: expected value to not equal {}",
                            path,
                            format_value(&not_expected)
                        ),
                    ));
                }
            }
            "type" => {
                let expected_type = val.as_str().unwrap_or("");
                let actual_type = json_type_name(actual_val);
                if actual_type == expected_type {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("type: {}", expected_type),
                        format!("type: {}", actual_type),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("type: {}", expected_type),
                        format!("type: {}", actual_type),
                        format!(
                            "JSONPath {}: expected type {}, got {}",
                            path, expected_type, actual_type
                        ),
                    ));
                }
            }
            "contains" => {
                let needle = yaml_to_json(val);
                let found = match actual_val {
                    Value::String(s) => {
                        if let Value::String(n) = &needle {
                            s.contains(n.as_str())
                        } else {
                            false
                        }
                    }
                    Value::Array(arr) => arr.contains(&needle),
                    _ => false,
                };
                if found {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("contains {}", format_value(&needle)),
                        format_value(actual_val),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("contains {}", format_value(&needle)),
                        format_value(actual_val),
                        format!(
                            "JSONPath {}: value does not contain {}",
                            path,
                            format_value(&needle)
                        ),
                    ));
                }
            }
            "not_contains" => {
                let needle = yaml_to_json(val);
                let found = match actual_val {
                    Value::String(s) => {
                        if let Value::String(n) = &needle {
                            s.contains(n.as_str())
                        } else {
                            false
                        }
                    }
                    Value::Array(arr) => arr.contains(&needle),
                    _ => false,
                };
                if !found {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("not contains {}", format_value(&needle)),
                        format_value(actual_val),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("not contains {}", format_value(&needle)),
                        format_value(actual_val),
                        format!(
                            "JSONPath {}: value should not contain {}",
                            path,
                            format_value(&needle)
                        ),
                    ));
                }
            }
            "starts_with" => {
                let prefix = val.as_str().unwrap_or("");
                let passes = actual_val
                    .as_str()
                    .map(|s| s.starts_with(prefix))
                    .unwrap_or(false);
                if passes {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("starts_with \"{}\"", prefix),
                        format_value(actual_val),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("starts_with \"{}\"", prefix),
                        format_value(actual_val),
                        format!(
                            "JSONPath {}: value does not start with \"{}\"",
                            path, prefix
                        ),
                    ));
                }
            }
            "ends_with" => {
                let suffix = val.as_str().unwrap_or("");
                let passes = actual_val
                    .as_str()
                    .map(|s| s.ends_with(suffix))
                    .unwrap_or(false);
                if passes {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("ends_with \"{}\"", suffix),
                        format_value(actual_val),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("ends_with \"{}\"", suffix),
                        format_value(actual_val),
                        format!("JSONPath {}: value does not end with \"{}\"", path, suffix),
                    ));
                }
            }
            "matches" => {
                let pattern = val.as_str().unwrap_or("");
                match Regex::new(pattern) {
                    Ok(re) => {
                        let actual_str = match actual_val {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        if re.is_match(&actual_str) {
                            results.push(AssertionResult::pass(
                                &label,
                                format!("matches \"{}\"", pattern),
                                format_value(actual_val),
                            ));
                        } else {
                            results.push(AssertionResult::fail(
                                &label,
                                format!("matches \"{}\"", pattern),
                                format_value(actual_val),
                                format!(
                                    "JSONPath {}: value does not match regex \"{}\"",
                                    path, pattern
                                ),
                            ));
                        }
                    }
                    Err(e) => {
                        results.push(AssertionResult::fail(
                            &label,
                            format!("matches \"{}\"", pattern),
                            format_value(actual_val),
                            format!("Invalid regex \"{}\": {}", pattern, e),
                        ));
                    }
                }
            }
            "not_empty" => {
                let should_not_be_empty = val.as_bool().unwrap_or(true);
                let is_empty = match actual_val {
                    Value::String(s) => s.is_empty(),
                    Value::Array(a) => a.is_empty(),
                    Value::Object(o) => o.is_empty(),
                    Value::Null => true,
                    _ => false,
                };
                let passes = if should_not_be_empty {
                    !is_empty
                } else {
                    is_empty
                };
                if passes {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("not_empty: {}", should_not_be_empty),
                        format_value(actual_val),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("not_empty: {}", should_not_be_empty),
                        format_value(actual_val),
                        format!(
                            "JSONPath {}: expected value to {} be empty",
                            path,
                            if should_not_be_empty { "not" } else { "" }
                        ),
                    ));
                }
            }
            "length" => {
                let expected_len = val.as_u64().unwrap_or(0) as usize;
                let actual_len = value_length(actual_val);
                if actual_len == Some(expected_len) {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("length: {}", expected_len),
                        format!("length: {}", expected_len),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("length: {}", expected_len),
                        format!(
                            "length: {}",
                            actual_len.map(|l| l.to_string()).unwrap_or("N/A".into())
                        ),
                        format!(
                            "JSONPath {}: expected length {}, got {:?}",
                            path, expected_len, actual_len
                        ),
                    ));
                }
            }
            "length_gt" => {
                let threshold = val.as_u64().unwrap_or(0) as usize;
                let actual_len = value_length(actual_val);
                let passes = actual_len.map(|l| l > threshold).unwrap_or(false);
                if passes {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("length > {}", threshold),
                        format!("length: {}", actual_len.unwrap()),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("length > {}", threshold),
                        format!(
                            "length: {}",
                            actual_len.map(|l| l.to_string()).unwrap_or("N/A".into())
                        ),
                        format!(
                            "JSONPath {}: expected length > {}, got {:?}",
                            path, threshold, actual_len
                        ),
                    ));
                }
            }
            "length_gte" => {
                let threshold = val.as_u64().unwrap_or(0) as usize;
                let actual_len = value_length(actual_val);
                let passes = actual_len.map(|l| l >= threshold).unwrap_or(false);
                if passes {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("length >= {}", threshold),
                        format!("length: {}", actual_len.unwrap()),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("length >= {}", threshold),
                        format!(
                            "length: {}",
                            actual_len.map(|l| l.to_string()).unwrap_or("N/A".into())
                        ),
                        format!(
                            "JSONPath {}: expected length >= {}, got {:?}",
                            path, threshold, actual_len
                        ),
                    ));
                }
            }
            "length_lte" => {
                let threshold = val.as_u64().unwrap_or(0) as usize;
                let actual_len = value_length(actual_val);
                let passes = actual_len.map(|l| l <= threshold).unwrap_or(false);
                if passes {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("length <= {}", threshold),
                        format!("length: {}", actual_len.unwrap()),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("length <= {}", threshold),
                        format!(
                            "length: {}",
                            actual_len.map(|l| l.to_string()).unwrap_or("N/A".into())
                        ),
                        format!(
                            "JSONPath {}: expected length <= {}, got {:?}",
                            path, threshold, actual_len
                        ),
                    ));
                }
            }
            "gt" => {
                let threshold = yaml_to_f64(val);
                let actual_num = actual_val.as_f64();
                let passes = actual_num
                    .zip(threshold)
                    .map(|(a, t)| a > t)
                    .unwrap_or(false);
                results.push(numeric_result(
                    &label, path, ">", threshold, actual_num, passes,
                ));
            }
            "gte" => {
                let threshold = yaml_to_f64(val);
                let actual_num = actual_val.as_f64();
                let passes = actual_num
                    .zip(threshold)
                    .map(|(a, t)| a >= t)
                    .unwrap_or(false);
                results.push(numeric_result(
                    &label, path, ">=", threshold, actual_num, passes,
                ));
            }
            "lt" => {
                let threshold = yaml_to_f64(val);
                let actual_num = actual_val.as_f64();
                let passes = actual_num
                    .zip(threshold)
                    .map(|(a, t)| a < t)
                    .unwrap_or(false);
                results.push(numeric_result(
                    &label, path, "<", threshold, actual_num, passes,
                ));
            }
            "lte" => {
                let threshold = yaml_to_f64(val);
                let actual_num = actual_val.as_f64();
                let passes = actual_num
                    .zip(threshold)
                    .map(|(a, t)| a <= t)
                    .unwrap_or(false);
                results.push(numeric_result(
                    &label, path, "<=", threshold, actual_num, passes,
                ));
            }
            other => {
                results.push(AssertionResult::fail(
                    &label,
                    "valid operator".to_string(),
                    other,
                    format!("Unknown assertion operator '{}' for {}", other, path),
                ));
            }
        }
    }

    results
}

fn numeric_result(
    label: &str,
    path: &str,
    op: &str,
    threshold: Option<f64>,
    actual: Option<f64>,
    passes: bool,
) -> AssertionResult {
    let expected_str = format!(
        "{} {}",
        op,
        threshold.map(|t| t.to_string()).unwrap_or("?".into())
    );
    let actual_str = actual
        .map(|a| a.to_string())
        .unwrap_or("non-numeric".into());

    if passes {
        AssertionResult::pass(label, &expected_str, &actual_str)
    } else {
        AssertionResult::fail(
            label,
            &expected_str,
            &actual_str,
            format!(
                "JSONPath {}: expected {} {}, got {}",
                path,
                op,
                threshold.map(|t| t.to_string()).unwrap_or("?".into()),
                actual_str
            ),
        )
    }
}

fn value_length(val: &Value) -> Option<usize> {
    match val {
        Value::String(s) => Some(s.len()),
        Value::Array(a) => Some(a.len()),
        Value::Object(o) => Some(o.len()),
        _ => None,
    }
}

fn json_type_name(val: &Value) -> &str {
    match val {
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Null => "null",
    }
}

fn yaml_to_json(val: &serde_yaml::Value) -> Value {
    match val {
        serde_yaml::Value::String(s) => Value::String(s.clone()),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Number(serde_json::Number::from(i))
            } else if let Some(f) = n.as_f64() {
                Value::Number(
                    serde_json::Number::from_f64(f).unwrap_or(serde_json::Number::from(0)),
                )
            } else {
                Value::Null
            }
        }
        serde_yaml::Value::Bool(b) => Value::Bool(*b),
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Sequence(seq) => Value::Array(seq.iter().map(yaml_to_json).collect()),
        serde_yaml::Value::Mapping(map) => {
            let obj: serde_json::Map<String, Value> = map
                .iter()
                .filter_map(|(k, v)| k.as_str().map(|s| (s.to_string(), yaml_to_json(v))))
                .collect();
            Value::Object(obj)
        }
        serde_yaml::Value::Tagged(t) => yaml_to_json(&t.value),
    }
}

fn yaml_to_f64(val: &serde_yaml::Value) -> Option<f64> {
    match val {
        serde_yaml::Value::Number(n) => n.as_f64(),
        serde_yaml::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn format_value(val: &Value) -> String {
    match val {
        Value::String(s) => format!("\"{}\"", s),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_assertions(yaml: &str) -> IndexMap<String, serde_yaml::Value> {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn run(body_json: &str, assertions_yaml: &str) -> Vec<AssertionResult> {
        let body: Value = serde_json::from_str(body_json).unwrap();
        let assertions = make_assertions(assertions_yaml);
        assert_body(&body, &assertions)
    }

    // --- Simple equality ---

    #[test]
    fn string_equality_pass() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": "Alice""#);
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    #[test]
    fn string_equality_fail() {
        let results = run(r#"{"name": "Bob"}"#, r#""$.name": "Alice""#);
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        assert!(results[0].message.contains("Alice"));
        assert!(results[0].message.contains("Bob"));
    }

    #[test]
    fn number_equality_pass() {
        let results = run(r#"{"age": 30}"#, r#""$.age": 30"#);
        assert!(results[0].passed);
    }

    #[test]
    fn number_equality_fail() {
        let results = run(r#"{"age": 25}"#, r#""$.age": 30"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn boolean_equality_pass() {
        let results = run(r#"{"active": true}"#, r#""$.active": true"#);
        assert!(results[0].passed);
    }

    #[test]
    fn boolean_equality_fail() {
        let results = run(r#"{"active": false}"#, r#""$.active": true"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn null_equality_pass() {
        let results = run(r#"{"deleted": null}"#, r#""$.deleted": null"#);
        assert!(results[0].passed);
    }

    #[test]
    fn null_equality_fail() {
        let results = run(r#"{"deleted": "2024-01-01"}"#, r#""$.deleted": null"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn path_not_found() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.missing": "value""#);
        assert!(!results[0].passed);
        assert!(results[0].message.contains("did not match"));
    }

    // --- Explicit eq operator ---

    #[test]
    fn explicit_eq_pass() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": { eq: "Alice" }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn explicit_eq_fail() {
        let results = run(r#"{"name": "Bob"}"#, r#""$.name": { eq: "Alice" }"#);
        assert!(!results[0].passed);
    }

    // --- not_eq ---

    #[test]
    fn not_eq_pass() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": { not_eq: "Bob" }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn not_eq_fail() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": { not_eq: "Alice" }"#);
        assert!(!results[0].passed);
    }

    // --- Type checks ---

    #[test]
    fn type_string_pass() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": { type: string }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn type_string_fail() {
        let results = run(r#"{"name": 42}"#, r#""$.name": { type: string }"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn type_number_pass() {
        let results = run(r#"{"age": 30}"#, r#""$.age": { type: number }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn type_boolean_pass() {
        let results = run(r#"{"active": true}"#, r#""$.active": { type: boolean }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn type_array_pass() {
        let results = run(r#"{"tags": ["a", "b"]}"#, r#""$.tags": { type: array }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn type_object_pass() {
        let results = run(
            r#"{"meta": {"key": "val"}}"#,
            r#""$.meta": { type: object }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn type_null_pass() {
        let results = run(r#"{"val": null}"#, "\"$.val\": { type: \"null\" }");
        assert!(results[0].passed);
    }

    // --- contains ---

    #[test]
    fn string_contains_pass() {
        let results = run(
            r#"{"email": "alice@example.com"}"#,
            r#""$.email": { contains: "@example" }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn string_contains_fail() {
        let results = run(
            r#"{"email": "alice@test.com"}"#,
            r#""$.email": { contains: "@example" }"#,
        );
        assert!(!results[0].passed);
    }

    #[test]
    fn array_contains_pass() {
        let results = run(
            r#"{"tags": ["a", "b", "c"]}"#,
            r#""$.tags": { contains: "b" }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn array_contains_fail() {
        let results = run(r#"{"tags": ["a", "b"]}"#, r#""$.tags": { contains: "z" }"#);
        assert!(!results[0].passed);
    }

    // --- not_contains ---

    #[test]
    fn not_contains_pass() {
        let results = run(
            r#"{"msg": "hello world"}"#,
            r#""$.msg": { not_contains: "error" }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn not_contains_fail() {
        let results = run(
            r#"{"msg": "error occurred"}"#,
            r#""$.msg": { not_contains: "error" }"#,
        );
        assert!(!results[0].passed);
    }

    // --- starts_with / ends_with ---

    #[test]
    fn starts_with_pass() {
        let results = run(
            r#"{"id": "usr_abc123"}"#,
            r#""$.id": { starts_with: "usr_" }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn starts_with_fail() {
        let results = run(
            r#"{"id": "org_abc123"}"#,
            r#""$.id": { starts_with: "usr_" }"#,
        );
        assert!(!results[0].passed);
    }

    #[test]
    fn ends_with_pass() {
        let results = run(
            r#"{"file": "report.pdf"}"#,
            r#""$.file": { ends_with: ".pdf" }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn ends_with_fail() {
        let results = run(
            r#"{"file": "report.doc"}"#,
            r#""$.file": { ends_with: ".pdf" }"#,
        );
        assert!(!results[0].passed);
    }

    // --- matches (regex) ---

    #[test]
    fn matches_pass() {
        let results = run(
            r#"{"id": "usr_abc123"}"#,
            r#""$.id": { matches: "^usr_[a-z0-9]+$" }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn matches_fail() {
        let results = run(
            r#"{"id": "USR_ABC"}"#,
            r#""$.id": { matches: "^usr_[a-z0-9]+$" }"#,
        );
        assert!(!results[0].passed);
    }

    #[test]
    fn matches_invalid_regex() {
        let results = run(r#"{"id": "test"}"#, r#""$.id": { matches: "[invalid" }"#);
        assert!(!results[0].passed);
        assert!(results[0].message.contains("Invalid regex"));
    }

    // --- not_empty ---

    #[test]
    fn not_empty_string_pass() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": { not_empty: true }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn not_empty_string_fail() {
        let results = run(r#"{"name": ""}"#, r#""$.name": { not_empty: true }"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn not_empty_array_pass() {
        let results = run(r#"{"items": [1, 2]}"#, r#""$.items": { not_empty: true }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn not_empty_array_fail() {
        let results = run(r#"{"items": []}"#, r#""$.items": { not_empty: true }"#);
        assert!(!results[0].passed);
    }

    // --- length ---

    #[test]
    fn length_exact_pass() {
        let results = run(r#"{"tags": ["a", "b", "c"]}"#, r#""$.tags": { length: 3 }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn length_exact_fail() {
        let results = run(r#"{"tags": ["a", "b"]}"#, r#""$.tags": { length: 3 }"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn length_string() {
        let results = run(r#"{"code": "abcde"}"#, r#""$.code": { length: 5 }"#);
        assert!(results[0].passed);
    }

    // --- length_gt, length_gte, length_lte ---

    #[test]
    fn length_gt_pass() {
        let results = run(r#"{"items": [1, 2, 3]}"#, r#""$.items": { length_gt: 2 }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn length_gt_fail() {
        let results = run(r#"{"items": [1, 2]}"#, r#""$.items": { length_gt: 2 }"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn length_gte_pass() {
        let results = run(r#"{"items": [1, 2]}"#, r#""$.items": { length_gte: 2 }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn length_gte_fail() {
        let results = run(r#"{"items": [1]}"#, r#""$.items": { length_gte: 2 }"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn length_lte_pass() {
        let results = run(r#"{"items": [1, 2]}"#, r#""$.items": { length_lte: 5 }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn length_lte_fail() {
        let results = run(
            r#"{"items": [1, 2, 3, 4, 5, 6]}"#,
            r#""$.items": { length_lte: 5 }"#,
        );
        assert!(!results[0].passed);
    }

    // --- Numeric comparisons ---

    #[test]
    fn gt_pass() {
        let results = run(r#"{"age": 25}"#, r#""$.age": { gt: 20 }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn gt_fail() {
        let results = run(r#"{"age": 20}"#, r#""$.age": { gt: 20 }"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn gte_pass_equal() {
        let results = run(r#"{"age": 20}"#, r#""$.age": { gte: 20 }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn gte_fail() {
        let results = run(r#"{"age": 19}"#, r#""$.age": { gte: 20 }"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn lt_pass() {
        let results = run(r#"{"age": 15}"#, r#""$.age": { lt: 20 }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn lt_fail() {
        let results = run(r#"{"age": 20}"#, r#""$.age": { lt: 20 }"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn lte_pass_equal() {
        let results = run(r#"{"age": 20}"#, r#""$.age": { lte: 20 }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn lte_fail() {
        let results = run(r#"{"age": 21}"#, r#""$.age": { lte: 20 }"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn numeric_on_non_number() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": { gt: 10 }"#);
        assert!(!results[0].passed);
    }

    // --- exists ---

    #[test]
    fn exists_true_present() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": { exists: true }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn exists_true_null_value() {
        let results = run(r#"{"deleted": null}"#, r#""$.deleted": { exists: true }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn exists_true_missing() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.missing": { exists: true }"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn exists_false_missing() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.missing": { exists: false }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn exists_false_present() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": { exists: false }"#);
        assert!(!results[0].passed);
    }

    // --- Combined assertions ---

    #[test]
    fn combined_assertions_all_pass() {
        let results = run(
            r#"{"id": "usr_abc123"}"#,
            r#""$.id": { type: string, not_empty: true, starts_with: "usr_" }"#,
        );
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.passed));
    }

    #[test]
    fn combined_assertions_partial_fail() {
        let results = run(
            r#"{"id": "org_abc"}"#,
            r#""$.id": { type: string, starts_with: "usr_" }"#,
        );
        assert_eq!(results.len(), 2);
        assert!(results[0].passed); // type: string
        assert!(!results[1].passed); // starts_with: "usr_"
    }

    // --- Nested JSONPath ---

    #[test]
    fn nested_jsonpath() {
        let results = run(
            r#"{"user": {"name": "Alice", "address": {"city": "NYC"}}}"#,
            r#""$.user.address.city": "NYC""#,
        );
        assert!(results[0].passed);
    }

    // --- Array index JSONPath ---

    #[test]
    fn array_index_jsonpath() {
        let results = run(
            r#"{"items": [{"name": "first"}, {"name": "second"}]}"#,
            r#""$.items[0].name": "first""#,
        );
        assert!(results[0].passed);
    }

    // --- Multiple assertions in one body block ---

    #[test]
    fn multiple_assertions() {
        let results = run(
            r#"{"name": "Alice", "age": 30, "active": true}"#,
            r#"
"$.name": "Alice"
"$.age": 30
"$.active": true
"#,
        );
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.passed));
    }

    // --- Unknown operator ---

    #[test]
    fn unknown_operator() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": { foobar: "test" }"#);
        assert!(!results[0].passed);
        assert!(results[0].message.contains("Unknown assertion operator"));
    }
}
