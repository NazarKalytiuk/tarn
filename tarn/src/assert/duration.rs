use crate::assert::types::AssertionResult;

/// Assert response duration against a spec like "< 500ms" or "<= 1000ms".
pub fn assert_duration(spec: &str, actual_ms: u64) -> AssertionResult {
    match parse_duration_spec(spec) {
        Ok((op, threshold_ms)) => {
            let passes = match op {
                DurationOp::Lt => actual_ms < threshold_ms,
                DurationOp::Lte => actual_ms <= threshold_ms,
                DurationOp::Gt => actual_ms > threshold_ms,
                DurationOp::Gte => actual_ms >= threshold_ms,
            };

            if passes {
                AssertionResult::pass("duration", spec, format!("{}ms", actual_ms))
            } else {
                AssertionResult::fail(
                    "duration",
                    spec,
                    format!("{}ms", actual_ms),
                    format!("Response time {}ms does not satisfy {}", actual_ms, spec),
                )
            }
        }
        Err(msg) => AssertionResult::fail(
            "duration",
            spec,
            format!("{}ms", actual_ms),
            format!("Invalid duration spec \"{}\": {}", spec, msg),
        ),
    }
}

#[derive(Debug, PartialEq)]
enum DurationOp {
    Lt,
    Lte,
    Gt,
    Gte,
}

fn parse_duration_spec(spec: &str) -> Result<(DurationOp, u64), String> {
    let spec = spec.trim();

    let (op, rest) = if let Some(rest) = spec.strip_prefix("<=") {
        (DurationOp::Lte, rest)
    } else if let Some(rest) = spec.strip_prefix("<") {
        (DurationOp::Lt, rest)
    } else if let Some(rest) = spec.strip_prefix(">=") {
        (DurationOp::Gte, rest)
    } else if let Some(rest) = spec.strip_prefix(">") {
        (DurationOp::Gt, rest)
    } else {
        return Err("Must start with <, <=, >, or >=".into());
    };

    let rest = rest.trim();

    // Parse the number and unit
    let (num_str, unit) = if let Some(stripped) = rest.strip_suffix("ms") {
        (stripped.trim(), "ms")
    } else if let Some(stripped) = rest.strip_suffix('s') {
        (stripped.trim(), "s")
    } else {
        // Default to ms if no unit
        (rest, "ms")
    };

    let value: u64 = num_str
        .parse()
        .map_err(|_| format!("Cannot parse '{}' as a number", num_str))?;

    let ms = match unit {
        "s" => value * 1000,
        _ => value,
    };

    Ok((op, ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Parsing ---

    #[test]
    fn parse_lt_ms() {
        let (op, ms) = parse_duration_spec("< 500ms").unwrap();
        assert_eq!(op, DurationOp::Lt);
        assert_eq!(ms, 500);
    }

    #[test]
    fn parse_lte_ms() {
        let (op, ms) = parse_duration_spec("<= 1000ms").unwrap();
        assert_eq!(op, DurationOp::Lte);
        assert_eq!(ms, 1000);
    }

    #[test]
    fn parse_gt_ms() {
        let (op, ms) = parse_duration_spec("> 100ms").unwrap();
        assert_eq!(op, DurationOp::Gt);
        assert_eq!(ms, 100);
    }

    #[test]
    fn parse_gte_ms() {
        let (op, ms) = parse_duration_spec(">= 200ms").unwrap();
        assert_eq!(op, DurationOp::Gte);
        assert_eq!(ms, 200);
    }

    #[test]
    fn parse_seconds() {
        let (op, ms) = parse_duration_spec("< 2s").unwrap();
        assert_eq!(op, DurationOp::Lt);
        assert_eq!(ms, 2000);
    }

    #[test]
    fn parse_no_unit_defaults_to_ms() {
        let (op, ms) = parse_duration_spec("< 300").unwrap();
        assert_eq!(op, DurationOp::Lt);
        assert_eq!(ms, 300);
    }

    #[test]
    fn parse_no_space() {
        let (op, ms) = parse_duration_spec("<500ms").unwrap();
        assert_eq!(op, DurationOp::Lt);
        assert_eq!(ms, 500);
    }

    #[test]
    fn parse_invalid_no_operator() {
        assert!(parse_duration_spec("500ms").is_err());
    }

    #[test]
    fn parse_invalid_number() {
        assert!(parse_duration_spec("< abcms").is_err());
    }

    // --- Assertion results ---

    #[test]
    fn lt_pass() {
        let r = assert_duration("< 500ms", 200);
        assert!(r.passed);
    }

    #[test]
    fn lt_fail_equal() {
        let r = assert_duration("< 500ms", 500);
        assert!(!r.passed);
    }

    #[test]
    fn lt_fail_over() {
        let r = assert_duration("< 500ms", 600);
        assert!(!r.passed);
        assert!(r.message.contains("600ms"));
    }

    #[test]
    fn lte_pass_equal() {
        let r = assert_duration("<= 500ms", 500);
        assert!(r.passed);
    }

    #[test]
    fn lte_pass_under() {
        let r = assert_duration("<= 500ms", 300);
        assert!(r.passed);
    }

    #[test]
    fn lte_fail() {
        let r = assert_duration("<= 500ms", 501);
        assert!(!r.passed);
    }

    #[test]
    fn gt_pass() {
        let r = assert_duration("> 100ms", 200);
        assert!(r.passed);
    }

    #[test]
    fn gt_fail() {
        let r = assert_duration("> 100ms", 50);
        assert!(!r.passed);
    }

    #[test]
    fn gte_pass_equal() {
        let r = assert_duration(">= 100ms", 100);
        assert!(r.passed);
    }

    #[test]
    fn seconds_conversion() {
        let r = assert_duration("< 2s", 1500);
        assert!(r.passed);
        let r = assert_duration("< 2s", 2500);
        assert!(!r.passed);
    }

    #[test]
    fn invalid_spec_returns_failure() {
        let r = assert_duration("invalid", 100);
        assert!(!r.passed);
        assert!(r.message.contains("Invalid duration spec"));
    }
}
