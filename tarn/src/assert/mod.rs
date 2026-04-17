pub mod body;
pub mod duration;
pub mod headers;
pub mod hints;
pub mod redirect;
pub mod status;
pub mod types;

use types::AssertionResult;

use crate::http::HttpResponse;
use crate::model::Assertion;

/// Run all assertions for a step and return results.
pub fn run_assertions(assertion: &Assertion, response: &HttpResponse) -> Vec<AssertionResult> {
    let mut results = Vec::new();

    // Status assertion
    if let Some(ref expected_status) = assertion.status {
        results.push(status::assert_status(expected_status, response.status));
    }

    // Duration assertion
    if let Some(ref duration_spec) = assertion.duration {
        results.push(duration::assert_duration(
            duration_spec,
            response.duration_ms,
        ));
    }

    // Redirect assertions
    if let Some(ref expected_redirect) = assertion.redirect {
        results.extend(redirect::assert_redirect(
            expected_redirect,
            &response.url,
            response.redirect_count,
        ));
    }

    // Header assertions
    if let Some(ref expected_headers) = assertion.headers {
        results.extend(headers::assert_headers(expected_headers, &response.headers));
    }

    // Body assertions
    if let Some(ref body_assertions) = assertion.body {
        results.extend(body::assert_body(
            &response.body,
            &response.body_bytes,
            body_assertions,
        ));
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{HttpResponse, ResponseTimings};
    use crate::model::StatusAssertion;
    use std::collections::HashMap;

    fn mock_response(status: u16) -> HttpResponse {
        HttpResponse {
            status,
            url: String::new(),
            redirect_count: 0,
            headers: HashMap::new(),
            raw_headers: vec![],
            body_bytes: vec![],
            body: serde_json::Value::Null,
            duration_ms: 100,
            timings: ResponseTimings {
                total_ms: 100,
                ttfb_ms: 50,
                body_read_ms: 50,
                connect_ms: None,
                tls_ms: None,
            },
        }
    }

    #[test]
    fn run_assertions_status_only() {
        let assertion = Assertion {
            status: Some(StatusAssertion::Exact(200)),
            duration: None,
            redirect: None,
            headers: None,
            body: None,
        };
        let response = mock_response(200);
        let results = run_assertions(&assertion, &response);
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    #[test]
    fn run_assertions_status_fails() {
        let assertion = Assertion {
            status: Some(StatusAssertion::Exact(200)),
            duration: None,
            redirect: None,
            headers: None,
            body: None,
        };
        let response = mock_response(404);
        let results = run_assertions(&assertion, &response);
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
    }

    #[test]
    fn run_assertions_no_assertions() {
        let assertion = Assertion {
            status: None,
            duration: None,
            redirect: None,
            headers: None,
            body: None,
        };
        let response = mock_response(200);
        let results = run_assertions(&assertion, &response);
        assert!(results.is_empty());
    }
}
