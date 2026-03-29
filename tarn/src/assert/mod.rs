pub mod body;
pub mod duration;
pub mod headers;
pub mod status;
pub mod types;

use std::collections::HashMap;
use types::AssertionResult;

use crate::model::Assertion;

/// Run all assertions for a step and return results.
pub fn run_assertions(
    assertion: &Assertion,
    response_status: u16,
    response_headers: &HashMap<String, String>,
    response_body: &serde_json::Value,
    duration_ms: u64,
) -> Vec<AssertionResult> {
    let mut results = Vec::new();

    // Status assertion
    if let Some(ref expected_status) = assertion.status {
        results.push(status::assert_status(expected_status, response_status));
    }

    // Duration assertion
    if let Some(ref duration_spec) = assertion.duration {
        results.push(duration::assert_duration(duration_spec, duration_ms));
    }

    // Header assertions
    if let Some(ref expected_headers) = assertion.headers {
        results.extend(headers::assert_headers(expected_headers, response_headers));
    }

    // Body assertions
    if let Some(ref body_assertions) = assertion.body {
        results.extend(body::assert_body(response_body, body_assertions));
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StatusAssertion;

    #[test]
    fn run_assertions_status_only() {
        let assertion = Assertion {
            status: Some(StatusAssertion::Exact(200)),
            duration: None,
            headers: None,
            body: None,
        };
        let headers = HashMap::new();
        let body = serde_json::Value::Null;
        let results = run_assertions(&assertion, 200, &headers, &body, 100);
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    #[test]
    fn run_assertions_status_fails() {
        let assertion = Assertion {
            status: Some(StatusAssertion::Exact(200)),
            duration: None,
            headers: None,
            body: None,
        };
        let headers = HashMap::new();
        let body = serde_json::Value::Null;
        let results = run_assertions(&assertion, 404, &headers, &body, 100);
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
    }

    #[test]
    fn run_assertions_no_assertions() {
        let assertion = Assertion {
            status: None,
            duration: None,
            headers: None,
            body: None,
        };
        let headers = HashMap::new();
        let body = serde_json::Value::Null;
        let results = run_assertions(&assertion, 200, &headers, &body, 100);
        assert!(results.is_empty());
    }
}
