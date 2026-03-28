use crate::assert::types::AssertionResult;

/// Assert that the HTTP response status code matches the expected value.
pub fn assert_status(expected: u16, actual: u16) -> AssertionResult {
    if expected == actual {
        AssertionResult::pass("status", expected.to_string(), actual.to_string())
    } else {
        AssertionResult::fail(
            "status",
            expected.to_string(),
            actual.to_string(),
            format!("Expected HTTP status {}, got {}", expected, actual),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_match_passes() {
        let r = assert_status(200, 200);
        assert!(r.passed);
        assert_eq!(r.assertion, "status");
        assert_eq!(r.expected, "200");
        assert_eq!(r.actual, "200");
    }

    #[test]
    fn status_mismatch_fails() {
        let r = assert_status(200, 404);
        assert!(!r.passed);
        assert_eq!(r.expected, "200");
        assert_eq!(r.actual, "404");
        assert!(r.message.contains("Expected HTTP status 200, got 404"));
    }

    #[test]
    fn status_201_created() {
        let r = assert_status(201, 201);
        assert!(r.passed);
    }

    #[test]
    fn status_204_no_content() {
        let r = assert_status(204, 204);
        assert!(r.passed);
    }

    #[test]
    fn status_500_server_error() {
        let r = assert_status(200, 500);
        assert!(!r.passed);
        assert!(r.message.contains("500"));
    }

    #[test]
    fn status_401_unauthorized() {
        let r = assert_status(401, 403);
        assert!(!r.passed);
        assert_eq!(r.expected, "401");
        assert_eq!(r.actual, "403");
    }
}
