//! TL001 — positional capture on a shared list endpoint.
//!
//! Catches tests that extract a value by array index from a GET that
//! returns the whole collection, e.g. `$.[0].id` on `/users`. Element
//! zero depends on sort order and the state other tests leave behind,
//! so the moment a parallel run creates another user the capture
//! starts pointing at the wrong row.
//!
//! The heuristic is conservative: we only fire when the URL *looks
//! like* a shared list endpoint — no path-parameter segment (`:id`,
//! `/{id}`), no UUID-like literal, and no filter-style query
//! parameter. That keeps the rule quiet on lookups that already pin
//! the record (e.g. `/users?email=admin@test.com`), which are
//! positional-safe by construction.

use crate::lint::{capture_jsonpath, finding_from_step, walk_steps, Finding, Severity};
use crate::model::TestFile;

pub fn lint(file: &TestFile, path: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (step_path, step) in walk_steps(file, path) {
        let Some(request) = step.request.as_ref() else {
            continue;
        };
        for (name, spec) in &step.capture {
            let Some(jsonpath) = capture_jsonpath(spec) else {
                continue;
            };
            if !is_positional_index_path(jsonpath) {
                continue;
            }
            if !url_looks_like_shared_list(&request.url) {
                continue;
            }
            findings.push(finding_from_step(
                "TL001",
                Severity::Warning,
                path,
                Some(step_path.clone()),
                step,
                format!(
                    "Capture `{}` uses positional index `{}` on what looks like a shared list endpoint ({}).",
                    name, jsonpath, request.url
                ),
                Some(
                    "Capturing from element 0 of a shared list depends on sort order and state from other tests. Filter the request or match by a stable attribute (e.g. `?email=...` or JSONPath predicate).".to_string(),
                ),
            ));
        }
    }
    findings
}

/// Detect `$[0]`, `$.items[0].x`, etc. We deliberately do NOT flag
/// `$[*]` (wildcard) or `$[?(…)]` (filter predicate) — those are
/// identity-based selections, not positional ones.
pub(crate) fn is_positional_index_path(path: &str) -> bool {
    let trimmed = path.trim();
    for (i, c) in trimmed.char_indices() {
        if c != '[' {
            continue;
        }
        // Look ahead for the contents up to the matching `]`. If every
        // character between is an ASCII digit, it's a positional index.
        let rest = &trimmed[i + 1..];
        if let Some(end) = rest.find(']') {
            let inner = &rest[..end];
            if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()) {
                return true;
            }
        }
    }
    false
}

/// Heuristic for "this URL addresses an entire collection, not a
/// specific record". We strip any query string before reasoning about
/// path segments, then check for any of:
///
/// - a `:id` / `{id}` placeholder segment (parameterized path)
/// - a UUID-looking literal segment
/// - a filter-style query parameter (`filter=`, `name=`, `email=`,
///   `where=`, `q=`, `search=`, or an `id=` query)
///
/// Absence of all of those → probably a shared list.
pub(crate) fn url_looks_like_shared_list(url: &str) -> bool {
    // Templated values (`{{ env.base_url }}`) are fine and don't
    // themselves count as filters. Strip them so they don't produce
    // false negatives (e.g. a URL like `{{ x }}/users` would otherwise
    // contain `{` and look parameterized).
    let without_templates = strip_templates(url);
    let (path, query) = split_url(&without_templates);

    // Parameterized path? Then it's a specific record, not a list.
    for segment in path.split('/') {
        if segment.is_empty() {
            continue;
        }
        if segment.starts_with(':') || (segment.starts_with('{') && segment.ends_with('}')) {
            return false;
        }
        if looks_like_uuid(segment) {
            return false;
        }
    }

    // Filter-style query params narrow the result set — not a shared
    // list in the brittle sense.
    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some(key) = pair.split('=').next() {
                let key = key.trim().to_ascii_lowercase();
                const FILTER_KEYS: &[&str] =
                    &["filter", "name", "email", "where", "q", "search", "id"];
                if FILTER_KEYS.contains(&key.as_str()) {
                    return false;
                }
            }
        }
    }

    true
}

fn strip_templates(url: &str) -> String {
    // Replace every `{{ ... }}` block with an empty string. Cheap
    // state machine — no regex needed.
    let mut out = String::with_capacity(url.len());
    let bytes = url.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Skip to closing `}}` — if never found, bail and append
            // the rest literally rather than looping forever.
            if let Some(end) = url[i + 2..].find("}}") {
                i += 2 + end + 2;
                continue;
            } else {
                out.push_str(&url[i..]);
                return out;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn split_url(url: &str) -> (&str, Option<&str>) {
    match url.find('?') {
        Some(idx) => (&url[..idx], Some(&url[idx + 1..])),
        None => (url, None),
    }
}

fn looks_like_uuid(segment: &str) -> bool {
    // 8-4-4-4-12 hex groups.
    let s = segment;
    if s.len() != 36 {
        return false;
    }
    let expected_dashes = [8, 13, 18, 23];
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if expected_dashes.contains(&i) {
            if *b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_str;
    use std::path::Path;

    fn parse(source: &str) -> TestFile {
        parse_str(source, Path::new("t.tarn.yaml")).expect("parse")
    }

    #[test]
    fn fires_on_positional_capture_over_shared_list() {
        let file = parse(
            r#"
name: list
steps:
  - name: get users
    request:
      method: GET
      url: "http://example.com/users"
    capture:
      first_id: "$[0].id"
"#,
        );
        let findings = lint(&file, "t.tarn.yaml");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "TL001");
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(findings[0].message.contains("first_id"));
    }

    #[test]
    fn silent_when_url_has_path_parameter() {
        // `/users/:id` is a specific-record endpoint — capturing
        // `$.items[0]` here is still awkward but the *list-endpoint*
        // heuristic should not fire.
        let file = parse(
            r#"
name: scoped
steps:
  - name: get roles
    request:
      method: GET
      url: "http://example.com/users/{id}/roles"
    capture:
      first_role: "$.items[0].name"
"#,
        );
        let findings = lint(&file, "t.tarn.yaml");
        assert!(findings.is_empty(), "expected no fire, got {:?}", findings);
    }

    #[test]
    fn silent_when_query_filters_the_list() {
        // A filter-style query param pins the record-set — not brittle.
        let file = parse(
            r#"
name: filtered
steps:
  - name: get user by email
    request:
      method: GET
      url: "http://example.com/users?email=admin@test.com"
    capture:
      id: "$[0].id"
"#,
        );
        let findings = lint(&file, "t.tarn.yaml");
        assert!(findings.is_empty(), "expected no fire, got {:?}", findings);
    }

    #[test]
    fn url_heuristics_detect_uuid_segments() {
        assert!(!url_looks_like_shared_list(
            "http://example.com/users/550e8400-e29b-41d4-a716-446655440000"
        ));
        assert!(url_looks_like_shared_list("http://example.com/users"));
        assert!(!url_looks_like_shared_list(
            "http://example.com/users?filter=admin"
        ));
    }
}
