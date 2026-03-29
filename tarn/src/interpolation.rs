use crate::builtin;
use crate::capture;
use regex::Regex;
use std::collections::HashMap;

/// Interpolation context holding all available variables.
#[derive(Default)]
pub struct Context {
    pub env: HashMap<String, String>,
    pub captures: HashMap<String, serde_json::Value>,
}

impl Context {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Interpolate all `{{ ... }}` expressions in a string.
/// Supports:
///   - `{{ env.var_name }}` — environment variables
///   - `{{ capture.var_name }}` — captured values from previous steps
///   - `{{ $builtin }}` — built-in functions ($uuid, $random_hex, etc.)
pub fn interpolate(template: &str, ctx: &Context) -> String {
    let re = Regex::new(r"\{\{\s*(.+?)\s*\}\}").unwrap();

    re.replace_all(template, |caps: &regex::Captures| {
        let expr = caps[1].trim();
        resolve_expression(expr, ctx)
    })
    .into_owned()
}

/// Interpolate all string values in a JSON value recursively.
/// Preserves types when a JSON string value is a single capture expression.
pub fn interpolate_json(value: &serde_json::Value, ctx: &Context) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            // If the entire string is a single capture expression, preserve its type
            if let Some(typed_value) = try_resolve_typed(s.trim(), ctx) {
                return typed_value;
            }
            serde_json::Value::String(interpolate(s, ctx))
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(|v| interpolate_json(v, ctx)).collect())
        }
        serde_json::Value::Object(obj) => {
            let new_obj: serde_json::Map<String, serde_json::Value> = obj
                .iter()
                .map(|(k, v)| (interpolate(k, ctx), interpolate_json(v, ctx)))
                .collect();
            serde_json::Value::Object(new_obj)
        }
        other => other.clone(),
    }
}

/// Interpolate all string values in a HashMap.
pub fn interpolate_headers(
    headers: &HashMap<String, String>,
    ctx: &Context,
) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(k, v)| (k.clone(), interpolate(v, ctx)))
        .collect()
}

/// Try to resolve a string as a single typed capture expression.
/// Returns the typed JSON value if the string is exactly `{{ capture.name }}`.
fn try_resolve_typed(s: &str, ctx: &Context) -> Option<serde_json::Value> {
    let re = Regex::new(r"^\{\{\s*capture\.(\w+)\s*\}\}$").unwrap();
    if let Some(caps) = re.captures(s) {
        let name = &caps[1];
        return ctx.captures.get(name).cloned();
    }
    None
}

/// Resolve a single expression (the content inside `{{ ... }}`).
fn resolve_expression(expr: &str, ctx: &Context) -> String {
    // env.var_name
    if let Some(var_name) = expr.strip_prefix("env.") {
        return ctx
            .env
            .get(var_name)
            .cloned()
            .unwrap_or_else(|| format!("{{{{ env.{} }}}}", var_name));
    }

    // capture.var_name — convert typed value to string for string contexts
    if let Some(var_name) = expr.strip_prefix("capture.") {
        return ctx
            .captures
            .get(var_name)
            .map(capture::value_to_string)
            .unwrap_or_else(|| format!("{{{{ capture.{} }}}}", var_name));
    }

    // Built-in function ($uuid, $random_hex, etc.)
    if expr.starts_with('$') {
        if let Some(result) = builtin::evaluate(expr) {
            return result;
        }
    }

    // Unknown expression: leave as-is
    format!("{{{{ {} }}}}", expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_ctx(
        env: &[(&str, &str)],
        captures: &[(&str, serde_json::Value)],
    ) -> Context {
        Context {
            env: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            captures: captures
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        }
    }

    fn make_string_ctx(env: &[(&str, &str)], captures: &[(&str, &str)]) -> Context {
        Context {
            env: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            captures: captures
                .iter()
                .map(|(k, v)| (k.to_string(), json!(v)))
                .collect(),
        }
    }

    // --- Basic interpolation ---

    #[test]
    fn interpolate_env_variable() {
        let ctx = make_string_ctx(&[("base_url", "http://localhost:3000")], &[]);
        assert_eq!(
            interpolate("{{ env.base_url }}/health", &ctx),
            "http://localhost:3000/health"
        );
    }

    #[test]
    fn interpolate_capture_variable() {
        let ctx = make_string_ctx(&[], &[("user_id", "usr_123")]);
        assert_eq!(
            interpolate("/users/{{ capture.user_id }}", &ctx),
            "/users/usr_123"
        );
    }

    #[test]
    fn interpolate_builtin_uuid() {
        let ctx = make_string_ctx(&[], &[]);
        let result = interpolate("{{ $uuid }}", &ctx);
        assert_ne!(result, "{{ $uuid }}");
        assert_eq!(result.len(), 36);
    }

    #[test]
    fn interpolate_builtin_random_hex() {
        let ctx = make_string_ctx(&[], &[]);
        let result = interpolate("prefix_{{ $random_hex(6) }}@test.com", &ctx);
        assert!(result.starts_with("prefix_"));
        assert!(result.ends_with("@test.com"));
        // 6 hex chars between
        assert_eq!(result.len(), "prefix_".len() + 6 + "@test.com".len());
    }

    #[test]
    fn interpolate_builtin_timestamp() {
        let ctx = make_string_ctx(&[], &[]);
        let result = interpolate("{{ $timestamp }}", &ctx);
        let ts: i64 = result.parse().unwrap();
        assert!(ts > 1_000_000_000);
    }

    // --- Multiple variables ---

    #[test]
    fn interpolate_multiple_variables() {
        let ctx = make_string_ctx(
            &[("base_url", "http://localhost:3000")],
            &[("token", "abc123")],
        );
        let result = interpolate("{{ env.base_url }}/api?token={{ capture.token }}", &ctx);
        assert_eq!(result, "http://localhost:3000/api?token=abc123");
    }

    // --- Missing variables ---

    #[test]
    fn missing_env_variable_preserved() {
        let ctx = make_string_ctx(&[], &[]);
        let result = interpolate("{{ env.missing }}", &ctx);
        assert_eq!(result, "{{ env.missing }}");
    }

    #[test]
    fn missing_capture_variable_preserved() {
        let ctx = make_string_ctx(&[], &[]);
        let result = interpolate("{{ capture.missing }}", &ctx);
        assert_eq!(result, "{{ capture.missing }}");
    }

    // --- No interpolation needed ---

    #[test]
    fn no_templates_unchanged() {
        let ctx = make_string_ctx(&[], &[]);
        assert_eq!(interpolate("plain text", &ctx), "plain text");
    }

    #[test]
    fn empty_string() {
        let ctx = make_string_ctx(&[], &[]);
        assert_eq!(interpolate("", &ctx), "");
    }

    // --- Whitespace handling ---

    #[test]
    fn extra_whitespace_in_template() {
        let ctx = make_string_ctx(&[("x", "val")], &[]);
        assert_eq!(interpolate("{{  env.x  }}", &ctx), "val");
        assert_eq!(interpolate("{{env.x}}", &ctx), "val");
    }

    // --- JSON interpolation ---

    #[test]
    fn interpolate_json_string() {
        let ctx = make_string_ctx(&[("name", "Alice")], &[]);
        let val = json!("Hello {{ env.name }}");
        let result = interpolate_json(&val, &ctx);
        assert_eq!(result, json!("Hello Alice"));
    }

    #[test]
    fn interpolate_json_object() {
        let ctx = make_string_ctx(&[("email", "a@b.com")], &[]);
        let val = json!({"email": "{{ env.email }}", "count": 5});
        let result = interpolate_json(&val, &ctx);
        assert_eq!(result["email"], "a@b.com");
        assert_eq!(result["count"], 5); // numbers unchanged
    }

    #[test]
    fn interpolate_json_nested() {
        let ctx = make_string_ctx(&[("city", "NYC")], &[]);
        let val = json!({"user": {"address": {"city": "{{ env.city }}"}}});
        let result = interpolate_json(&val, &ctx);
        assert_eq!(result["user"]["address"]["city"], "NYC");
    }

    #[test]
    fn interpolate_json_array() {
        let ctx = make_string_ctx(&[("tag", "test")], &[]);
        let val = json!(["{{ env.tag }}", "static"]);
        let result = interpolate_json(&val, &ctx);
        assert_eq!(result[0], "test");
        assert_eq!(result[1], "static");
    }

    #[test]
    fn interpolate_json_preserves_non_strings() {
        let ctx = make_string_ctx(&[], &[]);
        let val = json!({"num": 42, "bool": true, "null": null});
        let result = interpolate_json(&val, &ctx);
        assert_eq!(result, val);
    }

    // --- Type-aware capture interpolation in JSON ---

    #[test]
    fn interpolate_json_preserves_number_capture() {
        let ctx = make_ctx(&[], &[("count", json!(42))]);
        let val = json!({"count": "{{ capture.count }}"});
        let result = interpolate_json(&val, &ctx);
        assert_eq!(result["count"], json!(42));
        assert!(result["count"].is_number());
    }

    #[test]
    fn interpolate_json_preserves_bool_capture() {
        let ctx = make_ctx(&[], &[("active", json!(true))]);
        let val = json!({"active": "{{ capture.active }}"});
        let result = interpolate_json(&val, &ctx);
        assert_eq!(result["active"], json!(true));
        assert!(result["active"].is_boolean());
    }

    #[test]
    fn interpolate_json_mixed_string_stays_string() {
        let ctx = make_ctx(&[], &[("count", json!(42))]);
        let val = json!("count is {{ capture.count }}");
        let result = interpolate_json(&val, &ctx);
        assert_eq!(result, json!("count is 42"));
    }

    #[test]
    fn interpolate_capture_number_in_url_becomes_string() {
        let ctx = make_ctx(&[], &[("id", json!(42))]);
        let result = interpolate("/users/{{ capture.id }}", &ctx);
        assert_eq!(result, "/users/42");
    }

    // --- Header interpolation ---

    #[test]
    fn interpolate_headers_basic() {
        let ctx = make_string_ctx(&[], &[("token", "xyz")]);
        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), "Bearer {{ capture.token }}".into());
        headers.insert("Accept".into(), "application/json".into());

        let result = interpolate_headers(&headers, &ctx);
        assert_eq!(result.get("Authorization").unwrap(), "Bearer xyz");
        assert_eq!(result.get("Accept").unwrap(), "application/json");
    }

    // --- Unknown expressions ---

    #[test]
    fn unknown_expression_preserved() {
        let ctx = make_string_ctx(&[], &[]);
        let result = interpolate("{{ something.else }}", &ctx);
        assert_eq!(result, "{{ something.else }}");
    }

    // --- Context creation ---

    #[test]
    fn context_new_is_empty() {
        let ctx = Context::new();
        assert!(ctx.env.is_empty());
        assert!(ctx.captures.is_empty());
    }
}
