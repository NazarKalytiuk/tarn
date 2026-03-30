use crate::assert::types::AssertionResult;
use crate::regex_cache;
use std::collections::HashMap;

/// Assert response headers.
/// Each entry is a header name -> expected value.
/// Values can be:
///   - Simple string: exact match
///   - `contains "substring"`: substring check
///   - `matches "regex"`: regex match
pub fn assert_headers(
    expected: &HashMap<String, String>,
    actual: &HashMap<String, String>,
) -> Vec<AssertionResult> {
    let mut results = Vec::new();

    for (name, spec) in expected {
        let label = format!("header {}", name);
        let header_name_lower = name.to_lowercase();

        // Look up header case-insensitively
        let actual_value = actual
            .iter()
            .find(|(k, _)| k.to_lowercase() == header_name_lower)
            .map(|(_, v)| v.as_str());

        match actual_value {
            None => {
                results.push(AssertionResult::fail(
                    &label,
                    spec,
                    "<not present>",
                    format!("Header '{}' not found in response", name),
                ));
            }
            Some(actual_val) => {
                let result = evaluate_header_spec(&label, spec, actual_val);
                results.push(result);
            }
        }
    }

    results
}

fn evaluate_header_spec(label: &str, spec: &str, actual: &str) -> AssertionResult {
    // Check for "contains" prefix
    if let Some(rest) = spec.strip_prefix("contains ") {
        let needle = rest.trim_matches('"');
        if actual.contains(needle) {
            AssertionResult::pass(label, format!("contains \"{}\"", needle), actual)
        } else {
            AssertionResult::fail(
                label,
                format!("contains \"{}\"", needle),
                actual,
                format!(
                    "{}: expected to contain \"{}\", got \"{}\"",
                    label, needle, actual
                ),
            )
        }
    }
    // Check for "matches" prefix
    else if let Some(rest) = spec.strip_prefix("matches ") {
        let pattern = rest.trim_matches('"');
        match regex_cache::get(pattern) {
            Ok(re) => {
                if re.is_match(actual) {
                    AssertionResult::pass(label, format!("matches \"{}\"", pattern), actual)
                } else {
                    AssertionResult::fail(
                        label,
                        format!("matches \"{}\"", pattern),
                        actual,
                        format!(
                            "{}: \"{}\" does not match regex \"{}\"",
                            label, actual, pattern
                        ),
                    )
                }
            }
            Err(e) => AssertionResult::fail(
                label,
                format!("matches \"{}\"", pattern),
                actual,
                format!("Invalid regex \"{}\": {}", pattern, e),
            ),
        }
    }
    // Exact match
    else if actual == spec {
        AssertionResult::pass(label, spec, actual)
    } else {
        AssertionResult::fail(
            label,
            spec,
            actual,
            format!("{}: expected \"{}\", got \"{}\"", label, spec, actual),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_headers(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // --- Exact match ---

    #[test]
    fn exact_match_pass() {
        let expected = make_headers(&[("content-type", "application/json")]);
        let actual = make_headers(&[("content-type", "application/json")]);
        let results = assert_headers(&expected, &actual);
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    #[test]
    fn exact_match_fail() {
        let expected = make_headers(&[("content-type", "application/json")]);
        let actual = make_headers(&[("content-type", "text/html")]);
        let results = assert_headers(&expected, &actual);
        assert!(!results[0].passed);
    }

    // --- Case-insensitive lookup ---

    #[test]
    fn case_insensitive_header_name() {
        let expected = make_headers(&[("Content-Type", "application/json")]);
        let actual = make_headers(&[("content-type", "application/json")]);
        let results = assert_headers(&expected, &actual);
        assert!(results[0].passed);
    }

    // --- Missing header ---

    #[test]
    fn missing_header() {
        let expected = make_headers(&[("x-custom", "value")]);
        let actual = make_headers(&[("content-type", "application/json")]);
        let results = assert_headers(&expected, &actual);
        assert!(!results[0].passed);
        assert!(results[0].message.contains("not found"));
    }

    // --- Contains ---

    #[test]
    fn contains_pass() {
        let expected = make_headers(&[("content-type", "contains \"application/json\"")]);
        let actual = make_headers(&[("content-type", "application/json; charset=utf-8")]);
        let results = assert_headers(&expected, &actual);
        assert!(results[0].passed);
    }

    #[test]
    fn contains_fail() {
        let expected = make_headers(&[("content-type", "contains \"xml\"")]);
        let actual = make_headers(&[("content-type", "application/json")]);
        let results = assert_headers(&expected, &actual);
        assert!(!results[0].passed);
    }

    // --- Matches (regex) ---

    #[test]
    fn matches_pass() {
        let expected = make_headers(&[("x-request-id", "matches \"^[a-f0-9-]{36}$\"")]);
        let actual = make_headers(&[("x-request-id", "550e8400-e29b-41d4-a716-446655440000")]);
        let results = assert_headers(&expected, &actual);
        assert!(results[0].passed);
    }

    #[test]
    fn matches_fail() {
        let expected = make_headers(&[("x-request-id", "matches \"^[a-f0-9-]{36}$\"")]);
        let actual = make_headers(&[("x-request-id", "not-a-uuid")]);
        let results = assert_headers(&expected, &actual);
        assert!(!results[0].passed);
    }

    #[test]
    fn matches_invalid_regex() {
        let expected = make_headers(&[("x-header", "matches \"[invalid\"")]);
        let actual = make_headers(&[("x-header", "test")]);
        let results = assert_headers(&expected, &actual);
        assert!(!results[0].passed);
        assert!(results[0].message.contains("Invalid regex"));
    }

    // --- Multiple headers ---

    #[test]
    fn multiple_headers() {
        let expected = make_headers(&[
            ("content-type", "contains \"json\""),
            ("x-request-id", "matches \"^[a-f0-9-]+$\""),
        ]);
        let actual = make_headers(&[
            ("content-type", "application/json"),
            ("x-request-id", "abc-123-def"),
        ]);
        let results = assert_headers(&expected, &actual);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.passed));
    }
}
