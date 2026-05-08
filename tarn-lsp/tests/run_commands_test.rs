//! Integration tests for the NAZ-256 run-from-gutter commands
//! (`tarn.runFile`, `tarn.runTest`, `tarn.runStep`,
//! `tarn.runLastFailures`) and the companion diff command
//! (`tarn.diffLastPassing`). The tests drive the pure handler surface
//! from `tarn_lsp::run_commands` / `tarn_lsp::diff` without spinning up
//! the full LSP loop — the stdio transport is already covered by the
//! other integration tests and would only add flakiness here.

use std::fs;
use std::path::PathBuf;

use serde_json::json;
use tarn_lsp::diff::{execute_diff_last_passing, DiffLastPassingArgs, InMemoryFixtureSource};
use tarn_lsp::run_commands::{
    execute_run_file, execute_run_test, extract_failures, CapturingSink, LastRunArtifact,
    RunFileArgs, RunTestArgs, PROGRESS_NOTIFICATION,
};
use tempfile::TempDir;

/// A minimal `.tarn.yaml` whose only step targets a closed TCP port.
/// The step fails immediately with a connection error, which is enough
/// to exercise every code path in the run-from-gutter handlers without
/// needing a live demo server.
const DRY_FIXTURE: &str = r#"name: dry fixture
tests:
  alpha:
    steps:
      - name: ping
        request:
          method: GET
          url: "http://127.0.0.1:1/health"
        timeout: 50
  beta:
    steps:
      - name: probe
        request:
          method: GET
          url: "http://127.0.0.1:1/health"
        timeout: 50
"#;

fn write_fixture(dir: &TempDir) -> PathBuf {
    let path = dir.path().join("debug.tarn.yaml");
    fs::write(&path, DRY_FIXTURE).unwrap();
    path
}

#[test]
fn run_file_returns_wrapped_envelope_with_schema_version() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir);

    let sink = CapturingSink::new();
    let envelope = execute_run_file(
        &RunFileArgs {
            file: path.display().to_string(),
            env: None,
        },
        &sink,
    )
    .expect("run_file ok");

    assert_eq!(envelope.schema_version, 1);
    // Data must be a RunResult-shaped JSON document.
    let data = &envelope.data;
    assert!(data["files"].is_array(), "data.files must be array");
    assert!(
        data["summary"]["steps"]["total"].as_u64().unwrap_or(0) >= 1,
        "summary must tally at least one step"
    );
}

#[test]
fn run_file_emits_progress_notifications_per_step() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir);

    let sink = CapturingSink::new();
    let _ = execute_run_file(
        &RunFileArgs {
            file: path.display().to_string(),
            env: None,
        },
        &sink,
    )
    .expect("run_file ok");

    let notes = sink.notifications();
    assert!(!notes.is_empty(), "expected at least one progress note");
    assert!(notes.iter().all(|n| n.method == PROGRESS_NOTIFICATION));

    // One per step (2) + one finished sentinel = 3.
    let finished_count = notes
        .iter()
        .filter(|n| n.params.get("stage").and_then(|v| v.as_str()) == Some("finished"))
        .count();
    assert_eq!(finished_count, 1);
    let step_count = notes
        .iter()
        .filter(|n| n.params.get("stage").and_then(|v| v.as_str()) == Some("test"))
        .count();
    assert_eq!(step_count, 2, "one notification per step");
}

#[test]
fn run_test_filters_down_to_the_named_test() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir);

    let sink = CapturingSink::new();
    let envelope = execute_run_test(
        &RunTestArgs {
            file: path.display().to_string(),
            test: "alpha".to_string(),
            env: None,
        },
        &sink,
    )
    .expect("run_test ok");

    let tests = envelope.data["files"][0]["tests"].as_array().unwrap();
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0]["name"], "alpha");
}

// NAZ-466 regression: TLS settings (`insecure`, `cacert`) declared in
// `tarn.config.yaml` must reach the runner when tests are launched
// from VS Code via `tarn.runFile`. The LSP path previously used
// `RunOptions::default()` and never consulted the project config, so
// `insecure: true` and custom `cacert` paths were silently ignored.
//
// Proof-by-side-effect: pointing `cacert` at a missing file makes the
// merged transport invalid, and `HttpClient::new` surfaces it as a
// runtime error before any request is sent. Without the merge, the
// LSP would happily run the fixture (which would fail on connection,
// not on cacert).
#[test]
fn run_file_merges_project_cacert_into_runtime() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir);

    let bogus_cacert = dir.path().join("missing-ca.pem");
    fs::write(
        dir.path().join("tarn.config.yaml"),
        format!("test_dir: \".\"\ncacert: \"{}\"\n", bogus_cacert.display()),
    )
    .unwrap();

    let sink = CapturingSink::new();
    let result = execute_run_file(
        &RunFileArgs {
            file: path.display().to_string(),
            env: None,
        },
        &sink,
    );

    let err = result.expect_err("missing cacert from tarn.config.yaml must surface as an error");
    let msg = err.message.to_lowercase();
    assert!(
        msg.contains("cacert") || msg.contains("ca certificate") || msg.contains("read"),
        "error must mention the cacert read failure, got: {}",
        err.message
    );
}

#[test]
fn extract_failures_returns_matching_pairs() {
    let artifact = LastRunArtifact {
        env_name: Some("local".into()),
        files: vec![tarn_lsp::run_commands::LastRunFile {
            file: "tests/a.tarn.yaml".into(),
            tests: vec![
                tarn_lsp::run_commands::LastRunTest {
                    name: "t1".into(),
                    status: "PASSED".into(),
                },
                tarn_lsp::run_commands::LastRunTest {
                    name: "t2".into(),
                    status: "FAILED".into(),
                },
            ],
        }],
    };
    let failures = extract_failures(&artifact);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].test, "t2");
}

#[test]
fn diff_last_passing_returns_structured_diff_for_200_to_500_shift() {
    let source = InMemoryFixtureSource::with(
        json!({
            "status": 500,
            "headers": { "content-type": "application/json" },
            "body": { "error": "boom" }
        }),
        Some(json!({
            "status": 200,
            "headers": { "content-type": "application/json" },
            "body": { "user": { "id": 42, "name": "Ada" } }
        })),
    );

    let out = execute_diff_last_passing(
        &DiffLastPassingArgs {
            file: "/tmp/debug.tarn.yaml".into(),
            test: "alpha".into(),
            step: 0,
        },
        &source,
    )
    .unwrap();

    assert_eq!(out.schema_version, 1);
    let data = &out.data;
    assert_eq!(data["status"]["was"], 200);
    assert_eq!(data["status"]["now"], 500);
    let added = data["body_keys_added"].as_array().unwrap();
    assert!(added.iter().any(|v| v == "$.error"));
    let removed = data["body_keys_removed"].as_array().unwrap();
    assert!(removed.iter().any(|v| v == "$.user"));
}

#[test]
fn diff_last_passing_returns_no_baseline_when_fixture_absent() {
    let source = InMemoryFixtureSource {
        current: Some(json!({"status": 500})),
        last_passing: None,
    };
    let out = execute_diff_last_passing(
        &DiffLastPassingArgs {
            file: "/tmp/debug.tarn.yaml".into(),
            test: "alpha".into(),
            step: 0,
        },
        &source,
    )
    .unwrap();
    assert_eq!(out.data["error"], "no_baseline");
    assert!(out.data["message"]
        .as_str()
        .unwrap()
        .contains("no passing run"));
}
