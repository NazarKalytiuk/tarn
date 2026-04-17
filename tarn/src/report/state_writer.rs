//! Writer for `<workspace-root>/.tarn/state.json`, the human-readable
//! sidecar LLM tooling consumes to answer "what just happened?".
//!
//! Where `.tarn/last-run.json` mirrors the machine-readable JSON
//! report (full request/response bodies on every failure, structured
//! for diffing), `state.json` is a condensed, action-oriented summary:
//! when did the run start, how many tests passed, which ones failed,
//! what command-line arguments were used. The fields are stable and
//! versioned through `schema_version`.
//!
//! The writer is atomic: it writes to `state.json.tmp` and renames on
//! success. A crash mid-write never leaves the file half-populated.

use crate::assert::types::{RunResult, StepResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Bumped whenever we make an incompatible change to the `state.json`
/// envelope. Readers must refuse to parse a newer schema version and
/// treat `no state.json` as the graceful fallback.
pub const STATE_SCHEMA_VERSION: u32 = 1;

/// Envelope written to `state.json`. Field order matches the ticket's
/// template so a human inspecting the file can read it top-to-bottom.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDoc {
    /// Version tag. Bumped when `StateDoc` itself gains or loses a
    /// field in an incompatible way.
    pub schema_version: u32,
    /// Summary of the most recent run.
    pub last_run: LastRun,
    /// Per-failure breakdown, sorted by file/test/step. Empty when
    /// every test passed.
    #[serde(default)]
    pub failures: Vec<Failure>,
    /// Reserved for NAZ-256's debug session wiring. Written as `null`
    /// today; the debug ticket fills this in without bumping
    /// `schema_version` because `null` is already a valid value.
    pub debug_session: Option<serde_json::Value>,
    /// Resolved environment metadata — which named environment was
    /// used and the effective `base_url`, when available.
    pub env: StateEnv,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastRun {
    /// ISO 8601 timestamp the run started at.
    pub started_at: String,
    /// ISO 8601 timestamp the run completed at.
    pub ended_at: String,
    /// Number of tests that passed.
    pub passed: usize,
    /// Number of tests that failed.
    pub failed: usize,
    /// Process exit code reported by the runner.
    pub exit_code: i32,
    /// The `argv` that produced the run. Helps the LLM reproduce a
    /// failure verbatim without re-deriving flags.
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    pub file: String,
    pub test: String,
    pub step: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateEnv {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// Persist a [`StateDoc`] to `<root>/.tarn/state.json` atomically.
pub fn write_state(root: &Path, state: &StateDoc) -> std::io::Result<PathBuf> {
    let dir = root.join(".tarn");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("state.json");
    let tmp = dir.join("state.json.tmp");
    let encoded = serde_json::to_vec_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, encoded)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Build a [`StateDoc`] from a run's results + metadata.
///
/// Kept pure so tests can construct an expected state doc without
/// writing to disk. `started_at` / `ended_at` / `exit_code` / `args`
/// come from the caller because the runner crate doesn't own argv or
/// know the host process's exit disposition.
pub fn build_state(
    result: &RunResult,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    exit_code: i32,
    args: &[String],
    env_name: Option<String>,
    base_url: Option<String>,
) -> StateDoc {
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut failures: Vec<Failure> = Vec::new();

    for file in &result.file_results {
        for test in &file.test_results {
            if test.passed {
                passed += 1;
            } else {
                failed += 1;
                for step in &test.step_results {
                    if !step.passed {
                        failures.push(Failure {
                            file: file.file.clone(),
                            test: test.name.clone(),
                            step: step.name.clone(),
                            message: primary_failure_message(step),
                        });
                    }
                }
            }
        }
        // Setup / teardown failures also count so the failure list
        // surfaces root causes that happened outside named tests.
        for step in &file.setup_results {
            if !step.passed {
                failures.push(Failure {
                    file: file.file.clone(),
                    test: crate::fixtures::SETUP_TEST_SLUG.to_string(),
                    step: step.name.clone(),
                    message: primary_failure_message(step),
                });
            }
        }
        for step in &file.teardown_results {
            if !step.passed {
                failures.push(Failure {
                    file: file.file.clone(),
                    test: crate::fixtures::TEARDOWN_TEST_SLUG.to_string(),
                    step: step.name.clone(),
                    message: primary_failure_message(step),
                });
            }
        }
    }

    StateDoc {
        schema_version: STATE_SCHEMA_VERSION,
        last_run: LastRun {
            started_at: started_at.to_rfc3339(),
            ended_at: ended_at.to_rfc3339(),
            passed,
            failed,
            exit_code,
            args: args.to_vec(),
        },
        failures,
        debug_session: None,
        env: StateEnv {
            name: env_name,
            base_url,
        },
    }
}

fn primary_failure_message(step: &StepResult) -> String {
    step.assertion_results
        .iter()
        .find(|a| !a.passed)
        .map(|a| a.message.clone())
        .unwrap_or_else(|| "step failed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::{
        AssertionResult, FailureCategory, FileResult, StepResult, TestResult,
    };
    use crate::model::RedactionConfig;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn mk_run(passing_file: bool) -> RunResult {
        RunResult {
            file_results: vec![FileResult {
                file: "tests/a.tarn.yaml".into(),
                name: "A".into(),
                passed: passing_file,
                duration_ms: 100,
                redaction: RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "t1".into(),
                    description: None,
                    passed: passing_file,
                    duration_ms: 100,
                    step_results: vec![StepResult {
                        name: "s1".into(),
                        passed: passing_file,
                        duration_ms: 100,
                        assertion_results: if passing_file {
                            vec![AssertionResult::pass("status", "200", "200")]
                        } else {
                            vec![AssertionResult::fail(
                                "status", "200", "500", "boom",
                            )]
                        },
                        request_info: None,
                        response_info: None,
                        error_category: if passing_file {
                            None
                        } else {
                            Some(FailureCategory::AssertionFailed)
                        },
                        response_status: Some(if passing_file { 200 } else { 500 }),
                        response_summary: None,
                        captures_set: vec![],
                        location: None,
                    }],
                    captures: HashMap::new(),
                }],
                teardown_results: vec![],
            }],
            duration_ms: 100,
        }
    }

    #[test]
    fn build_state_counts_passed_and_failed_tests() {
        let run = mk_run(true);
        let state = build_state(
            &run,
            Utc::now(),
            Utc::now(),
            0,
            &["tarn".into(), "run".into()],
            Some("local".into()),
            Some("https://x.test".into()),
        );
        assert_eq!(state.last_run.passed, 1);
        assert_eq!(state.last_run.failed, 0);
        assert_eq!(state.last_run.exit_code, 0);
        assert_eq!(state.env.name.as_deref(), Some("local"));
        assert!(state.failures.is_empty());
    }

    #[test]
    fn build_state_emits_failures_with_primary_message() {
        let run = mk_run(false);
        let state = build_state(
            &run,
            Utc::now(),
            Utc::now(),
            1,
            &["tarn".into(), "run".into()],
            None,
            None,
        );
        assert_eq!(state.last_run.failed, 1);
        assert_eq!(state.failures.len(), 1);
        assert_eq!(state.failures[0].file, "tests/a.tarn.yaml");
        assert_eq!(state.failures[0].test, "t1");
        assert_eq!(state.failures[0].step, "s1");
        assert_eq!(state.failures[0].message, "boom");
    }

    #[test]
    fn write_state_is_atomic_and_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let run = mk_run(true);
        let state = build_state(
            &run,
            Utc::now(),
            Utc::now(),
            0,
            &["tarn".into(), "run".into()],
            None,
            None,
        );
        let written = write_state(tmp.path(), &state).unwrap();
        assert!(written.is_file());
        assert!(!tmp.path().join(".tarn/state.json.tmp").exists());
        let bytes = std::fs::read(&written).unwrap();
        let round: StateDoc = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(round.schema_version, STATE_SCHEMA_VERSION);
        assert_eq!(round.last_run.passed, 1);
    }
}
