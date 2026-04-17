use crate::builtin;
use crate::capture;
use indexmap::IndexMap;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

fn interpolation_regex() -> &'static regex::Regex {
    static REGEX: OnceLock<regex::Regex> = OnceLock::new();
    REGEX.get_or_init(|| regex::Regex::new(r"\{\{\s*(.+?)\s*\}\}").unwrap())
}

/// Interpolation context holding all available variables.
///
/// `optional_unset` tracks capture names that were declared with
/// `optional: true` (or a `when:` gate that did not match, or a missing
/// source without a `default:`) and therefore intentionally have no
/// value. Distinguishing this from "never declared" lets downstream
/// interpolation surface a precise
/// `"template variable 'X' was declared optional and not set"` error
/// instead of the generic unresolved-template fallback — which would
/// hide a real typo behind an already-handled optional miss.
#[derive(Default)]
pub struct Context {
    pub env: HashMap<String, String>,
    pub captures: HashMap<String, serde_json::Value>,
    pub optional_unset: HashSet<String>,
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
    interpolation_regex()
        .replace_all(template, |caps: &regex::Captures| {
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

/// Find unresolved template expressions remaining in a string after interpolation.
/// Returns the expression names (e.g., "capture.package_id", "env.base_url").
pub fn find_unresolved(s: &str) -> Vec<String> {
    interpolation_regex()
        .captures_iter(s)
        .map(|caps| caps[1].trim().to_string())
        .collect()
}

/// Find unresolved template expressions in all string values of a JSON value (recursive).
pub fn find_unresolved_in_json(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(s) => find_unresolved(s),
        serde_json::Value::Array(arr) => arr.iter().flat_map(find_unresolved_in_json).collect(),
        serde_json::Value::Object(obj) => obj.values().flat_map(find_unresolved_in_json).collect(),
        _ => vec![],
    }
}

/// Partition a raw list of unresolved `{{ expr }}` names into two
/// categories against a live [`Context`]:
///
/// * `optional_unset_refs`: `capture.X` references where X was declared
///   with `optional: true` (or a `when:` that did not match) earlier in
///   the run. Surfacing these separately lets the runner emit a
///   distinct
///   `"template variable 'X' was declared optional and not set"` error
///   instead of the generic unresolved-template fallback, which would
///   otherwise hide a real typo behind an already-handled optional miss.
/// * `unresolved`: every other still-unresolved name, i.e. references
///   to variables that were never declared at all (typos, forgotten
///   setup steps, missing env vars).
///
/// Order is preserved and duplicates are kept — the caller typically
/// sorts and dedups when producing a diagnostic.
pub fn classify_unresolved(raw: &[String], ctx: &Context) -> UnresolvedClassification {
    let mut optional_unset_refs = Vec::new();
    let mut unresolved = Vec::new();
    for expr in raw {
        match expr.strip_prefix("capture.") {
            Some(name_expr) => {
                // Pipeline: the capture name is the token before the
                // first `|`. Everything after is a transform that only
                // matters once a value is present.
                let name = name_expr.split('|').next().unwrap_or("").trim();
                if !name.is_empty() && ctx.optional_unset.contains(name) {
                    optional_unset_refs.push(name.to_string());
                } else {
                    unresolved.push(expr.clone());
                }
            }
            None => unresolved.push(expr.clone()),
        }
    }
    UnresolvedClassification {
        optional_unset_refs,
        unresolved,
    }
}

/// Result of [`classify_unresolved`]: optional-unset references are
/// separated from truly-unresolved ones so the runner can emit a
/// distinct diagnostic for each class.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UnresolvedClassification {
    /// `capture.X` references where X was explicitly declared optional
    /// and has no value, in the order they were encountered.
    pub optional_unset_refs: Vec<String>,
    /// Template expressions that remain unresolved for a non-optional
    /// reason (typo, forgotten setup step, missing env var).
    pub unresolved: Vec<String>,
}

/// Interpolate all strings in an ordered string map.
pub fn interpolate_string_map(
    values: &IndexMap<String, String>,
    ctx: &Context,
) -> IndexMap<String, String> {
    values
        .iter()
        .map(|(k, v)| (interpolate(k, ctx), interpolate(v, ctx)))
        .collect()
}

/// Try to resolve a string as a single typed capture expression.
/// Returns the typed JSON value if the string is exactly `{{ capture.name }}` or a transformed variant.
fn try_resolve_typed(s: &str, ctx: &Context) -> Option<Value> {
    let caps = interpolation_regex().captures(s)?;
    let whole = caps.get(0)?;
    if whole.start() != 0 || whole.end() != s.len() {
        return None;
    }
    resolve_capture_expression(caps.get(1)?.as_str().trim(), ctx).ok()
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
    if expr.starts_with("capture.") {
        return resolve_capture_expression(expr, ctx)
            .map(|value| capture::value_to_string(&value))
            .unwrap_or_else(|_| format!("{{{{ {} }}}}", expr));
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

fn resolve_capture_expression(expr: &str, ctx: &Context) -> Result<Value, String> {
    let Some(rest) = expr.strip_prefix("capture.") else {
        return Err("Not a capture expression".to_string());
    };

    let parts = split_capture_pipeline(rest);
    let Some(name) = parts.first().map(|part| part.trim()) else {
        return Err("Missing capture name".to_string());
    };
    if name.is_empty() {
        return Err("Missing capture name".to_string());
    }

    let Some(value) = ctx.captures.get(name) else {
        return Err(format!("Unknown capture '{}'", name));
    };

    let transforms: Result<Vec<_>, _> = parts
        .iter()
        .skip(1)
        .map(|part| capture::parse_transform(part))
        .collect();
    capture::apply_transforms(value, &transforms?)
}

fn split_capture_pipeline(expr: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut paren_depth = 0usize;

    for ch in expr.chars() {
        match ch {
            '\'' | '"' => {
                if quote == Some(ch) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(ch);
                }
                current.push(ch);
            }
            '(' if quote.is_none() => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' if quote.is_none() => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(ch);
            }
            '|' if quote.is_none() && paren_depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    parts.push(current.trim().to_string());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_ctx(env: &[(&str, &str)], captures: &[(&str, serde_json::Value)]) -> Context {
        Context {
            env: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            captures: captures
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            optional_unset: HashSet::new(),
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
            optional_unset: HashSet::new(),
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

    #[test]
    fn interpolate_json_preserves_count_transform_number() {
        let ctx = make_ctx(&[], &[("tags", json!(["alpha", "beta", "gamma"]))]);
        let val = json!({"count": "{{ capture.tags | count }}"});
        let result = interpolate_json(&val, &ctx);
        assert_eq!(result["count"], json!(3));
        assert!(result["count"].is_number());
    }

    #[test]
    fn interpolate_json_preserves_first_transform_type() {
        let ctx = make_ctx(&[], &[("users", json!([{"id": "usr_1"}, {"id": "usr_2"}]))]);
        let val = json!({"user": "{{ capture.users | first }}"});
        let result = interpolate_json(&val, &ctx);
        assert_eq!(result["user"], json!({"id": "usr_1"}));
        assert!(result["user"].is_object());
    }

    #[test]
    fn interpolate_json_preserves_split_transform_array() {
        let ctx = make_ctx(&[], &[("body", json!("plain text response"))]);
        let val = json!({"words": "{{ capture.body | split(' ') }}"});
        let result = interpolate_json(&val, &ctx);
        assert_eq!(result["words"], json!(["plain", "text", "response"]));
        assert!(result["words"].is_array());
    }

    #[test]
    fn interpolate_json_preserves_to_int_transform_number() {
        let ctx = make_ctx(&[], &[("status_text", json!("204"))]);
        let val = json!({"status": "{{ capture.status_text | to_int }}"});
        let result = interpolate_json(&val, &ctx);
        assert_eq!(result["status"], json!(204));
        assert!(result["status"].is_number());
    }

    #[test]
    fn interpolate_join_transform_in_string_context() {
        let ctx = make_ctx(&[], &[("tags", json!(["alpha", "beta", "gamma"]))]);
        let result = interpolate("tags={{ capture.tags | join('|') }}", &ctx);
        assert_eq!(result, "tags=alpha|beta|gamma");
    }

    #[test]
    fn interpolate_replace_and_to_string_in_string_context() {
        let ctx = make_ctx(
            &[],
            &[("body", json!("plain text response")), ("code", json!(204))],
        );
        let result = interpolate(
            "body={{ capture.body | replace(' response', '') }} code={{ capture.code | to_string }}",
            &ctx,
        );
        assert_eq!(result, "body=plain text code=204");
    }

    #[test]
    fn invalid_capture_transform_is_preserved() {
        let ctx = make_ctx(&[], &[("name", json!("alice"))]);
        let result = interpolate("{{ capture.name | first }}", &ctx);
        assert_eq!(result, "{{ capture.name | first }}");
    }

    #[test]
    fn split_pipeline_ignores_pipes_inside_join_arguments() {
        assert_eq!(
            split_capture_pipeline("tags | join('|') | count"),
            vec!["tags", "join('|')", "count"]
        );
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

    #[test]
    fn interpolate_string_map_preserves_order() {
        let ctx = make_string_ctx(&[("prefix", "user"), ("value", "ok")], &[("name", "alice")]);
        let values = IndexMap::from([
            (
                "{{ env.prefix }}_name".to_string(),
                "{{ capture.name }}".to_string(),
            ),
            ("static".to_string(), "{{ env.value }}".to_string()),
        ]);

        let result = interpolate_string_map(&values, &ctx);
        let pairs: Vec<_> = result.into_iter().collect();

        assert_eq!(
            pairs,
            vec![
                ("user_name".to_string(), "alice".to_string()),
                ("static".to_string(), "ok".to_string()),
            ]
        );
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

    // --- Unresolved template detection ---

    #[test]
    fn find_unresolved_no_templates() {
        assert!(find_unresolved("plain text").is_empty());
        assert!(find_unresolved("http://localhost:3000/health").is_empty());
    }

    #[test]
    fn find_unresolved_capture() {
        let result = find_unresolved("/api/{{ capture.package_id }}/items");
        assert_eq!(result, vec!["capture.package_id"]);
    }

    #[test]
    fn find_unresolved_env() {
        let result = find_unresolved("{{ env.base_url }}/health");
        assert_eq!(result, vec!["env.base_url"]);
    }

    #[test]
    fn find_unresolved_multiple() {
        let result =
            find_unresolved("{{ env.base_url }}/{{ capture.id }}?token={{ capture.token }}");
        assert_eq!(result, vec!["env.base_url", "capture.id", "capture.token"]);
    }

    #[test]
    fn find_unresolved_in_json_nested() {
        let val = json!({
            "url": "{{ env.base_url }}/api",
            "data": {
                "id": "{{ capture.item_id }}",
                "count": 5
            },
            "tags": ["{{ capture.tag }}", "static"]
        });
        let mut result = find_unresolved_in_json(&val);
        result.sort();
        assert_eq!(
            result,
            vec!["capture.item_id", "capture.tag", "env.base_url"]
        );
    }

    #[test]
    fn find_unresolved_in_json_no_templates() {
        let val = json!({"name": "Alice", "count": 42, "active": true});
        assert!(find_unresolved_in_json(&val).is_empty());
    }

    // --- NAZ-242: classify_unresolved splits generic vs optional-unset ---

    #[test]
    fn classify_unresolved_separates_optional_unset_references() {
        let mut ctx = make_string_ctx(&[("base_url", "http://localhost")], &[]);
        ctx.optional_unset.insert("maybe".into());

        let raw = vec![
            "env.base_url".to_string(),
            "capture.maybe".to_string(),
            "capture.typo".to_string(),
        ];
        let classification = classify_unresolved(&raw, &ctx);
        assert_eq!(classification.optional_unset_refs, vec!["maybe"]);
        assert_eq!(
            classification.unresolved,
            vec!["env.base_url", "capture.typo"]
        );
    }

    #[test]
    fn classify_unresolved_sees_through_pipeline_transforms() {
        // `{{ capture.maybe | to_string }}` must still classify on the
        // base name `maybe` — transforms don't change which variable
        // was referenced.
        let mut ctx = make_string_ctx(&[], &[]);
        ctx.optional_unset.insert("maybe".into());
        let raw = vec!["capture.maybe | to_string".to_string()];
        let c = classify_unresolved(&raw, &ctx);
        assert_eq!(c.optional_unset_refs, vec!["maybe"]);
        assert!(c.unresolved.is_empty());
    }

    #[test]
    fn classify_unresolved_without_optional_returns_all_as_unresolved() {
        let ctx = make_string_ctx(&[], &[]);
        let raw = vec!["env.missing".to_string(), "capture.none".to_string()];
        let c = classify_unresolved(&raw, &ctx);
        assert!(c.optional_unset_refs.is_empty());
        assert_eq!(c.unresolved, vec!["env.missing", "capture.none"]);
    }
}
