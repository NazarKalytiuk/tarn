//! TL002 — same-list capture reused across tests in a file.
//!
//! If two distinct named tests each hit the same list endpoint and
//! pull values out by positional index, they are coupled through the
//! list's ordering and length. If one test creates a row, the other
//! test's capture silently shifts. TL001 catches the individual smell;
//! TL002 catches the *coupling* — stronger signal, same fix.

use std::collections::HashMap;

use crate::lint::tl001_positional_capture::{is_positional_index_path, url_looks_like_shared_list};
use crate::lint::{capture_jsonpath, finding_from_step, Finding, Severity};
use crate::model::TestFile;

pub fn lint(file: &TestFile, path: &str) -> Vec<Finding> {
    // Bucket: normalized URL → list of (test_name, step, step_path).
    // We only consider named tests; setup/teardown are shared *by
    // design*, so positional coupling there isn't the same smell.
    let mut buckets: HashMap<String, Vec<(&str, &crate::model::Step, String)>> = HashMap::new();

    for (test_name, group) in &file.tests {
        for step in &group.steps {
            // Only capture-bearing steps matter.
            let has_positional_capture = step
                .capture
                .values()
                .filter_map(capture_jsonpath)
                .any(is_positional_index_path);
            if !has_positional_capture {
                continue;
            }
            let Some(request) = step.request.as_ref() else {
                continue;
            };
            if !url_looks_like_shared_list(&request.url) {
                continue;
            }
            let key = normalize_url(&request.url);
            let step_path = format!("{}::{}::{}", path, test_name, step.name);
            buckets
                .entry(key)
                .or_default()
                .push((test_name.as_str(), step, step_path));
        }
    }

    let mut findings = Vec::new();
    for (url_key, hits) in &buckets {
        // Count DISTINCT tests — multiple positional captures in the
        // same test are still just TL001.
        let distinct_tests: std::collections::HashSet<&str> =
            hits.iter().map(|(t, _, _)| *t).collect();
        if distinct_tests.len() < 2 {
            continue;
        }
        // Emit one finding per participating step so every offending
        // location is jumpable from the editor.
        for (_test, step, step_path) in hits {
            findings.push(finding_from_step(
                "TL002",
                Severity::Warning,
                path,
                Some(step_path.clone()),
                step,
                format!(
                    "Multiple tests capture positionally from the same list endpoint `{}`. Ordering coupling hazard.",
                    url_key
                ),
                Some(
                    "Tests sharing a positional capture on the same list endpoint couple. Consider a per-test setup or a stable filter.".to_string(),
                ),
            ));
        }
    }
    findings
}

/// Strip the query string and trailing slash so `/users` and
/// `/users/` hash the same bucket. We intentionally keep the scheme +
/// host in the key so two different base URLs don't false-positive as
/// the same endpoint.
fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();
    let no_query = match trimmed.find('?') {
        Some(i) => &trimmed[..i],
        None => trimmed,
    };
    no_query.trim_end_matches('/').to_string()
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
    fn fires_when_two_tests_share_positional_capture_on_same_list() {
        let file = parse(
            r#"
name: shared-list
tests:
  list_then_read:
    steps:
      - name: list
        request:
          method: GET
          url: "http://example.com/users"
        capture:
          first: "$[0].id"
  list_then_update:
    steps:
      - name: list again
        request:
          method: GET
          url: "http://example.com/users"
        capture:
          first: "$[0].id"
"#,
        );
        let findings = lint(&file, "t.tarn.yaml");
        assert_eq!(
            findings.len(),
            2,
            "expected one finding per participating step: {:?}",
            findings
        );
        assert!(findings.iter().all(|f| f.rule_id == "TL002"));
    }

    #[test]
    fn quiet_when_only_one_test_uses_positional_capture() {
        let file = parse(
            r#"
name: solo
tests:
  list_only:
    steps:
      - name: list
        request:
          method: GET
          url: "http://example.com/users"
        capture:
          first: "$[0].id"
"#,
        );
        let findings = lint(&file, "t.tarn.yaml");
        assert!(findings.is_empty());
    }

    #[test]
    fn quiet_when_captures_are_on_different_endpoints() {
        // The *coupling* hazard is "same endpoint, different tests" —
        // capturing positionally on two different URLs is still
        // TL001's job, not TL002's.
        let file = parse(
            r#"
name: different-endpoints
tests:
  users:
    steps:
      - name: list users
        request:
          method: GET
          url: "http://example.com/users"
        capture:
          first: "$[0].id"
  posts:
    steps:
      - name: list posts
        request:
          method: GET
          url: "http://example.com/posts"
        capture:
          first: "$[0].id"
"#,
        );
        let findings = lint(&file, "t.tarn.yaml");
        assert!(findings.is_empty(), "got {:?}", findings);
    }
}
