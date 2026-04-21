//! `tarn impact` — map changes to the Tarn test targets they affect.
//!
//! Pure matcher: no I/O, no HTTP. Callers assemble a [`ChangeSet`] (file
//! paths, endpoints, OpenAPI ids) and a list of already-loaded test files
//! plus their raw source, then call [`analyze`] to get a ranked list of
//! matches. Rendering helpers produce the machine- and human-readable
//! outputs used by the CLI.

pub mod endpoint_match;
pub mod include_match;
pub mod path_match;
pub mod substring_match;

use std::collections::BTreeSet;

use serde::Serialize;

use crate::model::{Step, TestFile};

pub use endpoint_match::{parse_endpoint, EndpointChange};

/// Confidence tier attached to every [`MatchCandidate`]. The CLI sorts
/// high first, filters with `--min-confidence`, and prints the tier in
/// both formats. Values are ordered so `High > Medium > Low`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "high" => Ok(Confidence::High),
            "medium" | "med" => Ok(Confidence::Medium),
            "low" => Ok(Confidence::Low),
            other => Err(format!(
                "invalid confidence '{other}' (expected low / medium / high)"
            )),
        }
    }
}

/// Signal weights — the per-signal score added to a match. These are not
/// probabilities; they only need to order matches sensibly within the
/// same target. Exact endpoint match leads comfortably; substring
/// matches trail so they never outrank a real hit.
///
/// Weights shipped for NAZ-410:
///   - endpoint exact  : 40 (high)
///   - endpoint prefix : 15 (medium)
///   - openapi op-id   : 38 (high)
///   - tag match       : 12 (medium)
///   - direct file edit: 50 (high, terminal)
///   - shared topic    : 10 (medium)
///   - include/fixture :  8 (medium)
///   - substring name  :  3 (low)
pub mod weights {
    pub const ENDPOINT_EXACT: u32 = 40;
    pub const ENDPOINT_PREFIX: u32 = 15;
    pub const OPENAPI_OP: u32 = 38;
    pub const TAG: u32 = 12;
    pub const DIRECT_FILE: u32 = 50;
    pub const SHARED_TOPIC: u32 = 10;
    pub const INCLUDE_REF: u32 = 8;
    pub const SUBSTRING: u32 = 3;
}

/// Bundled inputs describing the change the caller wants to analyze.
#[derive(Debug, Clone, Default)]
pub struct ChangeSet {
    /// Files pulled from `git diff`.
    pub diff_files: Vec<String>,
    /// Files provided explicitly via `--files`.
    pub files: Vec<String>,
    /// Endpoints provided via `--endpoints`.
    pub endpoints: Vec<EndpointChange>,
    /// OpenAPI operation ids provided via `--openapi-ops`.
    pub openapi_ops: Vec<String>,
}

impl ChangeSet {
    /// Flat union of `files` and `diff_files`. Used by file-path / include
    /// matchers that don't care about the source.
    pub fn all_changed_files(&self) -> Vec<String> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut out = Vec::new();
        for f in self.diff_files.iter().chain(self.files.iter()) {
            if seen.insert(f.clone()) {
                out.push(f.clone());
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.diff_files.is_empty()
            && self.files.is_empty()
            && self.endpoints.is_empty()
            && self.openapi_ops.is_empty()
    }
}

/// One candidate test (file- or test-level) with the reasons why it was
/// selected. `test` is `None` for file-level matches (direct edit, shared
/// fixture, include reference).
#[derive(Debug, Clone, Serialize)]
pub struct MatchCandidate {
    pub file: String,
    pub test: Option<String>,
    pub confidence: Confidence,
    pub score: u32,
    pub reasons: Vec<String>,
    pub run_hint: RunHint,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunHint {
    pub command: String,
}

/// The complete impact analysis result.
#[derive(Debug, Clone, Serialize)]
pub struct ImpactReport {
    pub schema_version: u32,
    pub inputs: ReportInputs,
    pub matches: Vec<MatchCandidate>,
    pub low_confidence_only: bool,
    pub advice: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReportInputs {
    pub diff_files: Vec<String>,
    pub files: Vec<String>,
    pub endpoints: Vec<ReportEndpoint>,
    pub openapi_ops: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReportEndpoint {
    pub method: String,
    pub path: String,
}

/// A single test file plus its raw source text. Raw source is kept so the
/// include / fixture matcher can search it without re-reading from disk.
pub struct LoadedTest<'a> {
    pub path: String,
    pub parsed: &'a TestFile,
    pub source: &'a str,
}

/// Core analyzer. Pure — all I/O happens in the caller.
pub fn analyze(change: &ChangeSet, tests: &[LoadedTest<'_>]) -> ImpactReport {
    let mut collected: Vec<MatchCandidate> = Vec::new();
    let mut advice: Vec<String> = Vec::new();

    // 1. Endpoint matching — both exact and prefix, per step.
    for test in tests {
        collect_endpoint_matches(test, &change.endpoints, &mut collected);
    }

    // 2. OpenAPI op-id matching.
    let any_file_declares_ops = tests
        .iter()
        .any(|t| matches!(&t.parsed.openapi_operation_ids, Some(v) if !v.is_empty()));
    for test in tests {
        collect_openapi_matches(test, &change.openapi_ops, &mut collected);
    }
    if !change.openapi_ops.is_empty() && !any_file_declares_ops {
        advice.push(
            "No OpenAPI operation ids declared in any test file; \
             adding `openapi_operation_ids:` sharpens impact analysis."
                .to_string(),
        );
    }

    // 3. Tag matching — synthesize candidate tokens from endpoints.
    for test in tests {
        collect_tag_matches(test, &change.endpoints, &mut collected);
    }

    // 4. File-path matching — direct-edit + shared-topic.
    let changed_files = change.all_changed_files();
    for test in tests {
        collect_path_matches(test, &changed_files, &mut collected);
    }

    // 5. Include / fixture matching — uses raw YAML source.
    for test in tests {
        collect_include_matches(test, &changed_files, &mut collected);
    }

    // 6. Substring name-token matching (low-confidence).
    for test in tests {
        collect_substring_matches(test, &changed_files, &mut collected);
    }

    let mut matches = merge_candidates(collected);
    matches.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then(b.score.cmp(&a.score))
            .then(a.file.cmp(&b.file))
            .then(a.test.cmp(&b.test))
    });

    let low_confidence_only = !matches.is_empty()
        && matches
            .iter()
            .all(|m| matches!(m.confidence, Confidence::Low));
    if matches.is_empty() {
        advice.push(
            "No matches found. Provide `--endpoints` with routes touched by the change \
             or narrow discovery with `--path` to help the matcher."
                .to_string(),
        );
    } else if low_confidence_only {
        advice.push(
            "All matches are low-confidence (name-token substring). Add `--endpoints METHOD:PATH` \
             or declare `openapi_operation_ids:` to strengthen the signal."
                .to_string(),
        );
    }

    ImpactReport {
        schema_version: 1,
        inputs: ReportInputs {
            diff_files: change.diff_files.clone(),
            files: change.files.clone(),
            endpoints: change
                .endpoints
                .iter()
                .map(|e| ReportEndpoint {
                    method: e.method.clone(),
                    path: e.path.clone(),
                })
                .collect(),
            openapi_ops: change.openapi_ops.clone(),
        },
        matches,
        low_confidence_only,
        advice,
    }
}

fn collect_endpoint_matches(
    test: &LoadedTest<'_>,
    endpoints: &[EndpointChange],
    out: &mut Vec<MatchCandidate>,
) {
    if endpoints.is_empty() {
        return;
    }
    let tf = test.parsed;

    // Flat steps (simple form).
    for step in &tf.steps {
        for change in endpoints {
            if let Some(kind) =
                endpoint_match::match_endpoint(&step.request.method, &step.request.url, change)
            {
                push_endpoint_match(test, None, step, change, kind, out);
            }
        }
    }

    // Named tests.
    for (test_name, group) in &tf.tests {
        for step in &group.steps {
            for change in endpoints {
                if let Some(kind) =
                    endpoint_match::match_endpoint(&step.request.method, &step.request.url, change)
                {
                    push_endpoint_match(test, Some(test_name.clone()), step, change, kind, out);
                }
            }
        }
    }
}

fn push_endpoint_match(
    test: &LoadedTest<'_>,
    test_name: Option<String>,
    step: &Step,
    change: &EndpointChange,
    kind: endpoint_match::MatchKind,
    out: &mut Vec<MatchCandidate>,
) {
    let (confidence, score, qualifier) = match kind {
        endpoint_match::MatchKind::Exact => (Confidence::High, weights::ENDPOINT_EXACT, "matches"),
        endpoint_match::MatchKind::Prefix => (
            Confidence::Medium,
            weights::ENDPOINT_PREFIX,
            "path prefix match on",
        ),
    };
    let reason = format!(
        "{} {} {} step '{}'{}",
        change.method,
        change.path,
        qualifier,
        step.name,
        test_name
            .as_ref()
            .map(|n| format!(" in test '{n}'"))
            .unwrap_or_default()
    );
    out.push(MatchCandidate {
        file: test.path.clone(),
        test: test_name.clone(),
        confidence,
        score,
        reasons: vec![reason],
        run_hint: make_run_hint(&test.path, test_name.as_deref()),
    });
}

fn collect_openapi_matches(test: &LoadedTest<'_>, ops: &[String], out: &mut Vec<MatchCandidate>) {
    if ops.is_empty() {
        return;
    }
    let Some(declared) = test.parsed.openapi_operation_ids.as_ref() else {
        return;
    };
    let declared_set: BTreeSet<&str> = declared.iter().map(String::as_str).collect();
    let mut hits: Vec<String> = Vec::new();
    for op in ops {
        if declared_set.contains(op.as_str()) {
            hits.push(op.clone());
        }
    }
    if hits.is_empty() {
        return;
    }
    let reason = format!("openapi_operation_ids match: {}", hits.join(", "));
    out.push(MatchCandidate {
        file: test.path.clone(),
        test: None,
        confidence: Confidence::High,
        score: weights::OPENAPI_OP,
        reasons: vec![reason],
        run_hint: make_run_hint(&test.path, None),
    });
}

fn collect_tag_matches(
    test: &LoadedTest<'_>,
    endpoints: &[EndpointChange],
    out: &mut Vec<MatchCandidate>,
) {
    if endpoints.is_empty() {
        return;
    }
    let candidate_tags: BTreeSet<String> = endpoints
        .iter()
        .flat_map(|e| synthesize_tag_tokens(&e.path))
        .collect();
    if candidate_tags.is_empty() {
        return;
    }

    // File-level tags → file-level match.
    let file_tags: BTreeSet<String> = test
        .parsed
        .tags
        .iter()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    for candidate in &candidate_tags {
        if file_tags.contains(candidate) {
            out.push(MatchCandidate {
                file: test.path.clone(),
                test: None,
                confidence: Confidence::Medium,
                score: weights::TAG,
                reasons: vec![format!("tag '{candidate}' matches endpoint")],
                run_hint: make_run_hint(&test.path, None),
            });
        }
    }

    // Per-test tags → test-level match.
    for (test_name, group) in &test.parsed.tests {
        let group_tags: BTreeSet<String> =
            group.tags.iter().map(|t| t.to_ascii_lowercase()).collect();
        for candidate in &candidate_tags {
            if group_tags.contains(candidate) {
                out.push(MatchCandidate {
                    file: test.path.clone(),
                    test: Some(test_name.clone()),
                    confidence: Confidence::Medium,
                    score: weights::TAG,
                    reasons: vec![format!("tag '{candidate}' matches endpoint")],
                    run_hint: make_run_hint(&test.path, Some(test_name.as_str())),
                });
            }
        }
    }
}

fn synthesize_tag_tokens(path: &str) -> Vec<String> {
    // From `/users/:id/posts`, extract `users` and `posts` as candidate tags.
    path.split('/')
        .filter_map(|seg| {
            let trimmed = seg.trim();
            if trimmed.is_empty() || trimmed.starts_with(':') || trimmed.starts_with('{') {
                return None;
            }
            Some(trimmed.to_ascii_lowercase())
        })
        .collect()
}

fn collect_path_matches(
    test: &LoadedTest<'_>,
    changed_files: &[String],
    out: &mut Vec<MatchCandidate>,
) {
    for changed in changed_files {
        match path_match::match_paths(changed, &test.path) {
            Some(path_match::PathMatch::Direct) => {
                out.push(MatchCandidate {
                    file: test.path.clone(),
                    test: None,
                    confidence: Confidence::High,
                    score: weights::DIRECT_FILE,
                    reasons: vec![format!("direct edit to {}", changed)],
                    run_hint: make_run_hint(&test.path, None),
                });
            }
            Some(path_match::PathMatch::SharedTopic { segment }) => {
                out.push(MatchCandidate {
                    file: test.path.clone(),
                    test: None,
                    confidence: Confidence::Medium,
                    score: weights::SHARED_TOPIC,
                    reasons: vec![format!(
                        "shared '{segment}/' directory with changed {changed}"
                    )],
                    run_hint: make_run_hint(&test.path, None),
                });
            }
            None => {}
        }
    }
}

fn collect_include_matches(
    test: &LoadedTest<'_>,
    changed_files: &[String],
    out: &mut Vec<MatchCandidate>,
) {
    for changed in changed_files {
        // Skip the self-reference path; that's already handled by the direct
        // edit matcher and would otherwise double-count.
        if path_match::match_paths(changed, &test.path)
            .map(|m| matches!(m, path_match::PathMatch::Direct))
            .unwrap_or(false)
        {
            continue;
        }
        if let Some(hit) = include_match::references_path(test.source, changed) {
            out.push(MatchCandidate {
                file: test.path.clone(),
                test: None,
                confidence: Confidence::Medium,
                score: weights::INCLUDE_REF,
                reasons: vec![format!("references {hit} (include or fixture)")],
                run_hint: make_run_hint(&test.path, None),
            });
        }
    }
}

fn collect_substring_matches(
    test: &LoadedTest<'_>,
    changed_files: &[String],
    out: &mut Vec<MatchCandidate>,
) {
    for changed in changed_files {
        let tokens = substring_match::tokens_for(changed);
        if tokens.is_empty() {
            continue;
        }
        // File-name / file-description haystack.
        let file_haystack = format!(
            "{} {} {}",
            test.path,
            test.parsed.name,
            test.parsed.description.clone().unwrap_or_default()
        );
        if let Some(hit) = substring_match::first_hit(&tokens, &file_haystack) {
            out.push(MatchCandidate {
                file: test.path.clone(),
                test: None,
                confidence: Confidence::Low,
                score: weights::SUBSTRING,
                reasons: vec![format!("file name token '{hit}' matches changed {changed}")],
                run_hint: make_run_hint(&test.path, None),
            });
        }
        // Named test / step haystack.
        for (test_name, group) in &test.parsed.tests {
            let step_urls = group
                .steps
                .iter()
                .map(|s| format!("{} {}", s.name, s.request.url))
                .collect::<Vec<_>>()
                .join(" ");
            let haystack = format!("{test_name} {step_urls}");
            if let Some(hit) = substring_match::first_hit(&tokens, &haystack) {
                out.push(MatchCandidate {
                    file: test.path.clone(),
                    test: Some(test_name.clone()),
                    confidence: Confidence::Low,
                    score: weights::SUBSTRING,
                    reasons: vec![format!("test/step token '{hit}' matches changed {changed}")],
                    run_hint: make_run_hint(&test.path, Some(test_name.as_str())),
                });
            }
        }
    }
}

/// Collapse duplicate (file, test) candidates: pick the highest confidence
/// and score, union the reasons, and preserve insertion order for stable
/// downstream sorting.
fn merge_candidates(candidates: Vec<MatchCandidate>) -> Vec<MatchCandidate> {
    use std::collections::HashMap;
    let mut by_key: HashMap<(String, Option<String>), MatchCandidate> = HashMap::new();
    let mut order: Vec<(String, Option<String>)> = Vec::new();
    for cand in candidates {
        let key = (cand.file.clone(), cand.test.clone());
        if let Some(existing) = by_key.get_mut(&key) {
            if cand.confidence > existing.confidence {
                existing.confidence = cand.confidence;
            }
            // Sum scores across signals (weighted additive), capped so the
            // final sort still fits in u32.
            existing.score = existing.score.saturating_add(cand.score);
            for r in cand.reasons {
                if !existing.reasons.contains(&r) {
                    existing.reasons.push(r);
                }
            }
        } else {
            order.push(key.clone());
            by_key.insert(key, cand);
        }
    }
    order
        .into_iter()
        .filter_map(|k| by_key.remove(&k))
        .collect()
}

fn make_run_hint(file: &str, test: Option<&str>) -> RunHint {
    let command = match test {
        Some(name) => format!("tarn run {file} --test-filter \"{name}\""),
        None => format!("tarn run {file}"),
    };
    RunHint { command }
}

/// Filter matches whose confidence is below `min`. Returns a new report
/// (with the same advice / inputs) reflecting the filter.
pub fn filter_by_confidence(mut report: ImpactReport, min: Confidence) -> ImpactReport {
    report.matches.retain(|m| m.confidence >= min);
    report.low_confidence_only = !report.matches.is_empty()
        && report
            .matches
            .iter()
            .all(|m| matches!(m.confidence, Confidence::Low));
    report
}

/// Machine-readable JSON output.
pub fn render_json(report: &ImpactReport) -> String {
    serde_json::to_string_pretty(report).expect("ImpactReport serializes as JSON")
}

/// Grouped human-readable output. `use_color` adds ANSI styling around the
/// confidence tag; callers pass `false` when stdout is not a TTY.
pub fn render_human(report: &ImpactReport, use_color: bool) -> String {
    let mut out = String::new();
    if report.matches.is_empty() {
        out.push_str("tarn impact: no matches.\n");
    } else {
        out.push_str(&format!(
            "tarn impact: {} match(es) (sorted by confidence + score)\n",
            report.matches.len()
        ));
        for m in &report.matches {
            let tag = match (m.confidence, use_color) {
                (Confidence::High, true) => "\x1b[32m[high]\x1b[0m".to_string(),
                (Confidence::Medium, true) => "\x1b[33m[medium]\x1b[0m".to_string(),
                (Confidence::Low, true) => "\x1b[90m[low]\x1b[0m".to_string(),
                (c, false) => format!("[{}]", c.label()),
            };
            let target = match &m.test {
                Some(t) => format!("{}::{}", m.file, t),
                None => m.file.clone(),
            };
            out.push_str(&format!(
                "  {tag:<10} {target} — {}\n",
                m.reasons.first().cloned().unwrap_or_default()
            ));
            for extra in m.reasons.iter().skip(1) {
                out.push_str(&format!("            └─ {extra}\n"));
            }
            out.push_str(&format!("            run: {}\n", m.run_hint.command));
        }
    }
    if !report.advice.is_empty() {
        out.push_str("\nAdvice:\n");
        for line in &report.advice {
            out.push_str(&format!("  - {line}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn load(source: &str, path: &str) -> (TestFile, String) {
        let tf = parser::parse_str(source, std::path::Path::new(path)).expect("parse");
        (tf, source.to_string())
    }

    fn endpoints(changes: &[(&str, &str)]) -> Vec<EndpointChange> {
        changes
            .iter()
            .map(|(m, p)| EndpointChange {
                method: m.to_string(),
                path: p.to_string(),
            })
            .collect()
    }

    #[test]
    fn exact_endpoint_match_returns_high_confidence() {
        let yaml = r#"
name: Users
tests:
  get_user:
    steps:
      - name: GET /users/:id
        request:
          method: GET
          url: "{{ env.base_url }}/users/42"
"#;
        let (tf, src) = load(yaml, "tests/users.tarn.yaml");
        let tests = vec![LoadedTest {
            path: "tests/users.tarn.yaml".into(),
            parsed: &tf,
            source: &src,
        }];
        let report = analyze(
            &ChangeSet {
                endpoints: endpoints(&[("GET", "/users/:id")]),
                ..ChangeSet::default()
            },
            &tests,
        );
        assert_eq!(report.matches.len(), 1);
        let m = &report.matches[0];
        assert_eq!(m.confidence, Confidence::High);
        assert_eq!(m.test.as_deref(), Some("get_user"));
        assert!(m.reasons[0].contains("GET /users/:id matches"));
    }

    #[test]
    fn prefix_endpoint_match_returns_medium_confidence() {
        let yaml = r#"
name: Users
tests:
  list_posts:
    steps:
      - name: GET /users/:id/posts
        request:
          method: GET
          url: "{{ env.base_url }}/users/42/posts"
"#;
        let (tf, src) = load(yaml, "tests/users.tarn.yaml");
        let tests = vec![LoadedTest {
            path: "tests/users.tarn.yaml".into(),
            parsed: &tf,
            source: &src,
        }];
        let report = analyze(
            &ChangeSet {
                endpoints: endpoints(&[("GET", "/users")]),
                ..ChangeSet::default()
            },
            &tests,
        );
        // Prefix endpoint + tag('users') → both contribute; confidence is
        // medium (no high signals) regardless.
        let top = report.matches.first().expect("at least one match");
        assert_eq!(top.confidence, Confidence::Medium);
    }

    #[test]
    fn unrelated_endpoint_yields_no_match() {
        let yaml = r#"
name: Health
steps:
  - name: GET /health
    request:
      method: GET
      url: "{{ env.base_url }}/health"
"#;
        let (tf, src) = load(yaml, "tests/health.tarn.yaml");
        let report = analyze(
            &ChangeSet {
                endpoints: endpoints(&[("POST", "/orders")]),
                ..ChangeSet::default()
            },
            &[LoadedTest {
                path: "tests/health.tarn.yaml".into(),
                parsed: &tf,
                source: &src,
            }],
        );
        assert!(report.matches.is_empty());
        assert!(!report.advice.is_empty(), "should surface fallback advice");
    }

    #[test]
    fn openapi_op_id_match_is_high_and_cites_id() {
        let yaml = r#"
name: Users
openapi_operation_ids:
  - getUserById
  - createUser
steps:
  - name: GET /users/:id
    request:
      method: GET
      url: "{{ env.base_url }}/users/1"
"#;
        let (tf, src) = load(yaml, "tests/users.tarn.yaml");
        let report = analyze(
            &ChangeSet {
                openapi_ops: vec!["getUserById".into()],
                ..ChangeSet::default()
            },
            &[LoadedTest {
                path: "tests/users.tarn.yaml".into(),
                parsed: &tf,
                source: &src,
            }],
        );
        assert_eq!(report.matches.len(), 1);
        assert_eq!(report.matches[0].confidence, Confidence::High);
        assert!(report.matches[0]
            .reasons
            .iter()
            .any(|r| r.contains("getUserById")));
    }

    #[test]
    fn openapi_ops_with_no_declarations_emits_advice() {
        let yaml = r#"
name: Users
steps:
  - name: GET /users
    request:
      method: GET
      url: "{{ env.base_url }}/users"
"#;
        let (tf, src) = load(yaml, "tests/users.tarn.yaml");
        let report = analyze(
            &ChangeSet {
                openapi_ops: vec!["getUsers".into()],
                ..ChangeSet::default()
            },
            &[LoadedTest {
                path: "tests/users.tarn.yaml".into(),
                parsed: &tf,
                source: &src,
            }],
        );
        assert!(report
            .advice
            .iter()
            .any(|a| a.contains("openapi_operation_ids")));
    }

    #[test]
    fn tag_match_contributes_medium_confidence() {
        let yaml = r#"
name: Users
tags: [users]
steps:
  - name: GET /health
    request:
      method: GET
      url: "{{ env.base_url }}/health"
"#;
        let (tf, src) = load(yaml, "tests/misc.tarn.yaml");
        let report = analyze(
            &ChangeSet {
                endpoints: endpoints(&[("GET", "/users/:id")]),
                ..ChangeSet::default()
            },
            &[LoadedTest {
                path: "tests/misc.tarn.yaml".into(),
                parsed: &tf,
                source: &src,
            }],
        );
        assert_eq!(report.matches.len(), 1);
        let m = &report.matches[0];
        assert_eq!(m.confidence, Confidence::Medium);
        assert!(m.reasons.iter().any(|r| r.contains("tag 'users'")));
    }

    #[test]
    fn direct_edit_to_tarn_yaml_is_high() {
        let yaml = r#"
name: Users
steps:
  - name: GET /users
    request:
      method: GET
      url: "{{ env.base_url }}/users"
"#;
        let (tf, src) = load(yaml, "tests/users.tarn.yaml");
        let report = analyze(
            &ChangeSet {
                files: vec!["tests/users.tarn.yaml".into()],
                ..ChangeSet::default()
            },
            &[LoadedTest {
                path: "tests/users.tarn.yaml".into(),
                parsed: &tf,
                source: &src,
            }],
        );
        assert_eq!(report.matches.len(), 1);
        assert_eq!(report.matches[0].confidence, Confidence::High);
    }

    #[test]
    fn shared_topic_produces_medium_confidence_file_match() {
        let yaml = r#"
name: Users
steps:
  - name: GET /users
    request:
      method: GET
      url: "{{ env.base_url }}/users"
"#;
        let (tf, src) = load(yaml, "tests/users/crud.tarn.yaml");
        let report = analyze(
            &ChangeSet {
                files: vec!["src/users/service.ts".into()],
                ..ChangeSet::default()
            },
            &[LoadedTest {
                path: "tests/users/crud.tarn.yaml".into(),
                parsed: &tf,
                source: &src,
            }],
        );
        let top = report.matches.first().expect("match exists");
        assert!(matches!(
            top.confidence,
            Confidence::Medium | Confidence::Low
        ));
        assert!(
            top.reasons
                .iter()
                .any(|r| r.contains("shared 'users/' directory")),
            "reasons={:?}",
            top.reasons
        );
    }

    #[test]
    fn unrelated_file_path_does_not_match() {
        let yaml = r#"
name: Users
steps:
  - name: GET /users
    request:
      method: GET
      url: "{{ env.base_url }}/users"
"#;
        let (tf, src) = load(yaml, "tests/users.tarn.yaml");
        let report = analyze(
            &ChangeSet {
                files: vec!["docs/readme.md".into()],
                ..ChangeSet::default()
            },
            &[LoadedTest {
                path: "tests/users.tarn.yaml".into(),
                parsed: &tf,
                source: &src,
            }],
        );
        assert!(report.matches.is_empty());
    }

    #[test]
    fn include_reference_yields_medium_match() {
        // Use a literal raw source so the `include:` reference survives the
        // match (we scan raw YAML, not the parsed TestFile).
        let source = r#"
name: Users
steps:
  - include: ./shared/auth.tarn.yaml
  - name: GET /health
    request:
      method: GET
      url: "{{ env.base_url }}/health"
"#
        .to_string();
        // Build a TestFile by parsing a minimal YAML (without the include) so
        // we do not need the referenced snippet on disk.
        let parsed_yaml = r#"
name: Users
steps:
  - name: GET /health
    request:
      method: GET
      url: "{{ env.base_url }}/health"
"#;
        let (tf, _) = load(parsed_yaml, "tests/users.tarn.yaml");
        let report = analyze(
            &ChangeSet {
                files: vec!["shared/auth.tarn.yaml".into()],
                ..ChangeSet::default()
            },
            &[LoadedTest {
                path: "tests/users.tarn.yaml".into(),
                parsed: &tf,
                source: &source,
            }],
        );
        assert!(
            report
                .matches
                .iter()
                .any(|m| m.confidence == Confidence::Medium
                    && m.reasons
                        .iter()
                        .any(|r| r.contains("shared/auth.tarn.yaml"))),
            "matches={:?}",
            report.matches
        );
    }

    #[test]
    fn substring_match_stays_low_confidence() {
        // Changed file and test file share no directory / filename stem, so
        // path_match contributes nothing. The only hit is a substring of a
        // step URL ("notifier" inside "/notifier/send"), which must stay
        // clamped at `low`.
        let yaml = r#"
name: Smoke suite
tests:
  t1:
    steps:
      - name: POST send
        request:
          method: POST
          url: "{{ env.base_url }}/notifier/send"
"#;
        let (tf, src) = load(yaml, "tests/smoke.tarn.yaml");
        let report = analyze(
            &ChangeSet {
                files: vec!["app/background/notifier.ts".into()],
                ..ChangeSet::default()
            },
            &[LoadedTest {
                path: "tests/smoke.tarn.yaml".into(),
                parsed: &tf,
                source: &src,
            }],
        );
        // The only signal here should be substring + maybe service/notifications
        // name overlap; confidence must not exceed Low.
        assert!(!report.matches.is_empty());
        assert!(
            report
                .matches
                .iter()
                .all(|m| matches!(m.confidence, Confidence::Low)),
            "matches={:?}",
            report.matches
        );
        assert!(report.low_confidence_only);
    }

    #[test]
    fn filter_by_confidence_drops_low_matches() {
        let yaml = r#"
name: Notifications
steps:
  - name: POST /notifications
    request:
      method: POST
      url: "{{ env.base_url }}/notifications"
"#;
        let (tf, src) = load(yaml, "tests/notifications.tarn.yaml");
        let report = analyze(
            &ChangeSet {
                files: vec!["src/notifier.ts".into()],
                ..ChangeSet::default()
            },
            &[LoadedTest {
                path: "tests/notifications.tarn.yaml".into(),
                parsed: &tf,
                source: &src,
            }],
        );
        let filtered = filter_by_confidence(report, Confidence::High);
        assert!(filtered.matches.is_empty());
    }
}
