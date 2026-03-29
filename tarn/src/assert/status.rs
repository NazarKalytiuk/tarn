use crate::assert::types::AssertionResult;
use crate::model::StatusAssertion;

/// Assert that the HTTP response status code matches the expected assertion.
/// Supports exact values, shorthand ranges ("2xx"), and complex specs.
pub fn assert_status(expected: &StatusAssertion, actual: u16) -> AssertionResult {
    match expected {
        StatusAssertion::Exact(code) => {
            if *code == actual {
                AssertionResult::pass("status", code.to_string(), actual.to_string())
            } else {
                AssertionResult::fail(
                    "status",
                    code.to_string(),
                    actual.to_string(),
                    format!("Expected HTTP status {}, got {}", code, actual),
                )
            }
        }
        StatusAssertion::Shorthand(pattern) => assert_status_shorthand(pattern, actual),
        StatusAssertion::Complex(spec) => assert_status_complex(spec, actual),
    }
}

/// Assert a shorthand status pattern like "2xx", "4xx", "5xx".
fn assert_status_shorthand(pattern: &str, actual: u16) -> AssertionResult {
    let pattern_lower = pattern.to_lowercase();

    // Parse "Nxx" pattern — first char is the status class digit
    let expected_class = pattern_lower
        .chars()
        .next()
        .and_then(|c| c.to_digit(10))
        .map(|d| d as u16);

    match expected_class {
        Some(class) if pattern_lower.ends_with("xx") => {
            let actual_class = actual / 100;
            if actual_class == class {
                AssertionResult::pass("status", pattern.to_string(), actual.to_string())
            } else {
                AssertionResult::fail(
                    "status",
                    pattern.to_string(),
                    actual.to_string(),
                    format!(
                        "Expected HTTP status in {} range, got {}",
                        pattern, actual
                    ),
                )
            }
        }
        _ => AssertionResult::fail(
            "status",
            pattern.to_string(),
            actual.to_string(),
            format!(
                "Invalid status shorthand '{}'. Use format like '2xx', '4xx', '5xx'",
                pattern
            ),
        ),
    }
}

/// Assert a complex status specification with ranges and sets.
fn assert_status_complex(
    spec: &crate::model::StatusSpec,
    actual: u16,
) -> AssertionResult {
    let mut conditions: Vec<String> = Vec::new();
    let mut passed = true;

    if let Some(ref set) = spec.in_set {
        conditions.push(format!("in {:?}", set));
        if !set.contains(&actual) {
            passed = false;
        }
    }
    if let Some(gte) = spec.gte {
        conditions.push(format!(">= {}", gte));
        if actual < gte {
            passed = false;
        }
    }
    if let Some(gt) = spec.gt {
        conditions.push(format!("> {}", gt));
        if actual <= gt {
            passed = false;
        }
    }
    if let Some(lte) = spec.lte {
        conditions.push(format!("<= {}", lte));
        if actual > lte {
            passed = false;
        }
    }
    if let Some(lt) = spec.lt {
        conditions.push(format!("< {}", lt));
        if actual >= lt {
            passed = false;
        }
    }

    let expected_str = conditions.join(", ");

    if passed {
        AssertionResult::pass("status", &expected_str, actual.to_string())
    } else {
        AssertionResult::fail(
            "status",
            &expected_str,
            actual.to_string(),
            format!("Expected HTTP status {}, got {}", expected_str, actual),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StatusSpec;

    // --- Exact status ---

    #[test]
    fn status_match_passes() {
        let r = assert_status(&StatusAssertion::Exact(200), 200);
        assert!(r.passed);
        assert_eq!(r.assertion, "status");
        assert_eq!(r.expected, "200");
        assert_eq!(r.actual, "200");
    }

    #[test]
    fn status_mismatch_fails() {
        let r = assert_status(&StatusAssertion::Exact(200), 404);
        assert!(!r.passed);
        assert_eq!(r.expected, "200");
        assert_eq!(r.actual, "404");
        assert!(r.message.contains("Expected HTTP status 200, got 404"));
    }

    #[test]
    fn status_201_created() {
        let r = assert_status(&StatusAssertion::Exact(201), 201);
        assert!(r.passed);
    }

    #[test]
    fn status_204_no_content() {
        let r = assert_status(&StatusAssertion::Exact(204), 204);
        assert!(r.passed);
    }

    #[test]
    fn status_500_server_error() {
        let r = assert_status(&StatusAssertion::Exact(200), 500);
        assert!(!r.passed);
        assert!(r.message.contains("500"));
    }

    #[test]
    fn status_401_unauthorized() {
        let r = assert_status(&StatusAssertion::Exact(401), 403);
        assert!(!r.passed);
        assert_eq!(r.expected, "401");
        assert_eq!(r.actual, "403");
    }

    // --- Shorthand status ranges ---

    #[test]
    fn shorthand_2xx_matches_200() {
        let r = assert_status(&StatusAssertion::Shorthand("2xx".into()), 200);
        assert!(r.passed);
    }

    #[test]
    fn shorthand_2xx_matches_201() {
        let r = assert_status(&StatusAssertion::Shorthand("2xx".into()), 201);
        assert!(r.passed);
    }

    #[test]
    fn shorthand_2xx_matches_204() {
        let r = assert_status(&StatusAssertion::Shorthand("2xx".into()), 204);
        assert!(r.passed);
    }

    #[test]
    fn shorthand_2xx_rejects_301() {
        let r = assert_status(&StatusAssertion::Shorthand("2xx".into()), 301);
        assert!(!r.passed);
    }

    #[test]
    fn shorthand_4xx_matches_400() {
        let r = assert_status(&StatusAssertion::Shorthand("4xx".into()), 400);
        assert!(r.passed);
    }

    #[test]
    fn shorthand_4xx_matches_422() {
        let r = assert_status(&StatusAssertion::Shorthand("4xx".into()), 422);
        assert!(r.passed);
    }

    #[test]
    fn shorthand_4xx_rejects_500() {
        let r = assert_status(&StatusAssertion::Shorthand("4xx".into()), 500);
        assert!(!r.passed);
    }

    #[test]
    fn shorthand_5xx_matches_503() {
        let r = assert_status(&StatusAssertion::Shorthand("5xx".into()), 503);
        assert!(r.passed);
    }

    #[test]
    fn shorthand_invalid_pattern() {
        let r = assert_status(&StatusAssertion::Shorthand("abc".into()), 200);
        assert!(!r.passed);
        assert!(r.message.contains("Invalid status shorthand"));
    }

    // --- Complex status specs ---

    #[test]
    fn complex_in_set_passes() {
        let spec = StatusAssertion::Complex(StatusSpec {
            in_set: Some(vec![200, 201, 204]),
            gte: None,
            gt: None,
            lte: None,
            lt: None,
        });
        let r = assert_status(&spec, 201);
        assert!(r.passed);
    }

    #[test]
    fn complex_in_set_fails() {
        let spec = StatusAssertion::Complex(StatusSpec {
            in_set: Some(vec![200, 201]),
            gte: None,
            gt: None,
            lte: None,
            lt: None,
        });
        let r = assert_status(&spec, 404);
        assert!(!r.passed);
    }

    #[test]
    fn complex_range_gte_lt() {
        let spec = StatusAssertion::Complex(StatusSpec {
            in_set: None,
            gte: Some(400),
            gt: None,
            lte: None,
            lt: Some(500),
        });
        assert!(assert_status(&spec, 400).passed);
        assert!(assert_status(&spec, 422).passed);
        assert!(assert_status(&spec, 499).passed);
        assert!(!assert_status(&spec, 399).passed);
        assert!(!assert_status(&spec, 500).passed);
    }

    #[test]
    fn complex_gt_lte() {
        let spec = StatusAssertion::Complex(StatusSpec {
            in_set: None,
            gte: None,
            gt: Some(199),
            lte: Some(299),
            lt: None,
        });
        assert!(assert_status(&spec, 200).passed);
        assert!(assert_status(&spec, 299).passed);
        assert!(!assert_status(&spec, 199).passed);
        assert!(!assert_status(&spec, 300).passed);
    }
}
