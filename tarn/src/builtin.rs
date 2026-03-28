use rand::Rng;
use uuid::Uuid;

/// Evaluate a built-in function expression.
/// Returns Some(result) if the expression is a recognized built-in, None otherwise.
pub fn evaluate(expr: &str) -> Option<String> {
    let expr = expr.trim();

    if expr == "$uuid" {
        return Some(Uuid::new_v4().to_string());
    }

    if expr == "$timestamp" {
        return Some(chrono::Utc::now().timestamp().to_string());
    }

    if expr == "$now_iso" {
        return Some(chrono::Utc::now().to_rfc3339());
    }

    // $random_hex(n)
    if let Some(inner) = strip_func(expr, "$random_hex") {
        let len: usize = inner.parse().ok()?;
        let bytes = (0..len)
            .map(|_| format!("{:x}", rand::rng().random_range(0u8..16)))
            .collect::<String>();
        return Some(bytes);
    }

    // $random_int(min, max)
    if let Some(inner) = strip_func(expr, "$random_int") {
        let parts: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
        if parts.len() == 2 {
            let min: i64 = parts[0].parse().ok()?;
            let max: i64 = parts[1].parse().ok()?;
            let val = rand::rng().random_range(min..=max);
            return Some(val.to_string());
        }
        return None;
    }

    None
}

/// Extract the arguments from a function call like "$func_name(args)".
fn strip_func<'a>(expr: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = expr.strip_prefix(prefix)?;
    let rest = rest.strip_prefix('(')?;
    let rest = rest.strip_suffix(')')?;
    Some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_generates_valid_uuid() {
        let result = evaluate("$uuid").unwrap();
        assert_eq!(result.len(), 36); // UUID v4 format: 8-4-4-4-12
        assert!(result.contains('-'));
        // Verify it parses as a UUID
        assert!(Uuid::parse_str(&result).is_ok());
    }

    #[test]
    fn timestamp_returns_number() {
        let result = evaluate("$timestamp").unwrap();
        let ts: i64 = result.parse().unwrap();
        assert!(ts > 1_000_000_000); // After 2001
    }

    #[test]
    fn now_iso_returns_valid_datetime() {
        let result = evaluate("$now_iso").unwrap();
        assert!(result.contains('T'));
        assert!(result.contains('+') || result.contains('Z'));
    }

    #[test]
    fn random_hex_correct_length() {
        let result = evaluate("$random_hex(8)").unwrap();
        assert_eq!(result.len(), 8);
        // All hex chars
        assert!(result.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn random_hex_different_lengths() {
        let r4 = evaluate("$random_hex(4)").unwrap();
        assert_eq!(r4.len(), 4);

        let r16 = evaluate("$random_hex(16)").unwrap();
        assert_eq!(r16.len(), 16);
    }

    #[test]
    fn random_int_in_range() {
        for _ in 0..100 {
            let result = evaluate("$random_int(1, 10)").unwrap();
            let val: i64 = result.parse().unwrap();
            assert!((1..=10).contains(&val));
        }
    }

    #[test]
    fn random_int_negative_range() {
        for _ in 0..50 {
            let result = evaluate("$random_int(-5, 5)").unwrap();
            let val: i64 = result.parse().unwrap();
            assert!((-5..=5).contains(&val));
        }
    }

    #[test]
    fn random_int_single_value() {
        let result = evaluate("$random_int(42, 42)").unwrap();
        assert_eq!(result, "42");
    }

    #[test]
    fn unknown_builtin_returns_none() {
        assert!(evaluate("$unknown").is_none());
        assert!(evaluate("$not_a_function(1)").is_none());
        assert!(evaluate("plain text").is_none());
    }

    #[test]
    fn random_hex_invalid_arg() {
        assert!(evaluate("$random_hex(abc)").is_none());
    }

    #[test]
    fn random_int_wrong_arg_count() {
        assert!(evaluate("$random_int(1)").is_none());
        assert!(evaluate("$random_int(1, 2, 3)").is_none());
    }

    #[test]
    fn strip_func_helper() {
        assert_eq!(strip_func("$random_hex(8)", "$random_hex"), Some("8"));
        assert_eq!(
            strip_func("$random_int(1, 10)", "$random_int"),
            Some("1, 10")
        );
        assert_eq!(strip_func("$other(x)", "$random_hex"), None);
        assert_eq!(strip_func("$random_hex", "$random_hex"), None); // no parens
    }

    #[test]
    fn uuid_generates_unique_values() {
        let a = evaluate("$uuid").unwrap();
        let b = evaluate("$uuid").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn whitespace_handling() {
        assert!(evaluate("  $uuid  ").is_some());
        assert!(evaluate(" $timestamp ").is_some());
    }
}
