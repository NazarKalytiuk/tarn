//! TL004 — mutation step has no status assertion.
//!
//! Reads often have only body assertions and that's fine. Mutations
//! (POST / PUT / PATCH / DELETE), though, should always pin an
//! expected status. Without one, a 500 that *happens to* return a
//! body that satisfies the body assertions (or an empty body, when
//! there are none) can look like a passing test.
//!
//! The rule only fires when the step has an `assert:` block. A step
//! with no assertions at all is a separate concern (we don't want to
//! demand status assertions on pure-smoke "did this not explode?"
//! steps that use only runtime checks).

use crate::lint::{finding_from_step, walk_steps, Finding, Severity};
use crate::model::TestFile;

pub fn lint(file: &TestFile, path: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (step_path, step) in walk_steps(file, path) {
        let Some(request) = step.request.as_ref() else {
            continue;
        };
        let method = request.method.to_ascii_uppercase();
        let is_mutation = matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE");
        if !is_mutation {
            continue;
        }
        let Some(assertion) = &step.assertions else {
            continue;
        };
        if assertion.status.is_some() {
            continue;
        }
        findings.push(finding_from_step(
            "TL004",
            Severity::Warning,
            path,
            Some(step_path.clone()),
            step,
            format!(
                "{method} step `{}` has an `assert:` block but no `status:` assertion.",
                step.name
            ),
            Some(
                "Mutations should assert an explicit status (e.g. 201 or 200). Without it, a 500 can look like a test pass if body assertions happen to be vacuous.".to_string(),
            ),
        ));
    }
    findings
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
    fn fires_on_post_without_status_assertion() {
        let file = parse(
            r#"
name: no-status
steps:
  - name: create user
    request:
      method: POST
      url: "http://example.com/users"
      body: { name: x }
    assert:
      body:
        "$.name": "x"
"#,
        );
        let findings = lint(&file, "t.tarn.yaml");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "TL004");
    }

    #[test]
    fn quiet_when_get_has_no_status_assertion() {
        // Reads without status assertions are a normal pattern —
        // don't flag them.
        let file = parse(
            r#"
name: read-only
steps:
  - name: get users
    request:
      method: GET
      url: "http://example.com/users"
    assert:
      body:
        "$": { length_gte: 0 }
"#,
        );
        let findings = lint(&file, "t.tarn.yaml");
        assert!(findings.is_empty());
    }

    #[test]
    fn quiet_when_post_has_status_assertion() {
        let file = parse(
            r#"
name: fine
steps:
  - name: create user
    request:
      method: POST
      url: "http://example.com/users"
      body: { name: x }
    assert:
      status: 201
      body:
        "$.name": "x"
"#,
        );
        let findings = lint(&file, "t.tarn.yaml");
        assert!(findings.is_empty());
    }
}
