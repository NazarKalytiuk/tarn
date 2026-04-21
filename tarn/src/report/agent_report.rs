//! Agent-oriented compact run report (NAZ-412).
//!
//! An LLM agent loop rarely benefits from the full `report.json` —
//! what it needs to decide "what should I edit next?" is tiny:
//!
//! * was the run green or red?
//! * what distinct *root causes* failed (not every downstream cascade)?
//! * for each, the smallest diagnostic excerpt that still names the
//!   next edit
//! * artifact paths for escalation when the compact form is not enough
//!
//! `tarn run --agent` prints one serialized [`AgentReport`] to stdout,
//! suppresses all other stdout output, and leaves stderr alone so
//! humans watching the run still see the usual `run id:` announcements.
//!
//! This module is pure: it takes an already-finished
//! [`crate::assert::types::RunResult`] plus bookkeeping (run id,
//! artifact paths, selector state) and produces the compact envelope.
//! It reuses [`crate::report::failures_command`] for root-cause
//! grouping so both `tarn failures` and `tarn run --agent` agree on
//! the definition of "same problem".

use crate::assert::types::{FailureCategory, RunResult};
use crate::report::failures_command::{build_report, FailureGroup, FailuresReport};
use crate::report::shape_diagnosis::{ShapeConfidence, ShapeMismatchDiagnosis};
use crate::report::summary::{build_summary_and_failures, Counts, FailureRequest, FailureResponse};
use crate::selector::{Selector, StepSelector};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::path::Path;

/// Bumped on any incompatible change to the envelope below.
///
/// Agent clients and MCP wrappers key off this to decide whether they
/// can safely deserialize the payload. Additive changes (new optional
/// field, new `next_actions.kind`) do not bump the version.
pub const AGENT_REPORT_SCHEMA_VERSION: u32 = 1;

/// Maximum number of root-cause groups surfaced in a single agent
/// report. Anything beyond this is truncated with a pointer to
/// `tarn failures` for the full list. Ten is enough to cover realistic
/// multi-service suites without turning the payload into a wall of
/// near-duplicates.
pub const MAX_ROOT_CAUSES: usize = 10;

/// Maximum `body_excerpt` length in characters, tighter than
/// `summary::BODY_EXCERPT_MAX_CHARS` (500). Agents that need more can
/// load `report.json`; the inline excerpt is just "enough to pattern
/// match on the error payload".
pub const AGENT_BODY_EXCERPT_MAX_CHARS: usize = 300;

/// Top-level compact payload. See module docs for the full contract.
#[derive(Debug, Clone, Serialize)]
pub struct AgentReport {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub status: AgentStatus,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub selected: Selected,
    pub totals: Counts,
    pub failed: FailedCounts,
    pub root_causes: Vec<AgentRootCause>,
    pub artifacts: Artifacts,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Selected {
    /// Every file the run actually executed over.
    pub files: Vec<String>,
    /// Explicit test/step narrowing when the caller passed
    /// `--select`, `--test-filter`, or `--step-filter`. `None` when no
    /// selector narrowed the run below file granularity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tests: Option<Vec<SelectedTest>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectedTest {
    pub file: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FailedCounts {
    pub files: usize,
    pub tests: usize,
    pub steps: usize,
    /// Distinct root-cause groups. Not the same as `steps`: a single
    /// root cause can fan out into many failing steps via cascade.
    pub root_causes: usize,
    /// Cascade-only skips (skipped_due_to_failed_capture,
    /// skipped_due_to_fail_fast). Tracked separately so the agent
    /// can see "this run had 1 real failure + 7 follow-on skips"
    /// instead of reading `steps: 8` and chasing all eight.
    pub cascaded_skips: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentRootCause {
    pub fingerprint: String,
    pub category: Option<FailureCategory>,
    pub file: String,
    pub test: String,
    pub step: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<FailureRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<FailureResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_shape_mismatch: Option<ShapeMismatchDiagnosis>,
    pub cascaded_steps: Vec<CascadedStep>,
    pub next_actions: Vec<NextAction>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CascadedStep {
    pub file: String,
    pub test: String,
    pub step: String,
}

/// A machine-dispatchable suggestion. `kind` is stable and enumerable;
/// everything else is optional context the agent may surface verbatim.
///
/// Keep the payload shape data-driven so agent harnesses can switch
/// on `kind` and take the right follow-up action without string
/// parsing the `message`.
#[derive(Debug, Clone, Serialize)]
pub struct NextAction {
    pub kind: NextActionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<ShapeConfidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NextActionKind {
    /// The failing JSONPath has a high-confidence replacement derived
    /// from `response_shape_mismatch`. Caller should swap it in.
    ReplaceJsonpath,
    /// Open the fixture / last-response pair for the failing step.
    InspectStep,
    /// Kick off a targeted rerun of just the failed tests.
    RerunFailed,
    /// A network-level failure: the host is unreachable. The caller
    /// should verify the server / credentials before re-running.
    CheckServerReachable,
}

#[derive(Debug, Clone, Serialize)]
pub struct Artifacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failures: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<String>,
}

/// Inputs the builder needs on top of the finished [`RunResult`]. Kept
/// as a struct so future fields (e.g. an MCP session id) do not force a
/// cascading signature change.
#[derive(Debug, Clone)]
pub struct AgentReportInputs<'a> {
    pub run_id: Option<String>,
    pub exit_code: i32,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    /// Files the run actually walked over (post-discovery, post-selection).
    pub selected_files: &'a [String],
    /// Selectors supplied by the caller. Used to populate
    /// `selected.tests`; pass an empty slice when no narrowing happened.
    pub selectors: &'a [Selector],
    /// Absolute path to the per-run artifact directory, when one was
    /// created. When `None`, the artifacts block is empty.
    pub run_directory: Option<&'a Path>,
}

/// Build the compact agent payload from a finished run.
///
/// Two passes internally: first the NAZ-402 grouper gives us stable
/// fingerprints and cascade attribution; then we re-walk the raw
/// failure entries to merge the shape-drift hint and promote a
/// `replace_jsonpath` action to the front of `next_actions`. Keeping
/// the grouper API untouched (it deliberately drops the hint) and
/// doing the merge here means adding new hint types later only
/// touches this function.
pub fn build(run: &RunResult, inputs: AgentReportInputs<'_>) -> AgentReport {
    let (summary, failures_doc) = build_summary_and_failures(
        run,
        inputs.started_at,
        inputs.ended_at,
        inputs.exit_code,
        inputs.run_id.clone(),
        None,
    );

    // Reuse the NAZ-402 grouper so both `tarn failures` and the agent
    // report agree on fingerprints and cascade attribution. The source
    // label is display-only here and never read back.
    let failures_report: FailuresReport = build_report(&failures_doc, "agent");

    let status = if run.passed() {
        AgentStatus::Passed
    } else {
        AgentStatus::Failed
    };
    let duration_ms = inputs
        .ended_at
        .signed_duration_since(inputs.started_at)
        .num_milliseconds()
        .max(0) as u64;

    // Narrow the root-cause list down to the budget, recording a note
    // when we drop some so the caller knows they are looking at a
    // subset. `unattributed_cascade` is not a root cause — it is
    // fallout that could not be attributed to a primary failure and we
    // skip it here to keep the payload focused on actionable items.
    let actionable_groups: Vec<&FailureGroup> = failures_report
        .groups
        .iter()
        .filter(|g| g.fingerprint != "unattributed_cascade")
        .collect();
    let total_groups = actionable_groups.len();
    let truncated = total_groups > MAX_ROOT_CAUSES;
    let take = MAX_ROOT_CAUSES.min(total_groups);

    let mut notes: Vec<String> = Vec::new();
    if truncated {
        notes.push(format!(
            "truncated to {} root causes (of {}); run `tarn failures` for the full list",
            MAX_ROOT_CAUSES, total_groups
        ));
    }

    let mut root_causes: Vec<AgentRootCause> = actionable_groups
        .iter()
        .take(take)
        .map(|group| build_root_cause(group, total_groups))
        .collect();

    // Second pass: merge shape-drift hints and promote
    // `replace_jsonpath` actions. We look the hint up by the
    // exemplar's (file, test, step) coordinates — the grouper always
    // carries them through unchanged.
    for rc in root_causes.iter_mut() {
        if let Some(entry) = failures_doc
            .failures
            .iter()
            .find(|f| f.file == rc.file && f.test == rc.test && f.step == rc.step)
        {
            if let Some(hint) = entry.response_shape_mismatch.as_ref() {
                rc.response_shape_mismatch = Some(hint.clone());
                prepend_replace_jsonpath(rc, hint);
            }
        }
    }

    let selected = build_selected(inputs.selected_files, inputs.selectors);

    AgentReport {
        schema_version: AGENT_REPORT_SCHEMA_VERSION,
        run_id: inputs.run_id,
        status,
        exit_code: inputs.exit_code,
        duration_ms,
        selected,
        totals: summary.totals,
        failed: FailedCounts {
            files: summary.failed.files,
            tests: summary.failed.tests,
            steps: summary.failed.steps,
            root_causes: total_groups,
            cascaded_skips: failures_report.total_cascades,
        },
        root_causes,
        artifacts: build_artifacts(inputs.run_directory),
        notes,
    }
}

/// Render the agent report as pretty-printed JSON with a trailing
/// newline. The trailing newline is deliberate: command-line JSON
/// consumers (jq, many MCP wrappers) expect line-terminated output.
pub fn render_json(report: &AgentReport) -> String {
    let mut out = serde_json::to_string_pretty(report).expect("AgentReport is always serializable");
    out.push('\n');
    out
}

fn build_root_cause(group: &FailureGroup, total_groups: usize) -> AgentRootCause {
    let exemplar = &group.root_cause;

    let response = exemplar.response.as_ref().map(|r| FailureResponse {
        status: r.status,
        body_excerpt: r.body_excerpt.as_deref().map(trim_excerpt),
    });

    let cascaded_steps: Vec<CascadedStep> = group
        .blocked_steps
        .iter()
        .map(|b| CascadedStep {
            file: b.file.clone(),
            test: b.test.clone(),
            step: b.step.clone(),
        })
        .collect();

    let next_actions = synthesize_next_actions(group, total_groups);

    AgentRootCause {
        fingerprint: group.fingerprint.clone(),
        category: exemplar.category,
        file: exemplar.file.clone(),
        test: exemplar.test.clone(),
        step: exemplar.step.clone(),
        message: exemplar.message.clone(),
        request: exemplar.request.clone(),
        response,
        // Populated later in `build` by matching against the raw
        // failure entries — the grouper's exemplar deliberately drops
        // the structured hint so this module re-attaches it post-hoc.
        response_shape_mismatch: None,
        cascaded_steps,
        next_actions,
    }
}

fn prepend_replace_jsonpath(rc: &mut AgentRootCause, hint: &ShapeMismatchDiagnosis) {
    let Some(best) = hint
        .candidate_fixes
        .iter()
        .find(|c| c.confidence == ShapeConfidence::High)
    else {
        return;
    };
    // Do not double-stamp if we already have a replace_jsonpath for
    // this suggestion (defensive — normal flow does not call twice).
    if rc
        .next_actions
        .iter()
        .any(|a| a.kind == NextActionKind::ReplaceJsonpath)
    {
        return;
    }
    let action = NextAction {
        kind: NextActionKind::ReplaceJsonpath,
        suggestion: Some(best.path.clone()),
        command: None,
        confidence: Some(best.confidence),
        host: None,
    };
    rc.next_actions.insert(0, action);
}

fn synthesize_next_actions(group: &FailureGroup, total_groups: usize) -> Vec<NextAction> {
    let mut out: Vec<NextAction> = Vec::new();
    let exemplar = &group.root_cause;

    // 1. `inspect_step` is always useful: it names the exact LSP /
    //    CLI call that opens the last fixture for the failing step.
    let inspect_cmd = format!(
        "tarn inspect last {}::{}::{}",
        exemplar.file, exemplar.test, exemplar.step
    );
    out.push(NextAction {
        kind: NextActionKind::InspectStep,
        suggestion: None,
        command: Some(inspect_cmd),
        confidence: None,
        host: None,
    });

    // 2. When >= 2 distinct problems exist, offer a targeted rerun
    //    command the agent can dispatch after making edits.
    if total_groups >= 2 {
        out.push(NextAction {
            kind: NextActionKind::RerunFailed,
            suggestion: None,
            command: Some("tarn rerun --failed".to_string()),
            confidence: None,
            host: None,
        });
    }

    // 3. Connection-style failures benefit from a reachability hint.
    if matches!(
        exemplar.category,
        Some(FailureCategory::ConnectionError) | Some(FailureCategory::Timeout)
    ) {
        if let Some(host) = extract_host(exemplar.request.as_ref()) {
            out.push(NextAction {
                kind: NextActionKind::CheckServerReachable,
                suggestion: None,
                command: None,
                confidence: None,
                host: Some(host),
            });
        }
    }

    out
}

fn extract_host(request: Option<&FailureRequest>) -> Option<String> {
    let url = request?.url.as_str();
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = without_scheme.split('/').next().unwrap_or("");
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn trim_excerpt(s: &str) -> String {
    if s.chars().count() <= AGENT_BODY_EXCERPT_MAX_CHARS {
        return s.to_string();
    }
    // Truncate on a char boundary so we never produce invalid UTF-8.
    let end = s
        .char_indices()
        .take(AGENT_BODY_EXCERPT_MAX_CHARS)
        .last()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    format!("{}…[truncated]", &s[..end])
}

fn build_artifacts(run_directory: Option<&Path>) -> Artifacts {
    match run_directory {
        None => Artifacts {
            run_dir: None,
            report: None,
            summary: None,
            failures: None,
            state: None,
            events: None,
        },
        Some(dir) => Artifacts {
            run_dir: Some(display_path(dir)),
            report: Some(display_path(&dir.join("report.json"))),
            summary: Some(display_path(&dir.join("summary.json"))),
            failures: Some(display_path(&dir.join("failures.json"))),
            state: Some(display_path(&dir.join("state.json"))),
            events: Some(display_path(&dir.join("events.jsonl"))),
        },
    }
}

fn display_path(p: &Path) -> String {
    p.display().to_string()
}

fn build_selected(files: &[String], selectors: &[Selector]) -> Selected {
    let narrows_tests = selectors
        .iter()
        .any(|s| s.test.is_some() || s.step.is_some());
    let tests = if narrows_tests {
        Some(selectors_to_selected_tests(selectors))
    } else {
        None
    };
    Selected {
        files: files.to_vec(),
        tests,
    }
}

fn selectors_to_selected_tests(selectors: &[Selector]) -> Vec<SelectedTest> {
    selectors
        .iter()
        .filter_map(|sel| {
            let test = sel.test.clone()?;
            let step = sel.step.as_ref().map(step_selector_label);
            Some(SelectedTest {
                file: sel.file.clone(),
                name: test,
                step,
            })
        })
        .collect()
}

fn step_selector_label(step: &StepSelector) -> String {
    match step {
        StepSelector::Index(i) => i.to_string(),
        StepSelector::Name(n) => n.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::{
        AssertionResult, FailureCategory, FileResult, RequestInfo, ResponseInfo, RunResult,
        StepResult, TestResult,
    };
    use crate::model::RedactionConfig;
    use crate::report::shape_diagnosis::{CandidateFix, ShapeConfidence, ShapeMismatchDiagnosis};
    use std::collections::HashMap;

    fn passing_step(name: &str) -> StepResult {
        StepResult {
            name: name.into(),
            description: None,
            debug: false,
            passed: true,
            duration_ms: 1,
            assertion_results: vec![AssertionResult::pass("status", "200", "200")],
            request_info: None,
            response_info: None,
            error_category: None,
            response_status: Some(200),
            response_summary: None,
            captures_set: vec![],
            location: None,
            response_shape_mismatch: None,
        }
    }

    fn failing_step_status(name: &str) -> StepResult {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/json".into());
        StepResult {
            name: name.into(),
            description: None,
            debug: false,
            passed: false,
            duration_ms: 1,
            assertion_results: vec![AssertionResult::fail(
                "status",
                "200",
                "500",
                "Expected HTTP status 200, got 500",
            )],
            request_info: Some(RequestInfo {
                method: "POST".into(),
                url: "https://api.test/users".into(),
                headers: headers.clone(),
                body: None,
                multipart: None,
            }),
            response_info: Some(ResponseInfo {
                status: 500,
                headers,
                body: Some(serde_json::json!({"error": "boom"})),
            }),
            error_category: Some(FailureCategory::AssertionFailed),
            response_status: Some(500),
            response_summary: None,
            captures_set: vec![],
            location: None,
            response_shape_mismatch: None,
        }
    }

    fn cascade_step(name: &str) -> StepResult {
        StepResult {
            name: name.into(),
            description: None,
            debug: false,
            passed: false,
            duration_ms: 0,
            assertion_results: vec![AssertionResult::fail(
                "cascade",
                "prior captures available",
                "missing: user_id",
                "Skipped: capture user_id missing",
            )],
            request_info: None,
            response_info: None,
            error_category: Some(FailureCategory::SkippedDueToFailedCapture),
            response_status: None,
            response_summary: None,
            captures_set: vec![],
            location: None,
            response_shape_mismatch: None,
        }
    }

    fn shape_drift_step(name: &str, hint: ShapeMismatchDiagnosis) -> StepResult {
        let mut step = failing_step_status(name);
        step.error_category = Some(FailureCategory::ResponseShapeMismatch);
        step.response_shape_mismatch = Some(hint);
        step.assertion_results = vec![AssertionResult::fail(
            "body",
            "$.uuid",
            "missing",
            "JSONPath $.uuid did not match any value",
        )];
        step
    }

    fn wrap_file(name: &str, steps: Vec<StepResult>, test_name: &str) -> FileResult {
        let passed = steps.iter().all(|s| s.passed);
        FileResult {
            file: name.into(),
            name: name.into(),
            passed,
            duration_ms: 1,
            redaction: RedactionConfig::default(),
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![TestResult {
                name: test_name.into(),
                description: None,
                passed,
                duration_ms: 1,
                step_results: steps,
                captures: HashMap::new(),
            }],
            teardown_results: vec![],
        }
    }

    fn inputs<'a>(
        files: &'a [String],
        selectors: &'a [Selector],
        exit_code: i32,
    ) -> AgentReportInputs<'a> {
        AgentReportInputs {
            run_id: Some("rid-test".to_string()),
            exit_code,
            started_at: Utc::now(),
            ended_at: Utc::now(),
            selected_files: files,
            selectors,
            run_directory: None,
        }
    }

    // --- passed / failed status and budgets -----------------------------

    #[test]
    fn passing_run_has_no_root_causes_and_status_passed() {
        let run = RunResult {
            file_results: vec![wrap_file("a.tarn.yaml", vec![passing_step("s")], "t")],
            duration_ms: 1,
        };
        let files = vec!["a.tarn.yaml".to_string()];
        let report = build(&run, inputs(&files, &[], 0));
        assert_eq!(report.status, AgentStatus::Passed);
        assert!(report.root_causes.is_empty());
        assert_eq!(report.failed.root_causes, 0);
        assert_eq!(report.exit_code, 0);
    }

    #[test]
    fn cascade_folds_under_upstream_root_cause() {
        let run = RunResult {
            file_results: vec![wrap_file(
                "a.tarn.yaml",
                vec![
                    failing_step_status("create_user"),
                    cascade_step("followup_1"),
                    cascade_step("followup_2"),
                    cascade_step("followup_3"),
                ],
                "flow",
            )],
            duration_ms: 1,
        };
        let files = vec!["a.tarn.yaml".to_string()];
        let report = build(&run, inputs(&files, &[], 1));
        assert_eq!(report.status, AgentStatus::Failed);
        assert_eq!(report.root_causes.len(), 1);
        assert_eq!(report.root_causes[0].cascaded_steps.len(), 3);
        assert_eq!(report.failed.cascaded_skips, 3);
    }

    #[test]
    fn shape_drift_emits_replace_jsonpath_as_first_next_action() {
        let hint = ShapeMismatchDiagnosis {
            expected_path: "$.uuid".into(),
            observed_keys: vec!["request".into(), "stageStatus".into()],
            observed_type: "object".into(),
            candidate_fixes: vec![CandidateFix {
                path: "$.request.uuid".into(),
                confidence: ShapeConfidence::High,
                reason: "wrap".into(),
            }],
            high_confidence: true,
        };
        let run = RunResult {
            file_results: vec![wrap_file(
                "a.tarn.yaml",
                vec![shape_drift_step("check", hint.clone())],
                "t",
            )],
            duration_ms: 1,
        };
        let files = vec!["a.tarn.yaml".to_string()];
        let report = build(&run, inputs(&files, &[], 1));
        assert_eq!(report.root_causes.len(), 1);
        let rc = &report.root_causes[0];
        assert_eq!(
            rc.next_actions[0].kind,
            NextActionKind::ReplaceJsonpath,
            "replace_jsonpath must be the first action when we have a high-confidence candidate"
        );
        assert_eq!(
            rc.next_actions[0].suggestion.as_deref(),
            Some("$.request.uuid")
        );
        assert_eq!(
            rc.response_shape_mismatch
                .as_ref()
                .map(|d| d.expected_path.as_str()),
            Some("$.uuid")
        );
    }

    #[test]
    fn more_than_max_root_causes_truncates_and_emits_note() {
        // Build MAX_ROOT_CAUSES + 3 distinct-fingerprint primary failures
        // so the budget kicks in.
        let mut files: Vec<FileResult> = Vec::new();
        let mut file_names: Vec<String> = Vec::new();
        for i in 0..(MAX_ROOT_CAUSES + 3) {
            let name = format!("f{}.tarn.yaml", i);
            let mut step = failing_step_status("s");
            // Vary URL so each entry carries a unique fingerprint.
            if let Some(req) = step.request_info.as_mut() {
                req.url = format!("https://api.test/resource-{}", i);
            }
            files.push(wrap_file(&name, vec![step], "t"));
            file_names.push(name);
        }
        let run = RunResult {
            file_results: files,
            duration_ms: 1,
        };
        let report = build(&run, inputs(&file_names, &[], 1));
        assert_eq!(report.root_causes.len(), MAX_ROOT_CAUSES);
        assert_eq!(report.failed.root_causes, MAX_ROOT_CAUSES + 3);
        assert!(
            report.notes.iter().any(|n| n.contains("truncated")),
            "notes must carry the truncation advisory, got {:?}",
            report.notes
        );
    }

    #[test]
    fn selected_tests_populated_when_selectors_narrow_run() {
        let selectors = vec![Selector {
            file: "a.tarn.yaml".into(),
            test: Some("happy".into()),
            step: None,
        }];
        let run = RunResult {
            file_results: vec![wrap_file("a.tarn.yaml", vec![passing_step("s")], "happy")],
            duration_ms: 1,
        };
        let files = vec!["a.tarn.yaml".to_string()];
        let report = build(&run, inputs(&files, &selectors, 0));
        let selected_tests = report.selected.tests.expect("selected.tests populated");
        assert_eq!(selected_tests.len(), 1);
        assert_eq!(selected_tests[0].file, "a.tarn.yaml");
        assert_eq!(selected_tests[0].name, "happy");
        assert!(selected_tests[0].step.is_none());
    }

    #[test]
    fn selected_tests_omitted_when_only_file_level_selectors_present() {
        let selectors = vec![Selector {
            file: "a.tarn.yaml".into(),
            test: None,
            step: None,
        }];
        let run = RunResult {
            file_results: vec![wrap_file("a.tarn.yaml", vec![passing_step("s")], "t")],
            duration_ms: 1,
        };
        let files = vec!["a.tarn.yaml".to_string()];
        let report = build(&run, inputs(&files, &selectors, 0));
        assert!(report.selected.tests.is_none());
    }

    #[test]
    fn multiple_root_causes_offer_rerun_failed_action() {
        let run = RunResult {
            file_results: vec![
                wrap_file("a.tarn.yaml", vec![failing_step_status("x")], "t"),
                wrap_file(
                    "b.tarn.yaml",
                    vec![{
                        let mut s = failing_step_status("y");
                        if let Some(req) = s.request_info.as_mut() {
                            req.url = "https://api.test/other".into();
                        }
                        s
                    }],
                    "t",
                ),
            ],
            duration_ms: 1,
        };
        let files = vec!["a.tarn.yaml".to_string(), "b.tarn.yaml".to_string()];
        let report = build(&run, inputs(&files, &[], 1));
        assert_eq!(report.root_causes.len(), 2);
        for rc in &report.root_causes {
            assert!(
                rc.next_actions
                    .iter()
                    .any(|a| a.kind == NextActionKind::RerunFailed),
                "rerun_failed must appear when >=2 root causes exist",
            );
        }
    }

    #[test]
    fn connection_error_emits_check_server_reachable_with_host() {
        let mut step = failing_step_status("ping");
        step.error_category = Some(FailureCategory::ConnectionError);
        step.response_info = None;
        step.response_status = None;
        if let Some(req) = step.request_info.as_mut() {
            req.url = "http://127.0.0.1:9/health".into();
        }
        let run = RunResult {
            file_results: vec![wrap_file("a.tarn.yaml", vec![step], "t")],
            duration_ms: 1,
        };
        let files = vec!["a.tarn.yaml".to_string()];
        let report = build(&run, inputs(&files, &[], 3));
        assert_eq!(report.root_causes.len(), 1);
        let rc = &report.root_causes[0];
        let reach = rc
            .next_actions
            .iter()
            .find(|a| a.kind == NextActionKind::CheckServerReachable)
            .expect("check_server_reachable must be synthesized for ConnectionError");
        assert_eq!(reach.host.as_deref(), Some("127.0.0.1:9"));
    }

    #[test]
    fn artifacts_point_into_run_dir_when_provided() {
        let run = RunResult {
            file_results: vec![wrap_file("a.tarn.yaml", vec![passing_step("s")], "t")],
            duration_ms: 1,
        };
        let dir = std::path::PathBuf::from("/tmp/tarn-runs/run-x");
        let inputs = AgentReportInputs {
            run_id: Some("rid".into()),
            exit_code: 0,
            started_at: Utc::now(),
            ended_at: Utc::now(),
            selected_files: &["a.tarn.yaml".to_string()],
            selectors: &[],
            run_directory: Some(dir.as_path()),
        };
        let report = build(&run, inputs);
        assert_eq!(
            report.artifacts.run_dir.as_deref(),
            Some("/tmp/tarn-runs/run-x")
        );
        assert!(report
            .artifacts
            .report
            .as_deref()
            .map(|p| p.ends_with("report.json"))
            .unwrap_or(false));
        assert!(report
            .artifacts
            .events
            .as_deref()
            .map(|p| p.ends_with("events.jsonl"))
            .unwrap_or(false));
    }

    #[test]
    fn render_json_roundtrip_preserves_schema_version_and_status() {
        let run = RunResult {
            file_results: vec![wrap_file("a.tarn.yaml", vec![passing_step("s")], "t")],
            duration_ms: 1,
        };
        let files = vec!["a.tarn.yaml".to_string()];
        let report = build(&run, inputs(&files, &[], 0));
        let text = render_json(&report);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["schema_version"], AGENT_REPORT_SCHEMA_VERSION);
        assert_eq!(v["status"], "passed");
        assert_eq!(v["exit_code"], 0);
    }

    #[test]
    fn body_excerpt_trimmed_to_agent_budget() {
        let long = "a".repeat(AGENT_BODY_EXCERPT_MAX_CHARS + 200);
        let trimmed = trim_excerpt(&long);
        assert!(trimmed.contains("…[truncated]"));
        let prefix: String = trimmed.chars().take(AGENT_BODY_EXCERPT_MAX_CHARS).collect();
        assert!(
            !prefix.contains("…"),
            "prefix must be untruncated up to the cap"
        );
    }
}
