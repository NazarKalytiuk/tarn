//! Integration tests for `.tarn/state.json` — the human-readable
//! sidecar the LLM tooling reads when answering "what just happened?".
//!
//! These tests drive `tarn::report::state_writer::write_state`
//! directly because the higher-level `execute_run` helper lives in
//! the `tarn` binary and is not reachable from a library test.

use chrono::Utc;
use std::collections::HashMap;
use tarn::assert::types::{
    AssertionResult, FailureCategory, FileResult, RunResult, StepResult, TestResult,
};
use tarn::model::RedactionConfig;
use tarn::report::state_writer::{build_state, write_state, StateDoc, STATE_SCHEMA_VERSION};
use tempfile::TempDir;

fn mk_run(passing: bool) -> RunResult {
    RunResult {
        file_results: vec![FileResult {
            file: "tests/users.tarn.yaml".into(),
            name: "Users".into(),
            passed: passing,
            duration_ms: 42,
            redaction: RedactionConfig::default(),
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![TestResult {
                name: "happy".into(),
                description: None,
                passed: passing,
                duration_ms: 42,
                step_results: vec![StepResult {
                    name: "list".into(),
                    description: None,
                    debug: false,
                    passed: passing,
                    duration_ms: 42,
                    assertion_results: if passing {
                        vec![AssertionResult::pass("status", "200", "200")]
                    } else {
                        vec![AssertionResult::fail(
                            "status",
                            "200",
                            "500",
                            "status mismatch: expected 200, got 500",
                        )]
                    },
                    request_info: None,
                    response_info: None,
                    error_category: if passing {
                        None
                    } else {
                        Some(FailureCategory::AssertionFailed)
                    },
                    response_status: Some(if passing { 200 } else { 500 }),
                    response_summary: None,
                    captures_set: vec![],
                    location: None,
                }],
                captures: HashMap::new(),
            }],
            teardown_results: vec![],
        }],
        duration_ms: 42,
    }
}

#[test]
fn state_json_is_written_at_end_of_run() {
    let tmp = TempDir::new().unwrap();
    let started = Utc::now();
    let ended = Utc::now();
    let state = build_state(
        &mk_run(true),
        started,
        ended,
        0,
        &["tarn".into(), "run".into()],
        Some("local".into()),
        Some("https://x.test".into()),
    );
    let written = write_state(tmp.path(), &state).expect("write_state ok");
    assert!(written.is_file(), "state.json must exist at {}", written.display());
    assert!(
        !tmp.path().join(".tarn/state.json.tmp").exists(),
        "tmp file must be gone after the atomic rename"
    );
    let round: StateDoc = serde_json::from_slice(&std::fs::read(&written).unwrap()).unwrap();
    assert_eq!(round.schema_version, STATE_SCHEMA_VERSION);
    assert_eq!(round.last_run.exit_code, 0);
    assert_eq!(round.last_run.passed, 1);
    assert_eq!(round.env.base_url.as_deref(), Some("https://x.test"));
}

#[test]
fn state_json_exit_code_and_failures_reflect_run_result() {
    let tmp = TempDir::new().unwrap();
    let state = build_state(
        &mk_run(false),
        Utc::now(),
        Utc::now(),
        1,
        &["tarn".into(), "run".into()],
        None,
        None,
    );
    let written = write_state(tmp.path(), &state).expect("write_state ok");
    let doc: StateDoc = serde_json::from_slice(&std::fs::read(&written).unwrap()).unwrap();
    assert_eq!(doc.last_run.exit_code, 1);
    assert_eq!(doc.last_run.failed, 1);
    assert_eq!(doc.failures.len(), 1);
    let fail = &doc.failures[0];
    assert_eq!(fail.test, "happy");
    assert_eq!(fail.step, "list");
    assert!(fail.message.contains("status mismatch"));
}

#[test]
fn state_write_is_atomic_leftover_tmp_does_not_corrupt_final_payload() {
    // Simulate a previous crash that left a truncated `.tmp` file.
    // The next successful write must still land a complete
    // `state.json` without merging the old garbage.
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(".tarn");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("state.json.tmp"), b"{ garbage from earlier crash").unwrap();

    let state = build_state(
        &mk_run(true),
        Utc::now(),
        Utc::now(),
        0,
        &["tarn".into()],
        None,
        None,
    );
    write_state(tmp.path(), &state).expect("write_state ok");

    // The final file must parse — the crashed tmp must have been
    // overwritten cleanly, and the rename target must never have
    // held the garbage.
    let final_path = dir.join("state.json");
    let doc: StateDoc = serde_json::from_slice(&std::fs::read(&final_path).unwrap()).unwrap();
    assert_eq!(doc.schema_version, STATE_SCHEMA_VERSION);
    assert_eq!(doc.last_run.passed, 1);
    // The writer cleans up tmp files it owns via `rename`. A stale
    // tmp from a previous crash was overwritten before the rename,
    // so the final directory now contains only `state.json`.
    assert!(!dir.join("state.json.tmp").exists());
}
