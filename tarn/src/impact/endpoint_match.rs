//! Endpoint matching: normalize URL paths and compare `(method, path)` pairs.
//!
//! Normalization rules are boring on purpose — `tarn impact` favors precision
//! over cleverness. We strip the scheme+host prefix and the `{{ env.base_url }}`
//! placeholder, drop the query string, collapse concrete UUID / integer
//! segments to `:id` placeholders so `/users/1` and `/users/{{ capture.uid }}`
//! both match `/users/:id`, and compare literal segments otherwise.

/// A single changed endpoint parsed from `--endpoints METHOD:PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointChange {
    pub method: String,
    pub path: String,
}

impl EndpointChange {
    /// Returns the normalized path for matching.
    pub fn normalized_path(&self) -> String {
        normalize_path(&self.path)
    }

    /// Returns the uppercased method for matching.
    pub fn normalized_method(&self) -> String {
        self.method.to_ascii_uppercase()
    }
}

/// Parse a `METHOD:PATH` string. Methods are uppercased; paths are kept
/// verbatim (normalization happens at match time) so we can roundtrip them
/// into the JSON output under `inputs.endpoints` without losing the user's
/// intent.
pub fn parse_endpoint(spec: &str) -> Result<EndpointChange, String> {
    let (method, path) = spec.split_once(':').ok_or_else(|| {
        format!("invalid endpoint '{spec}': expected 'METHOD:PATH' (e.g. 'GET:/users/:id')")
    })?;
    let method = method.trim();
    let path = path.trim();
    if method.is_empty() || path.is_empty() {
        return Err(format!(
            "invalid endpoint '{spec}': both METHOD and PATH must be non-empty"
        ));
    }
    if !path.starts_with('/') {
        return Err(format!(
            "invalid endpoint '{spec}': PATH must start with '/'"
        ));
    }
    Ok(EndpointChange {
        method: method.to_ascii_uppercase(),
        path: path.to_string(),
    })
}

/// Normalize a URL path for matching:
///
/// - strip `{{ env.* }}` / `{{ capture.* }}` host prefixes
/// - strip `http(s)://host[:port]` prefixes
/// - drop the query string and fragment
/// - collapse `{id}`, `:id`, `{{ capture.x }}`, pure integers, and UUIDs into `:id`
/// - remove trailing slash (except the root)
pub fn normalize_path(input: &str) -> String {
    let without_templates = strip_leading_host_and_templates(input);
    let without_query = without_templates.split(['?', '#']).next().unwrap_or("");
    let trimmed = without_query.trim();

    let mut segments: Vec<String> = Vec::new();
    for raw in trimmed.split('/') {
        if raw.is_empty() {
            continue;
        }
        segments.push(normalize_segment(raw));
    }

    if segments.is_empty() {
        return "/".to_string();
    }
    let mut out = String::with_capacity(trimmed.len());
    for seg in &segments {
        out.push('/');
        out.push_str(seg);
    }
    out
}

fn strip_leading_host_and_templates(input: &str) -> String {
    let trimmed = input.trim_start();

    // Strip a leading `{{ ... }}` (e.g. `{{ env.base_url }}`) only when it
    // sits at the very start of the URL — mid-path handlebars are
    // path-parameter placeholders and must survive as segments so the
    // segment normalizer can collapse them to `:id`.
    if trimmed.starts_with("{{") {
        if let Some(end) = trimmed.find("}}") {
            let after = trimmed[end + 2..].to_string();
            return strip_leading_scheme_host(&after);
        }
    }
    strip_leading_scheme_host(trimmed)
}

fn strip_leading_scheme_host(s: &str) -> String {
    if let Some(idx) = s.find("://") {
        let tail = &s[idx + 3..];
        if let Some(slash) = tail.find('/') {
            return tail[slash..].to_string();
        }
        return "/".to_string();
    }
    s.to_string()
}

fn normalize_segment(seg: &str) -> String {
    // Single-segment handlebar placeholder (e.g. `{{ capture.user_id }}`)
    // should compare equal to `:id` / `{id}`.
    if contains_handlebar(seg) {
        return ":id".to_string();
    }
    // `{id}` / `{user_id}` — OpenAPI-style path parameters.
    if seg.starts_with('{') && seg.ends_with('}') && seg.len() > 2 {
        return ":id".to_string();
    }
    // `:id` — Express / Rails-style path parameters.
    if let Some(stripped) = seg.strip_prefix(':') {
        if !stripped.is_empty() {
            return ":id".to_string();
        }
    }
    if is_integer(seg) || is_uuid(seg) {
        return ":id".to_string();
    }
    seg.to_string()
}

fn contains_handlebar(s: &str) -> bool {
    s.contains("{{") && s.contains("}}")
}

fn is_integer(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_uuid(s: &str) -> bool {
    // 36-char canonical UUID (8-4-4-4-12 hex). Being strict keeps false positives low.
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        let dash_position = matches!(i, 8 | 13 | 18 | 23);
        if dash_position {
            if *b != b'-' {
                return false;
            }
        } else if !(b.is_ascii_digit() || (b'a'..=b'f').contains(b) || (b'A'..=b'F').contains(b)) {
            return false;
        }
    }
    true
}

/// Decide whether a step URL matches an endpoint change.
///
/// Returns `Some(MatchKind::Exact)` on full `(method, path)` equality after
/// normalization, `Some(MatchKind::Prefix)` when the step path starts with
/// the changed endpoint path (so `/users` catches `/users/:id/posts` but
/// `/user` does *not* catch `/users` — we only match on full segment
/// boundaries), and `None` otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    Exact,
    Prefix,
}

pub fn match_endpoint(
    step_method: &str,
    step_url: &str,
    change: &EndpointChange,
) -> Option<MatchKind> {
    let step_m = step_method.to_ascii_uppercase();
    let change_m = change.normalized_method();
    if step_m != change_m {
        return None;
    }
    let step_p = normalize_path(step_url);
    let change_p = normalize_path(&change.path);
    if step_p == change_p {
        return Some(MatchKind::Exact);
    }
    if is_segment_prefix(&change_p, &step_p) {
        return Some(MatchKind::Prefix);
    }
    None
}

fn is_segment_prefix(prefix: &str, candidate: &str) -> bool {
    if prefix == "/" || candidate == prefix {
        return false;
    }
    if !candidate.starts_with(prefix) {
        return false;
    }
    let after = &candidate[prefix.len()..];
    after.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_endpoint_accepts_method_and_path() {
        let e = parse_endpoint("GET:/users/:id").unwrap();
        assert_eq!(e.method, "GET");
        assert_eq!(e.path, "/users/:id");
    }

    #[test]
    fn parse_endpoint_uppercases_method() {
        let e = parse_endpoint("post:/users").unwrap();
        assert_eq!(e.method, "POST");
    }

    #[test]
    fn parse_endpoint_rejects_missing_colon() {
        let err = parse_endpoint("GET/users").unwrap_err();
        assert!(err.contains("METHOD:PATH"));
    }

    #[test]
    fn parse_endpoint_rejects_empty_parts() {
        let err = parse_endpoint("GET:").unwrap_err();
        assert!(err.contains("non-empty"));
    }

    #[test]
    fn parse_endpoint_rejects_relative_path() {
        let err = parse_endpoint("GET:users").unwrap_err();
        assert!(err.contains("must start with '/'"));
    }

    #[test]
    fn normalize_strips_scheme_host_and_query() {
        assert_eq!(
            normalize_path("http://example.com:3000/users/1?foo=bar"),
            "/users/:id"
        );
    }

    #[test]
    fn normalize_collapses_integer_uuid_and_openapi_params() {
        assert_eq!(normalize_path("/users/42"), "/users/:id");
        assert_eq!(
            normalize_path("/users/550e8400-e29b-41d4-a716-446655440000"),
            "/users/:id"
        );
        assert_eq!(normalize_path("/users/{id}"), "/users/:id");
        assert_eq!(normalize_path("/users/{{ capture.uid }}"), "/users/:id");
    }

    #[test]
    fn normalize_strips_handlebar_host_prefix() {
        assert_eq!(normalize_path("{{ env.base_url }}/users/1"), "/users/:id");
    }

    #[test]
    fn match_endpoint_exact_equal_paths() {
        let change = EndpointChange {
            method: "GET".into(),
            path: "/users/:id".into(),
        };
        assert_eq!(
            match_endpoint("GET", "{{ env.base_url }}/users/42", &change),
            Some(MatchKind::Exact)
        );
    }

    #[test]
    fn match_endpoint_prefix_catches_deeper_routes() {
        let change = EndpointChange {
            method: "GET".into(),
            path: "/users".into(),
        };
        assert_eq!(
            match_endpoint("GET", "{{ env.base_url }}/users/42/posts", &change),
            Some(MatchKind::Prefix)
        );
    }

    #[test]
    fn match_endpoint_rejects_different_method() {
        let change = EndpointChange {
            method: "POST".into(),
            path: "/users/:id".into(),
        };
        assert!(match_endpoint("GET", "/users/42", &change).is_none());
    }

    #[test]
    fn match_endpoint_rejects_unrelated_path() {
        let change = EndpointChange {
            method: "GET".into(),
            path: "/users/:id".into(),
        };
        assert!(match_endpoint("GET", "/posts/42", &change).is_none());
    }

    #[test]
    fn prefix_match_requires_full_segment_boundary() {
        // `/user` is NOT a segment prefix of `/users/42`.
        let change = EndpointChange {
            method: "GET".into(),
            path: "/user".into(),
        };
        assert!(match_endpoint("GET", "/users/42", &change).is_none());
    }

    #[test]
    fn prefix_match_does_not_fire_on_equal_paths() {
        // Equal paths must be Exact, not Prefix.
        let change = EndpointChange {
            method: "GET".into(),
            path: "/users".into(),
        };
        assert_eq!(
            match_endpoint("GET", "/users", &change),
            Some(MatchKind::Exact)
        );
    }
}
