use crate::error::TarnError;
use crate::interpolation::{self, Context};
use crate::model::{CaptureSpec, ExtendedCapture};
use crate::regex_cache;
use serde_json::Value;
use serde_json_path::JsonPath;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueTransform {
    First,
    Last,
    Count,
    Join(String),
    Split(String),
    Replace { from: String, to: String },
    ToInt,
    ToString,
}

pub fn parse_transform(stage: &str) -> Result<ValueTransform, String> {
    let stage = stage.trim();
    match stage {
        "first" => Ok(ValueTransform::First),
        "last" => Ok(ValueTransform::Last),
        "count" => Ok(ValueTransform::Count),
        "to_int" => Ok(ValueTransform::ToInt),
        "to_string" => Ok(ValueTransform::ToString),
        _ => {
            if let Some(args) = parse_function_args(stage, "join")? {
                return Ok(ValueTransform::Join(single_transform_arg("join", args)?));
            }
            if let Some(args) = parse_function_args(stage, "split")? {
                let delimiter = single_transform_arg("split", args)?;
                if delimiter.is_empty() {
                    return Err("Transform 'split' requires a non-empty delimiter".to_string());
                }
                return Ok(ValueTransform::Split(delimiter));
            }
            if let Some(args) = parse_function_args(stage, "replace")? {
                let [from, to] = exact_transform_args::<2>("replace", args)?;
                return Ok(ValueTransform::Replace { from, to });
            }
            Err(format!("Unknown transform '{}'", stage))
        }
    }
}

pub fn apply_transforms(value: &Value, transforms: &[ValueTransform]) -> Result<Value, String> {
    let mut current = value.clone();
    for transform in transforms {
        current = apply_transform(&current, transform)?;
    }
    Ok(current)
}

/// Borrowed view over the response fields that capture extraction can
/// read from. Bundled into a single struct so the top-level entry points
/// stay under clippy's `too_many_arguments` threshold and future response
/// dimensions (e.g. trailers) only require changes here, not at every
/// call site.
pub struct ResponseView<'a> {
    pub status: u16,
    pub url: &'a str,
    pub body: &'a Value,
    pub headers: &'a HashMap<String, String>,
    pub raw_headers: &'a [(String, String)],
}

/// Extract captures from an HTTP response using JSONPath or extended sources.
/// Returns a map of capture_name -> extracted JSON value (type-preserving).
///
/// `ctx` supplies `env` + previously captured values so that `capture.jsonpath`,
/// `capture.regex`, `capture.header`, and `capture.cookie` can reference
/// `{{ env.foo }}` or `{{ capture.bar }}` — useful when a JSONPath filter has
/// to include an id that was captured in an earlier step. Unresolved
/// placeholders (e.g. the referenced capture never succeeded) surface as a
/// regular [`TarnError::Capture`] describing the missing variable so the
/// caller sees the root cause instead of a cryptic "regex did not match".
pub fn extract_captures(
    response: &ResponseView<'_>,
    capture_map: &HashMap<String, CaptureSpec>,
    ctx: &Context,
) -> Result<HashMap<String, Value>, TarnError> {
    let mut captures = HashMap::new();

    for (name, spec) in capture_map {
        let value = extract_capture(response, name, spec, ctx)?;
        captures.insert(name.clone(), value);
    }

    Ok(captures)
}

/// Extract a single named capture while preserving the existing error messages.
pub fn extract_capture(
    response: &ResponseView<'_>,
    name: &str,
    spec: &CaptureSpec,
    ctx: &Context,
) -> Result<Value, TarnError> {
    let resolved = resolve_capture_spec(name, spec, ctx)?;
    match &resolved {
        CaptureSpec::JsonPath(path_str) => extract_jsonpath(response.body, path_str).map_err(|e| {
            TarnError::Capture(format!(
                "Failed to capture '{}' with path '{}': {}",
                name, path_str, e
            ))
        }),
        CaptureSpec::Extended(ext) => extract_extended(response, ext)
            .map_err(|e| TarnError::Capture(format!("Failed to capture '{}': {}", name, e))),
    }
}

/// Interpolate every string field of a capture spec against `ctx` and fail
/// fast if any placeholders are still unresolved after substitution. This is
/// the one place that decides capture expressions support `{{ ... }}`; keeping
/// it at the edge of extraction means the JSONPath/regex/header parsers see
/// only literal, validated input.
fn resolve_capture_spec(
    name: &str,
    spec: &CaptureSpec,
    ctx: &Context,
) -> Result<CaptureSpec, TarnError> {
    match spec {
        CaptureSpec::JsonPath(path_str) => {
            let resolved = interpolation::interpolate(path_str, ctx);
            ensure_resolved(name, "jsonpath", &resolved)?;
            Ok(CaptureSpec::JsonPath(resolved))
        }
        CaptureSpec::Extended(ext) => {
            let mut out = ext.clone();
            if let Some(ref raw) = ext.header {
                let resolved = interpolation::interpolate(raw, ctx);
                ensure_resolved(name, "header", &resolved)?;
                out.header = Some(resolved);
            }
            if let Some(ref raw) = ext.cookie {
                let resolved = interpolation::interpolate(raw, ctx);
                ensure_resolved(name, "cookie", &resolved)?;
                out.cookie = Some(resolved);
            }
            if let Some(ref raw) = ext.jsonpath {
                let resolved = interpolation::interpolate(raw, ctx);
                ensure_resolved(name, "jsonpath", &resolved)?;
                out.jsonpath = Some(resolved);
            }
            if let Some(ref raw) = ext.regex {
                let resolved = interpolation::interpolate(raw, ctx);
                ensure_resolved(name, "regex", &resolved)?;
                out.regex = Some(resolved);
            }
            if let Some(ref raw) = ext.where_predicate {
                let resolved = interpolate_yaml(raw, ctx);
                ensure_resolved_yaml(name, "where", &resolved)?;
                out.where_predicate = Some(resolved);
            }
            Ok(CaptureSpec::Extended(out))
        }
    }
}

/// Recursively interpolate all string leaves of a YAML value against `ctx`.
/// Used for the capture `where:` predicate, whose field values may
/// themselves be `{{ capture.x }}` references that should resolve before
/// the predicate is compared against response objects.
fn interpolate_yaml(value: &serde_yaml::Value, ctx: &Context) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::String(s) => {
            serde_yaml::Value::String(interpolation::interpolate(s, ctx))
        }
        serde_yaml::Value::Sequence(seq) => {
            serde_yaml::Value::Sequence(seq.iter().map(|v| interpolate_yaml(v, ctx)).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let mut out = serde_yaml::Mapping::with_capacity(map.len());
            for (k, v) in map {
                out.insert(interpolate_yaml(k, ctx), interpolate_yaml(v, ctx));
            }
            serde_yaml::Value::Mapping(out)
        }
        other => other.clone(),
    }
}

fn ensure_resolved_yaml(
    name: &str,
    field: &str,
    value: &serde_yaml::Value,
) -> Result<(), TarnError> {
    let mut remaining: Vec<String> = Vec::new();
    collect_unresolved_yaml(value, &mut remaining);
    if remaining.is_empty() {
        Ok(())
    } else {
        remaining.sort();
        remaining.dedup();
        Err(TarnError::Capture(format!(
            "Failed to capture '{}': unresolved template variable(s) in {} predicate: {}. \
             Check that prior captures succeeded and env vars are set.",
            name,
            field,
            remaining.join(", ")
        )))
    }
}

fn collect_unresolved_yaml(value: &serde_yaml::Value, out: &mut Vec<String>) {
    match value {
        serde_yaml::Value::String(s) => {
            out.extend(interpolation::find_unresolved(s));
        }
        serde_yaml::Value::Sequence(seq) => {
            for v in seq {
                collect_unresolved_yaml(v, out);
            }
        }
        serde_yaml::Value::Mapping(map) => {
            for (_, v) in map {
                collect_unresolved_yaml(v, out);
            }
        }
        _ => {}
    }
}

fn ensure_resolved(name: &str, field: &str, value: &str) -> Result<(), TarnError> {
    let remaining = interpolation::find_unresolved(value);
    if remaining.is_empty() {
        Ok(())
    } else {
        let mut names = remaining;
        names.sort();
        names.dedup();
        Err(TarnError::Capture(format!(
            "Failed to capture '{}': unresolved template variable(s) in {} expression '{}': {}. \
             Check that prior captures succeeded and env vars are set.",
            name,
            field,
            value,
            names.join(", ")
        )))
    }
}

/// Extract a value using an extended capture spec.
fn extract_extended(response: &ResponseView<'_>, ext: &ExtendedCapture) -> Result<Value, String> {
    let source = if let Some(ref header_name) = ext.header {
        extract_header_source(
            response.headers,
            response.raw_headers,
            header_name,
            ext.regex.as_deref(),
        )?
    } else if let Some(ref cookie_name) = ext.cookie {
        extract_cookie_source(response.raw_headers, cookie_name)?
    } else if let Some(ref jsonpath) = ext.jsonpath {
        let raw = extract_jsonpath(response.body, jsonpath)?;
        // `where:` filters the array that `jsonpath` returns, turning
        // "capture every user" + "`first` transform" into "capture the
        // user whose id is X". Users who intentionally want to pick by
        // index can still do so; `where:` is only active when set.
        match &ext.where_predicate {
            Some(predicate) => apply_where_filter(&raw, predicate)?,
            None => raw,
        }
    } else if ext.body.unwrap_or(false) {
        Value::String(value_to_string(response.body))
    } else if ext.status.unwrap_or(false) {
        Value::Number(response.status.into())
    } else if ext.url.unwrap_or(false) {
        Value::String(response.url.to_string())
    } else {
        return Err(
            "Extended capture must specify either 'header', 'cookie', 'jsonpath', 'body', 'status', or 'url' as the source".to_string(),
        );
    };

    if ext.header.is_some() && ext.regex.is_some() {
        Ok(source)
    } else if let Some(ref regex_str) = ext.regex {
        match_regex(regex_str, &value_to_string(&source))
    } else {
        Ok(source)
    }
}

/// Apply a `where:` predicate to a JSONPath result, keeping only the
/// elements that match. If the source is a single object it's treated
/// as a one-element array for uniformity. The result is always an array
/// of the matching elements, so callers can chain `| first` to pick a
/// single identifier-matched record without relying on `$[0]`.
fn apply_where_filter(source: &Value, predicate: &serde_yaml::Value) -> Result<Value, String> {
    let predicate_map = match predicate {
        serde_yaml::Value::Mapping(m) => m,
        _ => {
            return Err(
                "Capture `where` clause must be an object predicate (field: value pairs)"
                    .to_string(),
            );
        }
    };

    let items: Vec<Value> = match source {
        Value::Array(arr) => arr.clone(),
        Value::Object(_) => vec![source.clone()],
        other => {
            return Err(format!(
                "Capture `where` clause requires an array or object at the JSONPath, got {}",
                match other {
                    Value::Null => "null",
                    Value::Bool(_) => "boolean",
                    Value::Number(_) => "number",
                    Value::String(_) => "string",
                    _ => "unknown",
                }
            ));
        }
    };

    let filtered: Vec<Value> = items
        .into_iter()
        .filter(|item| crate::assert::body::object_matches_predicate(item, predicate_map))
        .collect();

    Ok(Value::Array(filtered))
}

fn extract_header_source(
    headers: &HashMap<String, String>,
    raw_headers: &[(String, String)],
    header_name: &str,
    regex: Option<&str>,
) -> Result<Value, String> {
    if let Some(regex_str) = regex {
        let values: Vec<&str> = raw_headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(header_name))
            .map(|(_, value)| value.as_str())
            .collect();

        if values.is_empty() {
            if let Some(value) = headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(header_name))
                .map(|(_, value)| value)
            {
                return match_regex(regex_str, value);
            }
            return Err(missing_header_message(headers, header_name));
        }

        for value in &values {
            if let Ok(matched) = match_regex(regex_str, value) {
                return Ok(matched);
            }
        }

        return Err(format!(
            "Regex '{}' did not match any '{}' header values",
            regex_str, header_name
        ));
    }

    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(header_name))
        .map(|(_, v)| Value::String(v.clone()))
        .ok_or_else(|| missing_header_message(headers, header_name))
}

fn extract_cookie_source(
    raw_headers: &[(String, String)],
    cookie_name: &str,
) -> Result<Value, String> {
    let mut available = Vec::new();

    for (header_name, header_value) in raw_headers {
        if !header_name.eq_ignore_ascii_case("set-cookie") {
            continue;
        }
        let Some((name, value)) = parse_set_cookie_header(header_value) else {
            continue;
        };
        available.push(name.to_string());
        if name.eq_ignore_ascii_case(cookie_name) {
            return Ok(Value::String(value.to_string()));
        }
    }

    Err(format!(
        "Cookie '{}' not found in Set-Cookie headers. Available: {}",
        cookie_name,
        if available.is_empty() {
            "(none)".to_string()
        } else {
            available.join(", ")
        }
    ))
}

fn parse_set_cookie_header(header_value: &str) -> Option<(&str, &str)> {
    let first = header_value.split(';').next()?.trim();
    let (name, value) = first.split_once('=')?;
    Some((name.trim(), value.trim()))
}

fn missing_header_message(headers: &HashMap<String, String>, header_name: &str) -> String {
    let available: Vec<&str> = headers.keys().map(|k| k.as_str()).collect();
    format!(
        "Header '{}' not found in response. Available: {}",
        header_name,
        if available.is_empty() {
            "(none)".to_string()
        } else {
            available.join(", ")
        }
    )
}

fn match_regex(regex_str: &str, source: &str) -> Result<Value, String> {
    let re =
        regex_cache::get(regex_str).map_err(|e| format!("Invalid regex '{}': {}", regex_str, e))?;
    match re.captures(source) {
        Some(caps) => {
            let matched = caps
                .get(1)
                .or_else(|| caps.get(0))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            Ok(Value::String(matched))
        }
        None => Err(format!(
            "Regex '{}' did not match value '{}'",
            regex_str, source
        )),
    }
}

fn apply_transform(value: &Value, transform: &ValueTransform) -> Result<Value, String> {
    match transform {
        ValueTransform::First => match value {
            Value::Array(items) => items
                .first()
                .cloned()
                .ok_or_else(|| "Transform 'first' requires a non-empty array".to_string()),
            other => Err(format!(
                "Transform 'first' requires an array, got {}",
                value_kind(other)
            )),
        },
        ValueTransform::Last => match value {
            Value::Array(items) => items
                .last()
                .cloned()
                .ok_or_else(|| "Transform 'last' requires a non-empty array".to_string()),
            other => Err(format!(
                "Transform 'last' requires an array, got {}",
                value_kind(other)
            )),
        },
        ValueTransform::Count => {
            let count = match value {
                Value::Array(items) => items.len() as u64,
                Value::Object(map) => map.len() as u64,
                Value::String(text) => text.chars().count() as u64,
                other => {
                    return Err(format!(
                        "Transform 'count' requires an array, object, or string, got {}",
                        value_kind(other)
                    ));
                }
            };
            Ok(Value::Number(count.into()))
        }
        ValueTransform::Join(delimiter) => match value {
            Value::Array(items) => Ok(Value::String(
                items
                    .iter()
                    .map(value_to_string)
                    .collect::<Vec<_>>()
                    .join(delimiter),
            )),
            other => Err(format!(
                "Transform 'join' requires an array, got {}",
                value_kind(other)
            )),
        },
        ValueTransform::Split(delimiter) => match value {
            Value::String(text) => Ok(Value::Array(
                text.split(delimiter)
                    .map(|part| Value::String(part.to_string()))
                    .collect(),
            )),
            other => Err(format!(
                "Transform 'split' requires a string, got {}",
                value_kind(other)
            )),
        },
        ValueTransform::Replace { from, to } => match value {
            Value::String(text) => Ok(Value::String(text.replace(from, to))),
            other => Err(format!(
                "Transform 'replace' requires a string, got {}",
                value_kind(other)
            )),
        },
        ValueTransform::ToInt => match value {
            Value::String(text) => {
                let parsed = text.trim().parse::<i64>().map_err(|_| {
                    format!(
                        "Transform 'to_int' could not parse '{}' as an integer",
                        text
                    )
                })?;
                Ok(Value::Number(parsed.into()))
            }
            Value::Number(number) => {
                if let Some(value) = number.as_i64() {
                    Ok(Value::Number(value.into()))
                } else if let Some(value) = number.as_u64() {
                    Ok(Value::Number(value.into()))
                } else {
                    Err(
                        "Transform 'to_int' requires an integer-compatible string or number"
                            .to_string(),
                    )
                }
            }
            other => Err(format!(
                "Transform 'to_int' requires a string or number, got {}",
                value_kind(other)
            )),
        },
        ValueTransform::ToString => Ok(Value::String(value_to_string(value))),
    }
}

fn parse_transform_arg(arg: &str) -> String {
    let trimmed = arg.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.chars().next().unwrap();
        let last = trimmed.chars().last().unwrap();
        if (first == '\'' && last == '\'') || (first == '"' && last == '"') {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

fn parse_function_args(stage: &str, name: &str) -> Result<Option<Vec<String>>, String> {
    let Some(inner) = stage
        .strip_prefix(&format!("{name}("))
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return Ok(None);
    };

    Ok(Some(split_function_args(inner)))
}

fn split_function_args(args: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = None;

    for ch in args.chars() {
        match ch {
            '\'' | '"' => {
                if quote == Some(ch) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(ch);
                }
                current.push(ch);
            }
            ',' if quote.is_none() => {
                parts.push(parse_transform_arg(&current));
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    parts.push(parse_transform_arg(&current));
    parts
}

fn single_transform_arg(name: &str, args: Vec<String>) -> Result<String, String> {
    let [value] = exact_transform_args::<1>(name, args)?;
    Ok(value)
}

fn exact_transform_args<const N: usize>(
    name: &str,
    args: Vec<String>,
) -> Result<[String; N], String> {
    let actual = args.len();
    args.try_into().map_err(|_| {
        format!(
            "Transform '{}' expects {} argument{}, got {}",
            name,
            N,
            if N == 1 { "" } else { "s" },
            actual
        )
    })
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Extract a single value via JSONPath from a JSON body.
/// Returns the JSON value directly (type-preserving).
fn extract_jsonpath(body: &Value, path_str: &str) -> Result<Value, String> {
    let json_path =
        JsonPath::parse(path_str).map_err(|e| format!("Invalid JSONPath '{}': {}", path_str, e))?;

    let node_list = json_path.query(body);
    let nodes: Vec<&Value> = node_list.all();

    if nodes.is_empty() {
        let hint = suggest_jsonpath_fix(body, path_str);
        return Err(format!(
            "JSONPath '{}' matched no values in response body{}",
            path_str, hint
        ));
    }

    // Take the first match — preserve the original type
    Ok(nodes[0].clone())
}

/// Suggest fixes when a JSONPath doesn't match.
fn suggest_jsonpath_fix(body: &Value, path_str: &str) -> String {
    // Extract the first key from the path (e.g., "$.users" -> "users")
    let first_key = path_str
        .strip_prefix("$.")
        .and_then(|rest| rest.split('.').next())
        .and_then(|k| k.split('[').next());

    if let (Some(key), Some(obj)) = (first_key, body.as_object()) {
        let available: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        if available.is_empty() {
            return ". Response body is an empty object.".to_string();
        }

        // Check for close matches
        for avail_key in &available {
            if avail_key.eq_ignore_ascii_case(key) && *avail_key != key {
                return format!(". Did you mean `$.{}`? (case mismatch)", avail_key);
            }
        }

        // Show available keys (up to 10)
        let shown: Vec<&str> = available.iter().take(10).copied().collect();
        format!(". Available keys: {}", shown.join(", "))
    } else if body.is_array() {
        let len = body.as_array().map(|a| a.len()).unwrap_or(0);
        format!(
            ". Response body is an array with {} elements. Use $[0] to access elements.",
            len
        )
    } else {
        String::new()
    }
}

/// Convert a JSON value to a string representation.
pub fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        // Arrays and objects are serialized as JSON strings
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw_headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn extract_string_field() {
        let body = json!({"name": "Alice"});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("user_name".into(), CaptureSpec::JsonPath("$.name".into()));

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("user_name").unwrap(), &json!("Alice"));
    }

    #[test]
    fn extract_number_field_preserves_type() {
        let body = json!({"age": 30});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("user_age".into(), CaptureSpec::JsonPath("$.age".into()));

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("user_age").unwrap(), &json!(30));
    }

    #[test]
    fn extract_boolean_field_preserves_type() {
        let body = json!({"active": true});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("is_active".into(), CaptureSpec::JsonPath("$.active".into()));

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("is_active").unwrap(), &json!(true));
    }

    #[test]
    fn extract_null_field() {
        let body = json!({"deleted": null});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("deleted".into(), CaptureSpec::JsonPath("$.deleted".into()));

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("deleted").unwrap(), &json!(null));
    }

    #[test]
    fn extract_nested_field() {
        let body = json!({"user": {"profile": {"email": "alice@test.com"}}});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "email".into(),
            CaptureSpec::JsonPath("$.user.profile.email".into()),
        );

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("email").unwrap(), &json!("alice@test.com"));
    }

    #[test]
    fn extract_array_element() {
        let body = json!({"items": [{"id": "first"}, {"id": "second"}]});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "first_id".into(),
            CaptureSpec::JsonPath("$.items[0].id".into()),
        );

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("first_id").unwrap(), &json!("first"));
    }

    #[test]
    fn extract_missing_path_returns_error() {
        let body = json!({"name": "Alice"});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "missing".into(),
            CaptureSpec::JsonPath("$.nonexistent".into()),
        );

        let result = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("matched no values"));
    }

    #[test]
    fn extract_invalid_jsonpath_returns_error() {
        let body = json!({"name": "Alice"});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("bad".into(), CaptureSpec::JsonPath("$[invalid".into()));

        let result = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn extract_multiple_captures() {
        let body = json!({"id": "usr_123", "token": "abc", "status": 200});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("id".into(), CaptureSpec::JsonPath("$.id".into()));
        map.insert("tok".into(), CaptureSpec::JsonPath("$.token".into()));
        map.insert("code".into(), CaptureSpec::JsonPath("$.status".into()));

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.len(), 3);
        assert_eq!(captures.get("id").unwrap(), &json!("usr_123"));
        assert_eq!(captures.get("tok").unwrap(), &json!("abc"));
        assert_eq!(captures.get("code").unwrap(), &json!(200));
    }

    #[test]
    fn extract_array_value() {
        let body = json!({"tags": ["a", "b"]});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert("tags".into(), CaptureSpec::JsonPath("$.tags".into()));

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("tags").unwrap(), &json!(["a", "b"]));
    }

    #[test]
    fn value_to_string_object() {
        let val = json!({"key": "value"});
        assert_eq!(value_to_string(&val), "{\"key\":\"value\"}");
    }

    #[test]
    fn parse_join_transform_accepts_quoted_delimiter() {
        assert_eq!(
            parse_transform(r#"join(", ")"#).unwrap(),
            ValueTransform::Join(", ".to_string())
        );
        assert_eq!(
            parse_transform("join('|')").unwrap(),
            ValueTransform::Join("|".to_string())
        );
    }

    #[test]
    fn parse_split_and_replace_transforms() {
        assert_eq!(
            parse_transform("split('|')").unwrap(),
            ValueTransform::Split("|".to_string())
        );
        assert_eq!(
            parse_transform("replace('plain', 'clean')").unwrap(),
            ValueTransform::Replace {
                from: "plain".to_string(),
                to: "clean".to_string()
            }
        );
        assert_eq!(parse_transform("to_int").unwrap(), ValueTransform::ToInt);
        assert_eq!(
            parse_transform("to_string").unwrap(),
            ValueTransform::ToString
        );
    }

    #[test]
    fn apply_first_last_count_and_join_transforms() {
        let users = json!([
            {"id": "usr_1"},
            {"id": "usr_2"},
            {"id": "usr_3"}
        ]);
        assert_eq!(
            apply_transforms(&users, &[ValueTransform::First]).unwrap(),
            json!({"id": "usr_1"})
        );
        assert_eq!(
            apply_transforms(&users, &[ValueTransform::Last]).unwrap(),
            json!({"id": "usr_3"})
        );
        assert_eq!(
            apply_transforms(&users, &[ValueTransform::Count]).unwrap(),
            json!(3)
        );

        let tags = json!(["alpha", "beta", "gamma"]);
        assert_eq!(
            apply_transforms(&tags, &[ValueTransform::Join("|".to_string())]).unwrap(),
            json!("alpha|beta|gamma")
        );
    }

    #[test]
    fn apply_split_replace_to_int_and_to_string_transforms() {
        assert_eq!(
            apply_transforms(
                &json!("plain text response"),
                &[ValueTransform::Split(" ".to_string())]
            )
            .unwrap(),
            json!(["plain", "text", "response"])
        );
        assert_eq!(
            apply_transforms(
                &json!("plain text response"),
                &[ValueTransform::Replace {
                    from: " response".to_string(),
                    to: "".to_string()
                }]
            )
            .unwrap(),
            json!("plain text")
        );
        assert_eq!(
            apply_transforms(&json!("204"), &[ValueTransform::ToInt]).unwrap(),
            json!(204)
        );
        assert_eq!(
            apply_transforms(&json!({"id": "usr_1"}), &[ValueTransform::ToString]).unwrap(),
            json!("{\"id\":\"usr_1\"}")
        );
    }

    #[test]
    fn apply_transform_pipeline_runs_in_order() {
        let users = json!([
            {"id": "usr_1"},
            {"id": "usr_2"}
        ]);
        assert_eq!(
            apply_transforms(
                &users,
                &[ValueTransform::First, ValueTransform::Join("|".to_string())]
            )
            .unwrap_err(),
            "Transform 'join' requires an array, got object"
        );
    }

    #[test]
    fn apply_first_transform_rejects_non_arrays() {
        let err = apply_transforms(&json!("abc"), &[ValueTransform::First]).unwrap_err();
        assert_eq!(err, "Transform 'first' requires an array, got string");
    }

    #[test]
    fn split_requires_non_empty_delimiter() {
        let err = parse_transform("split('')").unwrap_err();
        assert_eq!(err, "Transform 'split' requires a non-empty delimiter");
    }

    #[test]
    fn replace_requires_two_arguments() {
        let err = parse_transform("replace('only-one')").unwrap_err();
        assert_eq!(err, "Transform 'replace' expects 2 arguments, got 1");
    }

    #[test]
    fn to_int_rejects_non_integer_strings() {
        let err = apply_transforms(&json!("20.5"), &[ValueTransform::ToInt]).unwrap_err();
        assert_eq!(
            err,
            "Transform 'to_int' could not parse '20.5' as an integer"
        );
    }

    #[test]
    fn empty_capture_map() {
        let body = json!({"name": "Alice"});
        let headers = HashMap::new();
        let map = HashMap::new();
        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert!(captures.is_empty());
    }

    // --- Header capture tests ---

    #[test]
    fn capture_from_header() {
        let body = json!({});
        let mut headers = HashMap::new();
        headers.insert(
            "set-cookie".to_string(),
            "session=abc123; Path=/; HttpOnly".to_string(),
        );

        let mut map = HashMap::new();
        map.insert(
            "session".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: Some("set-cookie".to_string()),
                cookie: None,
                jsonpath: None,
                body: None,
                status: None,
                url: None,
                regex: Some("session=([^;]+)".to_string()),
                where_predicate: None,
            }),
        );
        let raw_headers = raw_headers(&[("set-cookie", "session=abc123; Path=/; HttpOnly")]);

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &raw_headers,
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("session").unwrap(), &json!("abc123"));
    }

    #[test]
    fn capture_from_header_without_regex() {
        let body = json!({});
        let mut headers = HashMap::new();
        headers.insert("x-request-id".to_string(), "req-12345".to_string());

        let mut map = HashMap::new();
        map.insert(
            "req_id".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: Some("x-request-id".to_string()),
                cookie: None,
                jsonpath: None,
                body: None,
                status: None,
                url: None,
                regex: None,
                where_predicate: None,
            }),
        );
        let raw_headers = raw_headers(&[("x-request-id", "req-12345")]);

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &raw_headers,
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("req_id").unwrap(), &json!("req-12345"));
    }

    #[test]
    fn capture_from_header_case_insensitive() {
        let body = json!({});
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let mut map = HashMap::new();
        map.insert(
            "ct".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: Some("content-type".to_string()),
                cookie: None,
                jsonpath: None,
                body: None,
                status: None,
                url: None,
                regex: None,
                where_predicate: None,
            }),
        );
        let raw_headers = raw_headers(&[("Content-Type", "application/json")]);

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &raw_headers,
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("ct").unwrap(), &json!("application/json"));
    }

    #[test]
    fn capture_from_missing_header_fails() {
        let body = json!({});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "missing".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: Some("x-nonexistent".to_string()),
                cookie: None,
                jsonpath: None,
                body: None,
                status: None,
                url: None,
                regex: None,
                where_predicate: None,
            }),
        );

        let result = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn capture_from_header_regex_no_match_fails() {
        let body = json!({});
        let mut headers = HashMap::new();
        headers.insert("set-cookie".to_string(), "other=value".to_string());

        let mut map = HashMap::new();
        map.insert(
            "session".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: Some("set-cookie".to_string()),
                cookie: None,
                jsonpath: None,
                body: None,
                status: None,
                url: None,
                regex: Some("session=([^;]+)".to_string()),
                where_predicate: None,
            }),
        );
        let raw_headers = raw_headers(&[
            ("set-cookie", "other=value"),
            ("set-cookie", "area=dashboard"),
        ]);

        let result = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &raw_headers,
            },
            &map,
            &Context::new(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("did not match"));
    }

    #[test]
    fn capture_jsonpath_with_regex() {
        let body = json!({"message": "User created with ID: usr_42"});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "user_id".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: None,
                cookie: None,
                jsonpath: Some("$.message".to_string()),
                body: None,
                status: None,
                url: None,
                regex: Some("ID: (\\w+)".to_string()),
                where_predicate: None,
            }),
        );

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("user_id").unwrap(), &json!("usr_42"));
    }

    #[test]
    fn extract_single_capture_matches_map_behavior() {
        let body = json!({"token": "abc123"});
        let headers = HashMap::new();
        let spec = CaptureSpec::JsonPath("$.token".into());

        let value = extract_capture(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            "token",
            &spec,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(value, json!("abc123"));
    }

    #[test]
    fn capture_from_status_preserves_number_type() {
        let body = json!({});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "status_code".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: None,
                cookie: None,
                jsonpath: None,
                body: None,
                status: Some(true),
                url: None,
                regex: None,
                where_predicate: None,
            }),
        );

        let captures = extract_captures(
            &ResponseView {
                status: 204,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("status_code").unwrap(), &json!(204));
    }

    #[test]
    fn capture_from_status_supports_regex() {
        let body = json!({});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "status_class".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: None,
                cookie: None,
                jsonpath: None,
                body: None,
                status: Some(true),
                url: None,
                regex: Some("^(\\d)".to_string()),
                where_predicate: None,
            }),
        );

        let captures = extract_captures(
            &ResponseView {
                status: 204,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("status_class").unwrap(), &json!("2"));
    }

    #[test]
    fn capture_from_final_url_returns_string() {
        let body = json!({});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "final_url".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: None,
                cookie: None,
                jsonpath: None,
                body: None,
                status: None,
                url: Some(true),
                regex: None,
                where_predicate: None,
            }),
        );

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/health",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(
            captures.get("final_url").unwrap(),
            &json!("http://example.com/health")
        );
    }

    #[test]
    fn capture_from_final_url_supports_regex() {
        let body = json!({});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "final_path".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: None,
                cookie: None,
                jsonpath: None,
                body: None,
                status: None,
                url: Some(true),
                regex: Some("https?://[^/]+(/.+)$".to_string()),
                where_predicate: None,
            }),
        );

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "https://example.com/redirected/path",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(
            captures.get("final_path").unwrap(),
            &json!("/redirected/path")
        );
    }

    #[test]
    fn capture_from_cookie_reads_set_cookie_headers() {
        let body = json!({});
        let headers = HashMap::new();
        let raw_headers = raw_headers(&[
            ("set-cookie", "session=abc123; Path=/; HttpOnly"),
            ("set-cookie", "area=dashboard; Path=/cookies/area"),
        ]);
        let mut map = HashMap::new();
        map.insert(
            "session_cookie".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: None,
                cookie: Some("session".to_string()),
                jsonpath: None,
                body: None,
                status: None,
                url: None,
                regex: None,
                where_predicate: None,
            }),
        );

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &raw_headers,
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("session_cookie").unwrap(), &json!("abc123"));
    }

    #[test]
    fn capture_from_cookie_reports_available_cookie_names() {
        let body = json!({});
        let headers = HashMap::new();
        let raw_headers = raw_headers(&[
            ("set-cookie", "session=abc123; Path=/; HttpOnly"),
            ("set-cookie", "area=dashboard; Path=/cookies/area"),
        ]);
        let mut map = HashMap::new();
        map.insert(
            "missing_cookie".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: None,
                cookie: Some("csrf".to_string()),
                jsonpath: None,
                body: None,
                status: None,
                url: None,
                regex: None,
                where_predicate: None,
            }),
        );

        let err = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &raw_headers,
            },
            &map,
            &Context::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Cookie 'csrf' not found"));
        assert!(err.contains("session"));
        assert!(err.contains("area"));
    }

    #[test]
    fn capture_from_body_with_regex_uses_whole_body_string() {
        let body = json!("plain text response");
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "body_word".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: None,
                cookie: None,
                jsonpath: None,
                body: Some(true),
                status: None,
                url: None,
                regex: Some("plain (text)".to_string()),
                where_predicate: None,
            }),
        );

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(captures.get("body_word").unwrap(), &json!("text"));
    }

    #[test]
    fn jsonpath_capture_interpolates_prior_capture_in_filter() {
        // Classic integration pattern: capture id from list endpoint and
        // reuse it inside a filter on a different endpoint. Without
        // interpolation the author has to hand-write a regex fallback.
        let body = json!({
            "items": [
                {"id": "abc-1", "label": "one"},
                {"id": "xyz-2", "label": "two"},
            ]
        });
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "matched_label".into(),
            CaptureSpec::JsonPath("$.items[?(@.id == '{{ capture.target_id }}')].label".into()),
        );

        let mut ctx = Context::new();
        ctx.captures
            .insert("target_id".into(), serde_json::json!("xyz-2"));

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/list",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &ctx,
        )
        .unwrap();
        assert_eq!(captures.get("matched_label").unwrap(), &json!("two"));
    }

    #[test]
    fn extended_capture_interpolates_header_and_regex() {
        let body = json!({});
        let mut headers = HashMap::new();
        headers.insert("X-Request-Id".into(), "req-42".into());
        let raw_headers = vec![("X-Request-Id".to_string(), "req-42".to_string())];

        let mut map = HashMap::new();
        map.insert(
            "rid".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: Some("{{ env.request_id_header }}".into()),
                cookie: None,
                jsonpath: None,
                body: None,
                status: None,
                url: None,
                regex: Some("{{ env.id_prefix }}-(.+)".into()),
                where_predicate: None,
            }),
        );

        let mut ctx = Context::new();
        ctx.env
            .insert("request_id_header".into(), "X-Request-Id".into());
        ctx.env.insert("id_prefix".into(), "req".into());

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &raw_headers,
            },
            &map,
            &ctx,
        )
        .unwrap();
        assert_eq!(captures.get("rid").unwrap(), &json!("42"));
    }

    #[test]
    fn capture_where_filters_array_by_field_predicate() {
        // Identity-based selection: pick the matching user without
        // relying on array position. Combined with `first` in a capture
        // chain this replaces brittle `$[0]` captures on shared
        // endpoints.
        let body = json!({
            "users": [
                {"id": "a", "role": "user"},
                {"id": "b", "role": "admin"},
                {"id": "c", "role": "admin"},
            ]
        });
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "admins".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: None,
                cookie: None,
                jsonpath: Some("$.users".to_string()),
                body: None,
                status: None,
                url: None,
                regex: None,
                where_predicate: Some(serde_yaml::from_str("role: admin").unwrap()),
            }),
        );

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/list",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap();
        assert_eq!(
            captures.get("admins").unwrap(),
            &json!([{"id": "b", "role": "admin"}, {"id": "c", "role": "admin"}])
        );
    }

    #[test]
    fn capture_where_interpolates_predicate_values() {
        let body = json!({
            "items": [
                {"id": "abc", "label": "one"},
                {"id": "xyz", "label": "two"},
            ]
        });
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "picked".into(),
            CaptureSpec::Extended(ExtendedCapture {
                header: None,
                cookie: None,
                jsonpath: Some("$.items".to_string()),
                body: None,
                status: None,
                url: None,
                regex: None,
                where_predicate: Some(
                    serde_yaml::from_str("id: '{{ capture.target_id }}'").unwrap(),
                ),
            }),
        );

        let mut ctx = Context::new();
        ctx.captures
            .insert("target_id".into(), serde_json::json!("xyz"));

        let captures = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/list",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &ctx,
        )
        .unwrap();
        assert_eq!(
            captures.get("picked").unwrap(),
            &json!([{"id": "xyz", "label": "two"}])
        );
    }

    #[test]
    fn capture_fails_fast_on_unresolved_template() {
        // If a capture spec references something that was never set, the
        // extractor must say so up-front. Prior behavior tried to use the
        // literal `{{ ... }}` as a JSONPath, producing a cryptic "invalid
        // JSONPath" message that hid the real root cause.
        let body = json!({"items": []});
        let headers = HashMap::new();
        let mut map = HashMap::new();
        map.insert(
            "x".into(),
            CaptureSpec::JsonPath("$.items[?(@.id == '{{ capture.missing }}')].id".into()),
        );

        let err = extract_captures(
            &ResponseView {
                status: 200,
                url: "http://example.com/final",
                body: &body,
                headers: &headers,
                raw_headers: &[],
            },
            &map,
            &Context::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("unresolved template variable(s)") && err.contains("capture.missing"),
            "expected unresolved-template error, got {err}"
        );
    }
}
