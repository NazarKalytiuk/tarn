//! TL008 — hard-coded absolute URL in a request.
//!
//! `url: https://api.prod.example.com/users` pins the test to one
//! environment. The moment the user tries to run the suite against
//! staging or a local stub, every step points at prod. Prefer
//! `{{ env.base_url }}/users` and flip the env file.
//!
//! Exceptions:
//! - `http://localhost` / `http://127.0.0.1` — these almost always
//!   mean "the local test fixture" and are idiomatic in example/demo
//!   files and in deliberate negative tests (e.g. port 1 / 65535
//!   used to force a connection failure).
//! - `--lint-allow-absolute-urls` suppresses the rule globally.

use crate::lint::{finding_from_step, walk_steps, Finding, LintOptions, Severity};
use crate::model::TestFile;

pub fn lint(file: &TestFile, path: &str, opts: &LintOptions) -> Vec<Finding> {
    if opts.allow_absolute_urls {
        return Vec::new();
    }
    let mut findings = Vec::new();
    for (step_path, step) in walk_steps(file, path) {
        let Some(request) = step.request.as_ref() else {
            continue;
        };
        let url = request.url.trim();
        if !is_absolute_http_url(url) {
            continue;
        }
        if is_localhost_url(url) {
            continue;
        }
        findings.push(finding_from_step(
            "TL008",
            Severity::Info,
            path,
            Some(step_path.clone()),
            step,
            format!(
                "Step `{}` uses a hard-coded absolute URL `{}`.",
                step.name, url
            ),
            Some(
                "Hard-coded URLs break across environments. Prefer `{{ env.base_url }}/path`."
                    .to_string(),
            ),
        ));
    }
    findings
}

fn is_absolute_http_url(url: &str) -> bool {
    // A URL is considered absolute for our purposes when it starts
    // with a scheme. Templated URLs (`{{ env.base_url }}/x`) start
    // with `{` and don't match. We only flag http/https — gRPC /
    // ws / file schemes are rare enough in API tests that a
    // conservative check is better than a greedy one.
    url.starts_with("http://") || url.starts_with("https://")
}

fn is_localhost_url(url: &str) -> bool {
    // After the scheme, check the host-plus-port prefix.
    let after_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    // `host[:port]/...` — cut at `/` or `?`.
    let host_end = after_scheme.find(['/', '?']).unwrap_or(after_scheme.len());
    let host = &after_scheme[..host_end];
    let host_only = match host.find(':') {
        Some(i) => &host[..i],
        None => host,
    };
    matches!(host_only, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0")
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
    fn fires_on_hard_coded_https_url() {
        let file = parse(
            r#"
name: absolute
steps:
  - name: ping
    request:
      method: GET
      url: "https://api.prod.example.com/users"
    assert:
      status: 200
"#,
        );
        let findings = lint(&file, "t.tarn.yaml", &LintOptions::default());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "TL008");
    }

    #[test]
    fn quiet_on_localhost_urls() {
        // Localhost is the canonical "this is a local fixture" host.
        let file = parse(
            r#"
name: localish
steps:
  - name: ping
    request:
      method: GET
      url: "http://localhost:3000/health"
    assert:
      status: 200
  - name: unreachable probe
    request:
      method: GET
      url: "http://127.0.0.1:1/closed-port"
    assert:
      status: 200
"#,
        );
        let findings = lint(&file, "t.tarn.yaml", &LintOptions::default());
        assert!(findings.is_empty(), "got {:?}", findings);
    }

    #[test]
    fn silent_when_override_enabled() {
        let file = parse(
            r#"
name: third-party
steps:
  - name: ping external
    request:
      method: GET
      url: "https://api.github.com/rate_limit"
    assert:
      status: 200
"#,
        );
        let findings = lint(
            &file,
            "t.tarn.yaml",
            &LintOptions {
                allow_absolute_urls: true,
            },
        );
        assert!(findings.is_empty());
    }
}
