//! Triage-sized artifacts a run emits next to the full JSON report.
//!
//! The full `report.json` carries every request, response, capture and
//! assertion — great for deep inspection, too large for the common
//! "what failed, why?" triage loop. `summary.json` and `failures.json`
//! cover that loop without forcing the consumer to parse the full
//! report:
//!
//! - `summary.json` — counts, timings, exit code, the list of failed
//!   files. Always emitted (even when every test passed) so tooling
//!   can key off a single stable artifact.
//! - `failures.json` — one entry per failing step with file/test/step
//!   coordinates, failure category, message, request/response summary
//!   and, when trivially derivable, a pointer to the upstream root
//!   cause. Always emitted (with `failures: []` on a clean run) for
//!   the same reason.
//!
//! The writers are atomic: each document is written to a `.tmp`
//! sibling and then renamed into place, mirroring `state_writer.rs`.

use crate::assert::types::{FailureCategory, FileResult, RunResult, StepResult};
use crate::fixtures::{SETUP_TEST_SLUG, TEARDOWN_TEST_SLUG};
use crate::model::RedactionConfig;
use crate::report::redaction::{sanitize_json, sanitize_string};
use crate::report::rerun::RerunSource;
use crate::report::shape_diagnosis::ShapeMismatchDiagnosis;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Bumped on incompatible changes to either artifact's envelope.
pub const SUMMARY_SCHEMA_VERSION: u32 = 1;

/// Maximum length of the `body_excerpt` carried in `failures.json`,
/// measured in characters. Chosen to fit "what does this response
/// look like?" into a compact artifact without pulling in the full
/// body (which lives in `report.json`).
pub const BODY_EXCERPT_MAX_CHARS: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryDoc {
    pub schema_version: u32,
    pub run_id: Option<String>,
    pub started_at: String,
    pub ended_at: String,
    pub duration_ms: u64,
    pub exit_code: i32,
    pub totals: Counts,
    pub failed: Counts,
    pub failed_files: Vec<String>,
    /// NAZ-403: populated when the run was produced by `tarn rerun`.
    /// Omitted from the serialized form on normal runs to keep the
    /// artifact byte-identical to prior versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerun_source: Option<RerunSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Counts {
    pub files: usize,
    pub tests: usize,
    pub steps: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailuresDoc {
    pub schema_version: u32,
    pub run_id: Option<String>,
    pub failures: Vec<FailureEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureEntry {
    pub file: String,
    pub test: String,
    pub step: String,
    #[serde(default)]
    pub failure_category: Option<FailureCategory>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<FailureRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<FailureResponse>,
    #[serde(default)]
    pub root_cause: Option<RootCauseRef>,
    /// Structured response-shape drift hint (NAZ-415). Present when
    /// the step's JSONPath miss was diagnosed as contract drift — the
    /// agent-consumable observed shape + candidate fix paths. Omitted
    /// for non-drift failures to keep the artifact compact and
    /// byte-identical to prior schema for unrelated categories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_shape_mismatch: Option<ShapeMismatchDiagnosis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureRequest {
    pub method: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureResponse {
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_excerpt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootCauseRef {
    pub file: String,
    pub test: String,
    pub step: String,
}

/// Build the summary + failures documents from a finished run.
///
/// `rerun_source` is stamped into the [`SummaryDoc`] when the run was
/// produced by `tarn rerun`; regular runs pass `None` and the field is
/// omitted from the serialized artifact.
pub fn build_summary_and_failures(
    result: &RunResult,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    exit_code: i32,
    run_id: Option<String>,
    rerun_source: Option<RerunSource>,
) -> (SummaryDoc, FailuresDoc) {
    let mut totals = Counts {
        files: result.file_results.len(),
        tests: 0,
        steps: 0,
    };
    let mut failed = Counts {
        files: 0,
        tests: 0,
        steps: 0,
    };
    let mut failed_files: Vec<String> = Vec::new();
    let mut failures: Vec<FailureEntry> = Vec::new();

    for file in &result.file_results {
        for step in &file.setup_results {
            totals.steps += 1;
            if !step.passed {
                failed.steps += 1;
                failures.push(build_failure_entry(file, SETUP_TEST_SLUG, step, None));
            }
        }

        for test in &file.test_results {
            totals.tests += 1;
            if !test.passed {
                failed.tests += 1;
            }
            for step in &test.step_results {
                totals.steps += 1;
                if !step.passed {
                    failed.steps += 1;
                    let root_cause = resolve_root_cause(file, &test.name, step);
                    failures.push(build_failure_entry(file, &test.name, step, root_cause));
                }
            }
        }

        for step in &file.teardown_results {
            totals.steps += 1;
            if !step.passed {
                failed.steps += 1;
                failures.push(build_failure_entry(file, TEARDOWN_TEST_SLUG, step, None));
            }
        }

        if !file.passed {
            failed.files += 1;
            failed_files.push(file.file.clone());
        }
    }

    let duration_ms = ended_at
        .signed_duration_since(started_at)
        .num_milliseconds()
        .max(0) as u64;

    let summary = SummaryDoc {
        schema_version: SUMMARY_SCHEMA_VERSION,
        run_id: run_id.clone(),
        started_at: started_at.to_rfc3339(),
        ended_at: ended_at.to_rfc3339(),
        duration_ms,
        exit_code,
        totals,
        failed,
        failed_files,
        rerun_source,
    };

    let failures_doc = FailuresDoc {
        schema_version: SUMMARY_SCHEMA_VERSION,
        run_id,
        failures,
    };

    (summary, failures_doc)
}

fn build_failure_entry(
    file: &FileResult,
    test_label: &str,
    step: &StepResult,
    root_cause: Option<RootCauseRef>,
) -> FailureEntry {
    let secrets = file.redacted_values.clone();
    let redaction = &file.redaction;
    let message = sanitize_string(
        &primary_failure_message(step),
        &redaction.replacement,
        &secrets,
    );

    let request = step.request_info.as_ref().map(|req| FailureRequest {
        method: req.method.clone(),
        url: sanitize_string(&req.url, &redaction.replacement, &secrets),
    });

    let response = step
        .response_info
        .as_ref()
        .map(|resp| FailureResponse {
            status: Some(resp.status),
            body_excerpt: resp
                .body
                .as_ref()
                .and_then(|body| body_excerpt(body, redaction, &secrets)),
        })
        .or_else(|| {
            step.response_status.map(|status| FailureResponse {
                status: Some(status),
                body_excerpt: None,
            })
        });

    FailureEntry {
        file: file.file.clone(),
        test: test_label.to_string(),
        step: step.name.clone(),
        failure_category: step.error_category,
        message,
        request,
        response,
        root_cause,
        response_shape_mismatch: step.response_shape_mismatch.clone(),
    }
}

fn primary_failure_message(step: &StepResult) -> String {
    step.assertion_results
        .iter()
        .find(|a| !a.passed)
        .map(|a| a.message.clone())
        .unwrap_or_else(|| "step failed".to_string())
}

fn body_excerpt(
    body: &serde_json::Value,
    redaction: &RedactionConfig,
    secrets: &[String],
) -> Option<String> {
    let sanitized = sanitize_json(body, &redaction.replacement, secrets);
    let serialized = serde_json::to_string(&sanitized).ok()?;
    if serialized.is_empty() {
        return None;
    }
    if serialized.chars().count() <= BODY_EXCERPT_MAX_CHARS {
        return Some(serialized);
    }
    let end = serialized
        .char_indices()
        .take(BODY_EXCERPT_MAX_CHARS)
        .last()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    Some(format!(
        "{}…[truncated, {} chars]",
        &serialized[..end],
        serialized.chars().count()
    ))
}

/// Best-effort upstream pointer for cascade failures. We only set this
/// when the step's own category explicitly marks it as cascade fallout
/// (`SkippedDueToFailedCapture` / `SkippedDueToFailFast`); everything
/// else is left as `null` so NAZ-402 can do proper grouping.
///
/// For `SkippedDueToFailedCapture`, we search earlier steps in the same
/// test for the one whose declared captures were not produced. For
/// `SkippedDueToFailFast`, the root cause is the first preceding failed
/// step in the test.
fn resolve_root_cause(
    file: &FileResult,
    test_name: &str,
    step: &StepResult,
) -> Option<RootCauseRef> {
    let category = step.error_category?;
    let test = file.test_results.iter().find(|t| t.name == test_name)?;
    let step_index = test
        .step_results
        .iter()
        .position(|s| std::ptr::eq(s, step))?;

    let upstream = match category {
        FailureCategory::SkippedDueToFailedCapture => {
            let missing = extract_missing_capture_names(step);
            test.step_results[..step_index].iter().rfind(|prior| {
                !prior.passed
                    && missing
                        .iter()
                        .any(|name| !prior.captures_set.iter().any(|c| c == name))
            })
        }
        FailureCategory::SkippedDueToFailFast => test.step_results[..step_index]
            .iter()
            .rfind(|prior| !prior.passed && !is_cascade_category(prior.error_category)),
        _ => None,
    }?;

    Some(RootCauseRef {
        file: file.file.clone(),
        test: test_name.to_string(),
        step: upstream.name.clone(),
    })
}

fn is_cascade_category(category: Option<FailureCategory>) -> bool {
    matches!(
        category,
        Some(FailureCategory::SkippedDueToFailedCapture)
            | Some(FailureCategory::SkippedDueToFailFast)
    )
}

/// Pull capture names out of the synthetic `cascade` assertion the
/// runner emits on `SkippedDueToFailedCapture`. The assertion's
/// `actual` is formatted as `missing: a, b, c`, so we split on the
/// colon + commas. An empty result leaves the root cause at `null`.
fn extract_missing_capture_names(step: &StepResult) -> Vec<String> {
    step.assertion_results
        .iter()
        .find(|a| a.assertion == "cascade")
        .and_then(|a| a.actual.strip_prefix("missing: "))
        .map(|rest| {
            rest.split(',')
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Persist `<dir>/summary.json` atomically.
pub fn write_summary_to_dir(dir: &Path, summary: &SummaryDoc) -> std::io::Result<PathBuf> {
    write_json_atomic(dir, "summary.json", summary)
}

/// Persist `<dir>/failures.json` atomically.
pub fn write_failures_to_dir(dir: &Path, failures: &FailuresDoc) -> std::io::Result<PathBuf> {
    write_json_atomic(dir, "failures.json", failures)
}

fn write_json_atomic<T: Serialize>(
    dir: &Path,
    file_name: &str,
    payload: &T,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name);
    let tmp = dir.join(format!("{}.tmp", file_name));
    let encoded = serde_json::to_vec_pretty(payload)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, encoded)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::{AssertionResult, RequestInfo, ResponseInfo, TestResult};
    use std::collections::HashMap;
    use tempfile::TempDir;

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
            ..Default::default()
        }
    }

    fn failing_step_with_http(name: &str) -> StepResult {
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
                "status mismatch: expected 200, got 500",
            )],
            request_info: Some(RequestInfo {
                method: "GET".into(),
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
            ..Default::default()
        }
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

    #[test]
    fn passing_run_yields_zero_failure_counts() {
        let run = RunResult {
            file_results: vec![wrap_file("a.tarn.yaml", vec![passing_step("s1")], "t1")],
            duration_ms: 1,
        };
        let (summary, failures) =
            build_summary_and_failures(&run, Utc::now(), Utc::now(), 0, Some("rid".into()), None);
        assert_eq!(summary.totals.files, 1);
        assert_eq!(summary.totals.tests, 1);
        assert_eq!(summary.totals.steps, 1);
        assert_eq!(summary.failed.files, 0);
        assert_eq!(summary.failed.tests, 0);
        assert_eq!(summary.failed.steps, 0);
        assert!(summary.failed_files.is_empty());
        assert_eq!(summary.exit_code, 0);
        assert!(failures.failures.is_empty());
    }

    #[test]
    fn failing_run_populates_failures_with_request_response_and_status() {
        let run = RunResult {
            file_results: vec![wrap_file(
                "tests/users.tarn.yaml",
                vec![passing_step("health"), failing_step_with_http("list")],
                "happy",
            )],
            duration_ms: 1,
        };
        let (summary, failures) =
            build_summary_and_failures(&run, Utc::now(), Utc::now(), 1, Some("rid".into()), None);
        assert_eq!(summary.totals.steps, 2);
        assert_eq!(summary.failed.steps, 1);
        assert_eq!(summary.failed.tests, 1);
        assert_eq!(summary.failed.files, 1);
        assert_eq!(summary.failed_files, vec!["tests/users.tarn.yaml"]);
        assert_eq!(failures.failures.len(), 1);
        let f = &failures.failures[0];
        assert_eq!(f.file, "tests/users.tarn.yaml");
        assert_eq!(f.test, "happy");
        assert_eq!(f.step, "list");
        assert_eq!(f.failure_category, Some(FailureCategory::AssertionFailed));
        assert!(f.message.contains("status mismatch"));
        let req = f.request.as_ref().expect("request captured");
        assert_eq!(req.method, "GET");
        assert_eq!(req.url, "https://api.test/users");
        let resp = f.response.as_ref().expect("response captured");
        assert_eq!(resp.status, Some(500));
        assert_eq!(resp.body_excerpt.as_deref(), Some(r#"{"error":"boom"}"#));
    }

    #[test]
    fn body_excerpt_is_truncated_past_limit() {
        let huge = "x".repeat(BODY_EXCERPT_MAX_CHARS + 200);
        let value = serde_json::json!({ "data": huge });
        let excerpt = body_excerpt(&value, &RedactionConfig::default(), &[]).unwrap();
        assert!(
            excerpt.contains("…[truncated,"),
            "expected truncation marker, got: {}",
            excerpt
        );
        assert!(
            excerpt.chars().count() > BODY_EXCERPT_MAX_CHARS,
            "marker is appended past the cap"
        );
        // Untruncated prefix must not exceed the cap.
        let prefix: String = excerpt.chars().take(BODY_EXCERPT_MAX_CHARS).collect();
        assert!(!prefix.contains("…"));
    }

    #[test]
    fn body_excerpt_respects_redaction() {
        let value = serde_json::json!({ "token": "super-secret" });
        let redaction = RedactionConfig {
            replacement: "***".into(),
            ..RedactionConfig::default()
        };
        let excerpt = body_excerpt(&value, &redaction, &["super-secret".into()]).unwrap();
        assert!(
            !excerpt.contains("super-secret"),
            "redacted secret leaked: {}",
            excerpt
        );
        assert!(excerpt.contains("***"));
    }

    #[test]
    fn write_summary_and_failures_atomic_and_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let run = RunResult {
            file_results: vec![wrap_file("a.tarn.yaml", vec![passing_step("s")], "t")],
            duration_ms: 1,
        };
        let (summary, failures_doc) =
            build_summary_and_failures(&run, Utc::now(), Utc::now(), 0, Some("rid".into()), None);
        let s_path = write_summary_to_dir(tmp.path(), &summary).unwrap();
        let f_path = write_failures_to_dir(tmp.path(), &failures_doc).unwrap();
        assert!(s_path.is_file());
        assert!(f_path.is_file());
        assert!(!tmp.path().join("summary.json.tmp").exists());
        assert!(!tmp.path().join("failures.json.tmp").exists());
        let s_round: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&s_path).unwrap()).unwrap();
        assert_eq!(s_round["schema_version"], SUMMARY_SCHEMA_VERSION);
        assert_eq!(s_round["run_id"], "rid");
        let f_round: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&f_path).unwrap()).unwrap();
        assert!(f_round["failures"].as_array().unwrap().is_empty());
    }

    #[test]
    fn cascade_skip_root_cause_points_at_upstream_step() {
        let mut upstream = failing_step_with_http("create_user");
        upstream.error_category = Some(FailureCategory::AssertionFailed);
        // `captures_set` stays empty so the capture is marked "not produced".
        let cascade_msg = "Skipped: step references capture(s) that failed earlier in this test: \
             user_id. Fix the root-cause step first — this cascade failure is a direct \
             consequence.";
        let cascade = StepResult {
            name: "delete_user".into(),
            description: None,
            debug: false,
            passed: false,
            duration_ms: 0,
            assertion_results: vec![AssertionResult::fail(
                "cascade",
                "prior captures available".to_string(),
                "missing: user_id".to_string(),
                cascade_msg,
            )],
            request_info: None,
            response_info: None,
            error_category: Some(FailureCategory::SkippedDueToFailedCapture),
            response_status: None,
            response_summary: None,
            captures_set: vec![],
            location: None,
            response_shape_mismatch: None,
            ..Default::default()
        };
        let mut file = wrap_file("a.tarn.yaml", vec![upstream, cascade], "happy");
        // Mark the upstream step's declared capture as unfulfilled by
        // making the test as a whole failed.
        file.passed = false;
        file.test_results[0].passed = false;
        let run = RunResult {
            file_results: vec![file],
            duration_ms: 0,
        };
        let (_, failures_doc) =
            build_summary_and_failures(&run, Utc::now(), Utc::now(), 1, Some("rid".into()), None);
        let delete_failure = failures_doc
            .failures
            .iter()
            .find(|f| f.step == "delete_user")
            .expect("delete step failure emitted");
        let root = delete_failure
            .root_cause
            .as_ref()
            .expect("root_cause populated for cascade");
        assert_eq!(root.step, "create_user");
        assert_eq!(root.test, "happy");
    }
}
