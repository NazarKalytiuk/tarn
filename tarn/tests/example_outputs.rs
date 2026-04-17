//! One-shot test that captures the JSON shapes the runner produces
//! under `.tarn/fixtures/**` and `.tarn/state.json` so a PR reviewer
//! can inspect the wire format without running an end-to-end demo.
//!
//! The test is `#[ignore]` by default because printing JSON pollutes
//! the normal test output — run with `cargo test -- --ignored
//! example_outputs --nocapture` to see the sample documents.

use chrono::Utc;
use std::collections::HashMap;
use tarn::assert::types::{
    AssertionResult, FailureCategory, FileResult, RequestInfo, ResponseInfo, RunResult, StepResult,
    TestResult,
};
use tarn::fixtures::{file_path_hash, slugify_name};
use tarn::model::RedactionConfig;
use tarn::report::fixture_writer::{self, FixtureWriteConfig};
use tarn::report::state_writer::{build_state, write_state};
use tempfile::TempDir;

#[test]
#[ignore]
fn example_outputs() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let file = root.join("tests/users.tarn.yaml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "name: Users\n").unwrap();

    // Build a realistic failed-step `StepResult` so the fixture
    // carries a non-trivial response + failure_message.
    let step = StepResult {
        name: "list users".into(),
        passed: false,
        duration_ms: 42,
        assertion_results: vec![AssertionResult::fail(
            "status",
            "200",
            "500",
            "status: expected 200, got 500",
        )],
        request_info: Some(RequestInfo {
            method: "GET".into(),
            url: "http://127.0.0.1:52341/users".into(),
            headers: HashMap::from([("accept".into(), "application/json".into())]),
            body: None,
            multipart: None,
        }),
        response_info: Some(ResponseInfo {
            status: 500,
            headers: HashMap::from([("content-type".into(), "application/json".into())]),
            body: Some(serde_json::json!({"error": "database unavailable"})),
        }),
        error_category: Some(FailureCategory::AssertionFailed),
        response_status: Some(500),
        response_summary: Some("500 Internal Server Error".into()),
        captures_set: vec![],
        location: None,
    };
    let fixture = fixture_writer::build_fixture(&step, &RedactionConfig::default(), &[]);
    let config = FixtureWriteConfig {
        enabled: true,
        workspace_root: root.clone(),
        retention: 5,
    };
    fixture_writer::write_step_fixture(&config, &file, "happy", 0, &fixture).unwrap();

    let step_dir = root
        .join(".tarn/fixtures")
        .join(file_path_hash(&root, &file))
        .join(slugify_name("happy"))
        .join("0");
    let history = std::fs::read_dir(&step_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".json") && n != "_index.json" && !n.ends_with(".tmp"))
        .expect("history file");
    let raw = std::fs::read_to_string(step_dir.join(&history)).unwrap();
    eprintln!("=== example fixture JSON ({}) ===\n{raw}", history);

    // Build a matching state.json with one failure.
    let run = RunResult {
        file_results: vec![FileResult {
            file: file.to_string_lossy().into_owned(),
            name: "Users".into(),
            passed: false,
            duration_ms: 80,
            redaction: RedactionConfig::default(),
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![TestResult {
                name: "happy".into(),
                description: None,
                passed: false,
                duration_ms: 80,
                step_results: vec![step],
                captures: HashMap::new(),
            }],
            teardown_results: vec![],
        }],
        duration_ms: 80,
    };
    let state = build_state(
        &run,
        Utc::now(),
        Utc::now(),
        1,
        &["run".into(), "--no-progress".into()],
        Some("local".into()),
        Some("http://127.0.0.1:52341".into()),
    );
    write_state(&root, &state).unwrap();
    let state_raw = std::fs::read_to_string(root.join(".tarn/state.json")).unwrap();
    eprintln!("=== example state.json ===\n{state_raw}");
}
