use crate::assert::types::AssertionResult;
use crate::regex_cache;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use indexmap::IndexMap;
use md5::compute as md5_compute;
use serde_json::Value;
use serde_json_path::JsonPath;
use sha2::{Digest, Sha256};
use similar::TextDiff;
use std::net::{Ipv4Addr, Ipv6Addr};
use uuid::Uuid;

pub(crate) const ASSERTION_OPERATORS: &[&str] = &[
    "exists",
    "eq",
    "not_eq",
    "type",
    "contains",
    "not_contains",
    "starts_with",
    "ends_with",
    "matches",
    "is_uuid",
    "is_date",
    "is_ipv4",
    "is_ipv6",
    "empty",
    "is_empty",
    "not_empty",
    "bytes",
    "sha256",
    "md5",
    "length",
    "length_gt",
    "length_gte",
    "length_lte",
    "gt",
    "gte",
    "lt",
    "lte",
    // Identifier-based array primitives (NAZ-341). These let tests
    // assert "the list contains *a record with these fields*" without
    // committing to a specific array index or exact length, which is
    // the brittleness pattern that dominated the EQHUB investigation.
    "exists_where",
    "not_exists_where",
    "contains_object",
];

/// Assert body fields via JSONPath expressions.
/// Each key is a JSONPath expression, value is the expected assertion.
pub fn assert_body(
    body: &Value,
    body_bytes: &[u8],
    assertions: &IndexMap<String, serde_yaml::Value>,
) -> Vec<AssertionResult> {
    let mut results = Vec::new();

    for (path_str, expected) in assertions {
        let queried = query_jsonpath(body, path_str);

        match expected {
            // Operator map: "$.field": { type: string, contains: "sub", ... }
            // Operator map: "$.field": { type: string, contains: "sub", ... }
            serde_yaml::Value::Mapping(map) => {
                if path_str != "$" || is_operator_map(map) {
                    results.extend(assert_operator_map(path_str, &queried, body_bytes, map));
                } else {
                    results.push(assert_eq_value(path_str, &queried, &yaml_to_json(expected)));
                }
            }
            _ => {
                results.push(assert_eq_value(path_str, &queried, &yaml_to_json(expected)));
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
                equality_failure(&label, path, expected, actual_val)
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
    body_bytes: &[u8],
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
                        equality_failure_message(path, &expected, actual_val),
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
                match regex_cache::get(pattern) {
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
            "is_uuid" => {
                results.push(format_check_result(
                    &label,
                    path,
                    "is_uuid",
                    val.as_bool().unwrap_or(true),
                    actual_val,
                    is_uuid_string,
                    "valid UUID",
                ));
            }
            "is_date" => {
                results.push(format_check_result(
                    &label,
                    path,
                    "is_date",
                    val.as_bool().unwrap_or(true),
                    actual_val,
                    is_date_string,
                    "valid date",
                ));
            }
            "is_ipv4" => {
                results.push(format_check_result(
                    &label,
                    path,
                    "is_ipv4",
                    val.as_bool().unwrap_or(true),
                    actual_val,
                    is_ipv4_string,
                    "valid IPv4 address",
                ));
            }
            "is_ipv6" => {
                results.push(format_check_result(
                    &label,
                    path,
                    "is_ipv6",
                    val.as_bool().unwrap_or(true),
                    actual_val,
                    is_ipv6_string,
                    "valid IPv6 address",
                ));
            }
            "empty" | "is_empty" => {
                let should_be_empty = val.as_bool().unwrap_or(true);
                let is_empty = is_empty_value(actual_val);
                let passes = if should_be_empty { is_empty } else { !is_empty };
                if passes {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("{op}: {}", should_be_empty),
                        format_value(actual_val),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("{op}: {}", should_be_empty),
                        format_value(actual_val),
                        format!(
                            "JSONPath {}: expected value to {}be empty",
                            path,
                            if should_be_empty { "" } else { "not " }
                        ),
                    ));
                }
            }
            "not_empty" => {
                let should_not_be_empty = val.as_bool().unwrap_or(true);
                let is_empty = is_empty_value(actual_val);
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
            "bytes" => {
                let expected_len = val.as_u64().unwrap_or(0) as usize;
                let actual_len = actual_bytes(path, actual_val, body_bytes).len();
                if actual_len == expected_len {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("bytes: {}", expected_len),
                        format!("bytes: {}", actual_len),
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("bytes: {}", expected_len),
                        format!("bytes: {}", actual_len),
                        format!(
                            "JSONPath {}: expected {} bytes, got {}",
                            path, expected_len, actual_len
                        ),
                    ));
                }
            }
            "sha256" => {
                let expected = val.as_str().unwrap_or("").to_ascii_lowercase();
                let actual_hash =
                    hex_encode(&sha256_digest(&actual_bytes(path, actual_val, body_bytes)));
                if actual_hash == expected {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("sha256: {}", expected),
                        actual_hash,
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("sha256: {}", expected),
                        actual_hash.clone(),
                        format!(
                            "JSONPath {}: expected sha256 {}, got {}",
                            path, expected, actual_hash
                        ),
                    ));
                }
            }
            "md5" => {
                let expected = val.as_str().unwrap_or("").to_ascii_lowercase();
                let actual_hash = md5_hex(&actual_bytes(path, actual_val, body_bytes));
                if actual_hash == expected {
                    results.push(AssertionResult::pass(
                        &label,
                        format!("md5: {}", expected),
                        actual_hash,
                    ));
                } else {
                    results.push(AssertionResult::fail(
                        &label,
                        format!("md5: {}", expected),
                        actual_hash.clone(),
                        format!(
                            "JSONPath {}: expected md5 {}, got {}",
                            path, expected, actual_hash
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
            "exists_where" | "contains_object" => {
                results.push(assert_predicate_in_array(
                    &label, path, actual_val, val, /* should_exist */ true,
                ));
            }
            "not_exists_where" => {
                results.push(assert_predicate_in_array(
                    &label, path, actual_val, val, /* should_exist */ false,
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

/// `exists_where` / `not_exists_where` / `contains_object` assertion.
///
/// `should_exist = true` passes when at least one element of the array
/// matches the predicate; `should_exist = false` passes when no element
/// matches. The predicate is the YAML value the user wrote — a mapping
/// of `{ field: expected }` pairs where `expected` is either a scalar
/// (exact match) or a nested operator map (e.g. `{ matches: "^usr_" }`).
fn assert_predicate_in_array(
    label: &str,
    path: &str,
    actual_val: &Value,
    predicate: &serde_yaml::Value,
    should_exist: bool,
) -> AssertionResult {
    let predicate_map = match predicate {
        serde_yaml::Value::Mapping(m) => m,
        _ => {
            return AssertionResult::fail(
                label,
                "predicate mapping".to_string(),
                format!("{:?}", predicate),
                format!(
                    "JSONPath {}: exists_where/contains_object expects an object predicate \
                     (e.g. `exists_where: {{ id: \"...\", name: \"...\" }}`)",
                    path
                ),
            );
        }
    };

    let items = match actual_val {
        Value::Array(a) => a.as_slice(),
        _ => {
            return AssertionResult::fail(
                label,
                "array at path".to_string(),
                json_type_name(actual_val).to_string(),
                format!(
                    "JSONPath {}: exists_where/contains_object requires an array at the path, got {}",
                    path,
                    json_type_name(actual_val),
                ),
            );
        }
    };

    let matches = items
        .iter()
        .any(|item| object_matches_predicate(item, predicate_map));

    let op_label = if should_exist {
        "exists_where"
    } else {
        "not_exists_where"
    };
    let predicate_str = predicate_summary(predicate_map);

    if should_exist == matches {
        AssertionResult::pass(
            label,
            format!("{} {}", op_label, predicate_str),
            format!("{} elements scanned", items.len()),
        )
    } else if should_exist {
        AssertionResult::fail(
            label,
            format!("{} {}", op_label, predicate_str),
            format!("no match in {} elements", items.len()),
            format!(
                "JSONPath {}: expected at least one object matching {}, none of the {} items did. \
                 Consider asserting by a stable identifier — array position and exact length are \
                 brittle on shared endpoints.",
                path,
                predicate_str,
                items.len()
            ),
        )
    } else {
        let first_match = items
            .iter()
            .find(|item| object_matches_predicate(item, predicate_map))
            .map(format_value)
            .unwrap_or_default();
        AssertionResult::fail(
            label,
            format!("{} {}", op_label, predicate_str),
            format!("match found: {}", first_match),
            format!(
                "JSONPath {}: expected no object matching {}, but found {}",
                path, predicate_str, first_match
            ),
        )
    }
}

/// Render a human-readable summary of an `{ field: expected }` predicate —
/// used in assertion messages so the operator can see what was being
/// matched without having to re-read the YAML.
fn predicate_summary(map: &serde_yaml::Mapping) -> String {
    let mut parts = Vec::with_capacity(map.len());
    for (k, v) in map {
        let key_str = k.as_str().unwrap_or("<?>").to_string();
        let value_str = match v {
            serde_yaml::Value::Mapping(_) => "<operator map>".to_string(),
            other => format_value(&yaml_to_json(other)),
        };
        parts.push(format!("{}: {}", key_str, value_str));
    }
    format!("{{ {} }}", parts.join(", "))
}

/// Check whether `item` (expected to be a JSON object) satisfies every
/// key/value pair in `predicate`. Scalar predicate values match by
/// equality; mapping predicate values are treated as operator maps and
/// dispatched through [`assert_operator_map`] — this is what lets users
/// write `exists_where: { id: "x", role: { matches: "^admin$" } }`
/// without inventing a second mini-language.
pub(crate) fn object_matches_predicate(item: &Value, predicate: &serde_yaml::Mapping) -> bool {
    let obj = match item {
        Value::Object(o) => o,
        // A predicate can only match an object — any array/scalar in the
        // input is an automatic miss.
        _ => return false,
    };

    for (key, expected) in predicate {
        let Some(field) = key.as_str() else {
            return false;
        };
        let actual = match obj.get(field) {
            Some(v) => v.clone(),
            None => return false,
        };

        match expected {
            serde_yaml::Value::Mapping(inner) => {
                // Inner operator map: reuse the existing assertion
                // dispatch by rendering a single-field assertion and
                // checking it passed. Going through the same code path
                // keeps behaviour consistent (e.g. numeric coercion
                // rules in `eq`, regex caching in `matches`).
                let sub_results =
                    assert_operator_map(&format!("predicate.{field}"), &Some(actual), &[], inner);
                if sub_results.iter().any(|r| !r.passed) {
                    return false;
                }
            }
            _ => {
                if !values_equal(&actual, &yaml_to_json(expected)) {
                    return false;
                }
            }
        }
    }

    true
}

fn equality_failure(label: &str, path: &str, expected: &Value, actual: &Value) -> AssertionResult {
    if let Some(diff) = whole_body_diff(path, expected, actual) {
        AssertionResult::fail_with_diff(
            label,
            format_value(expected),
            format_value(actual),
            equality_failure_message(path, expected, actual),
            diff,
        )
    } else {
        AssertionResult::fail(
            label,
            format_value(expected),
            format_value(actual),
            equality_failure_message(path, expected, actual),
        )
    }
}

fn equality_failure_message(path: &str, expected: &Value, actual: &Value) -> String {
    format!(
        "JSONPath {}: expected {}, got {}",
        path,
        format_value(expected),
        format_value(actual)
    )
}

fn whole_body_diff(path: &str, expected: &Value, actual: &Value) -> Option<String> {
    if path != "$" {
        return None;
    }

    let expected_text = diff_repr(expected);
    let actual_text = diff_repr(actual);
    if expected_text == actual_text {
        return None;
    }

    let diff = TextDiff::from_lines(&expected_text, &actual_text);
    let mut rendered = String::new();

    rendered.push_str("--- expected\n");
    rendered.push_str("+++ actual\n");
    for change in diff.iter_all_changes() {
        let prefix = match change.tag() {
            similar::ChangeTag::Delete => "-",
            similar::ChangeTag::Insert => "+",
            similar::ChangeTag::Equal => " ",
        };
        rendered.push_str(prefix);
        rendered.push_str(change.value());
        if !change.value().ends_with('\n') {
            rendered.push('\n');
        }
    }

    Some(rendered)
}

fn diff_repr(value: &Value) -> String {
    match value {
        Value::String(text) => ensure_trailing_newline(text.clone()),
        _ => ensure_trailing_newline(
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        ),
    }
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn is_operator_map(map: &serde_yaml::Mapping) -> bool {
    !map.is_empty()
        && map.keys().all(|key| {
            key.as_str()
                .is_some_and(|s| ASSERTION_OPERATORS.contains(&s))
        })
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

fn format_check_result(
    label: &str,
    path: &str,
    op: &str,
    should_match: bool,
    actual: &Value,
    predicate: impl Fn(&str) -> bool,
    human_name: &str,
) -> AssertionResult {
    let actual_str = match actual {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let is_valid = predicate(&actual_str);
    let passes = if should_match { is_valid } else { !is_valid };
    let expected = format!("{op}: {should_match}");

    if passes {
        AssertionResult::pass(label, expected, format_value(actual))
    } else {
        AssertionResult::fail(
            label,
            format!("{op}: {should_match}"),
            format_value(actual),
            format!(
                "JSONPath {}: expected value to {}be a {}, got {}",
                path,
                if should_match { "" } else { "not " },
                human_name,
                format_value(actual)
            ),
        )
    }
}

fn is_empty_value(value: &Value) -> bool {
    match value {
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::Null => true,
        _ => false,
    }
}

fn actual_bytes(path: &str, actual: &Value, body_bytes: &[u8]) -> Vec<u8> {
    if path == "$" {
        return body_bytes.to_vec();
    }

    match actual {
        Value::String(text) => text.as_bytes().to_vec(),
        other => serde_json::to_vec(other).unwrap_or_else(|_| other.to_string().into_bytes()),
    }
}

fn sha256_digest(bytes: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().to_vec()
}

fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", md5_compute(bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
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

fn is_uuid_string(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}

fn is_date_string(value: &str) -> bool {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
        || DateTime::parse_from_rfc3339(value).is_ok()
        || NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").is_ok()
        || NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f").is_ok()
        || NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").is_ok()
        || NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f").is_ok()
        || value
            .parse::<DateTime<Utc>>()
            .map(|_| true)
            .unwrap_or(false)
}

fn is_ipv4_string(value: &str) -> bool {
    value.parse::<Ipv4Addr>().is_ok()
}

fn is_ipv6_string(value: &str) -> bool {
    value.parse::<Ipv6Addr>().is_ok()
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
        assert_body(&body, body_json.as_bytes(), &assertions)
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

    #[test]
    fn wildcard_contains_object_array() {
        // $[*].field extracts field values from all array elements, then contains checks the result
        let body = r#"[
            {"materialData": {"partDescription": "Bolt"}},
            {"materialData": {"partDescription": "Sync Test Bearing"}},
            {"materialData": {"partDescription": "Nut"}}
        ]"#;
        let results = run(
            body,
            r#""$[*].materialData.partDescription": { contains: "Sync Test Bearing" }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn wildcard_contains_object_array_miss() {
        let body = r#"[
            {"materialData": {"partDescription": "Bolt"}},
            {"materialData": {"partDescription": "Nut"}}
        ]"#;
        let results = run(
            body,
            r#""$[*].materialData.partDescription": { contains: "Sync Test Bearing" }"#,
        );
        assert!(!results[0].passed);
    }

    #[test]
    fn filter_expression_matches_object() {
        let body = r#"[
            {"name": "Alice", "role": "admin"},
            {"name": "Bob", "role": "user"}
        ]"#;
        let results = run(body, r#""$[?@.name == 'Alice']": { exists: true }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn filter_expression_no_match() {
        let body = r#"[{"name": "Alice"}, {"name": "Bob"}]"#;
        let results = run(body, r#""$[?@.name == 'Charlie']": { exists: true }"#);
        assert!(!results[0].passed);
    }

    #[test]
    fn filter_expression_old_syntax_with_parens() {
        let body = r#"[{"name": "Alice"}, {"name": "Bob"}]"#;
        let results = run(body, r#""$[?(@.name == 'Alice')]": { exists: true }"#);
        assert!(results[0].passed);
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

    // --- format assertions ---

    #[test]
    fn is_uuid_pass() {
        let results = run(
            r#"{"id": "550e8400-e29b-41d4-a716-446655440000"}"#,
            r#""$.id": { is_uuid: true }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn is_uuid_fail() {
        let results = run(r#"{"id": "not-a-uuid"}"#, r#""$.id": { is_uuid: true }"#);
        assert!(!results[0].passed);
        assert!(results[0].message.contains("valid UUID"));
    }

    #[test]
    fn is_date_passes_for_plain_date_and_rfc3339() {
        let plain = run(
            r#"{"created_at": "2026-04-01"}"#,
            r#""$.created_at": { is_date: true }"#,
        );
        let datetime = run(
            r#"{"created_at": "2026-04-01T12:34:56Z"}"#,
            r#""$.created_at": { is_date: true }"#,
        );
        assert!(plain[0].passed);
        assert!(datetime[0].passed);
    }

    #[test]
    fn is_date_fail() {
        let results = run(
            r#"{"created_at": "01/04/2026"}"#,
            r#""$.created_at": { is_date: true }"#,
        );
        assert!(!results[0].passed);
        assert!(results[0].message.contains("valid date"));
    }

    #[test]
    fn is_ipv4_pass() {
        let results = run(r#"{"ip": "192.168.1.10"}"#, r#""$.ip": { is_ipv4: true }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn is_ipv4_fail() {
        let results = run(r#"{"ip": "2001:db8::1"}"#, r#""$.ip": { is_ipv4: true }"#);
        assert!(!results[0].passed);
        assert!(results[0].message.contains("valid IPv4 address"));
    }

    #[test]
    fn is_ipv6_pass() {
        let results = run(r#"{"ip": "2001:db8::1"}"#, r#""$.ip": { is_ipv6: true }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn is_ipv6_fail() {
        let results = run(r#"{"ip": "192.168.1.10"}"#, r#""$.ip": { is_ipv6: true }"#);
        assert!(!results[0].passed);
        assert!(results[0].message.contains("valid IPv6 address"));
    }

    // --- empty / is_empty ---

    #[test]
    fn empty_string_pass() {
        let results = run(r#"{"name": ""}"#, r#""$.name": { empty: true }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn empty_array_pass() {
        let results = run(r#"{"items": []}"#, r#""$.items": { empty: true }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn is_empty_null_pass() {
        let results = run(r#"{"deleted": null}"#, r#""$.deleted": { is_empty: true }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn empty_fail_for_non_empty_string() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": { empty: true }"#);
        assert!(!results[0].passed);
        assert!(results[0].message.contains("be empty"));
    }

    #[test]
    fn is_empty_false_passes_for_non_empty_value() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": { is_empty: false }"#);
        assert!(results[0].passed);
    }

    // --- bytes / sha256 / md5 ---

    #[test]
    fn bytes_pass_for_whole_raw_body() {
        let results = run(r#"{"msg":"hello"}"#, r#""$": { bytes: 15 }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn bytes_pass_for_nested_string_value() {
        let results = run(r#"{"msg":"hello"}"#, r#""$.msg": { bytes: 5 }"#);
        assert!(results[0].passed);
    }

    #[test]
    fn sha256_pass_for_nested_string_value() {
        let results = run(
            r#"{"msg":"hello"}"#,
            r#""$.msg": { sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824" }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn sha256_pass_for_whole_raw_body() {
        let results = run(
            r#"{"msg":"hello"}"#,
            r#""$": { sha256: "faf0237414bb4de6d09919f02006843e237179c7a3a866d6cc77e967688d6e02" }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn md5_pass_for_nested_string_value() {
        let results = run(
            r#"{"msg":"hello"}"#,
            r#""$.msg": { md5: "5d41402abc4b2a76b9719d911017c592" }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn md5_pass_for_whole_raw_body() {
        let results = run(
            r#"{"msg":"hello"}"#,
            r#""$": { md5: "698a83374eba0350063b0e2777e4ce85" }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn sha256_fail_reports_actual_hash() {
        let results = run(r#"{"msg":"hello"}"#, r#""$.msg": { sha256: "deadbeef" }"#);
        assert!(!results[0].passed);
        assert!(results[0]
            .message
            .contains("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"));
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

    #[test]
    fn whole_body_text_diff_is_included() {
        let results = run(r#""hello\nworld""#, r#""$": "hello\nthere""#);
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        let diff = results[0].diff.as_deref().unwrap();
        assert!(diff.contains("--- expected"));
        assert!(diff.contains("+++ actual"));
        assert!(diff.contains(" hello"));
        assert!(diff.contains("-there"));
        assert!(diff.contains("+world"));
    }

    #[test]
    fn whole_body_json_literal_passes() {
        let results = run(
            r#"{"name":"Alice","roles":["admin"]}"#,
            r#"
"$":
  name: Alice
  roles:
    - admin
"#,
        );
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    #[test]
    fn whole_body_json_literal_diff_is_included() {
        let results = run(
            r#"{"name":"Alice","roles":["admin"]}"#,
            r#"
"$":
  name: Bob
  roles:
    - admin
"#,
        );
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        let diff = results[0].diff.as_deref().unwrap();
        assert!(diff.contains("--- expected"));
        assert!(diff.contains("+++ actual"));
        assert!(diff.contains("-  \"name\": \"Bob\""));
        assert!(diff.contains("+  \"name\": \"Alice\""));
    }

    #[test]
    fn root_operator_map_still_works() {
        let results = run(r#"{"name":"Alice"}"#, r#""$": { type: object }"#);
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    // --- Unknown operator ---

    #[test]
    fn unknown_operator() {
        let results = run(r#"{"name": "Alice"}"#, r#""$.name": { foobar: "test" }"#);
        assert!(!results[0].passed);
        assert!(results[0].message.contains("Unknown assertion operator"));
    }

    // --- Array predicate operators (exists_where/not_exists_where/contains_object) ---

    #[test]
    fn exists_where_matches_by_identifier_fields() {
        let results = run(
            r#"{"users": [{"id": "a", "role": "user"}, {"id": "b", "role": "admin"}]}"#,
            r#""$.users": { exists_where: { id: "b", role: "admin" } }"#,
        );
        assert!(results[0].passed, "{:?}", results[0]);
    }

    #[test]
    fn exists_where_reports_the_other_items_on_miss() {
        // This is the EQHUB-style failure: the list is long, the test
        // wanted a specific record, and the diagnostic must say what's
        // missing, not just "assertion failed".
        let results = run(
            r#"{"users": [{"id": "a"}, {"id": "b"}, {"id": "c"}]}"#,
            r#""$.users": { exists_where: { id: "missing" } }"#,
        );
        assert!(!results[0].passed);
        assert!(
            results[0].message.contains("3 items")
                && results[0].message.contains("id: \"missing\""),
            "expected message to reference 3 scanned items and the predicate, got {:?}",
            results[0].message
        );
    }

    #[test]
    fn not_exists_where_rejects_forbidden_object() {
        let passed = run(
            r#"{"users": [{"id": "a"}, {"id": "b"}]}"#,
            r#""$.users": { not_exists_where: { id: "c" } }"#,
        );
        assert!(passed[0].passed);

        let failed = run(
            r#"{"users": [{"id": "a"}, {"id": "b"}]}"#,
            r#""$.users": { not_exists_where: { id: "a" } }"#,
        );
        assert!(!failed[0].passed);
    }

    #[test]
    fn contains_object_is_alias_for_exists_where() {
        let results = run(
            r#"{"items": [{"sku": "A1"}, {"sku": "B2"}]}"#,
            r#""$.items": { contains_object: { sku: "A1" } }"#,
        );
        assert!(results[0].passed);
    }

    #[test]
    fn exists_where_supports_nested_operator_maps_in_predicate() {
        let results = run(
            r#"{"users": [{"id": "u_1", "email": "a@b.com"}]}"#,
            r#""$.users": { exists_where: { email: { matches: ".+@.+" } } }"#,
        );
        assert!(results[0].passed, "{:?}", results[0]);
    }

    #[test]
    fn exists_where_requires_array_target() {
        let results = run(
            r#"{"user": {"id": "a"}}"#,
            r#""$.user": { exists_where: { id: "a" } }"#,
        );
        assert!(!results[0].passed);
        assert!(
            results[0].message.contains("requires an array"),
            "expected type-mismatch message, got {:?}",
            results[0].message
        );
    }
}
