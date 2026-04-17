//! Integration tests for the per-step fixture writer (NAZ-252).
//!
//! Drives `runner::run_file_with_cookie_jars` against a small in-process
//! server so we can assert the end-to-end shape of the fixture files
//! without mocking the HTTP layer.

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tarn::fixtures::{file_path_hash, slugify_name};
use tarn::model::TestFile;
use tarn::report::fixture_writer::{self, FixtureWriteConfig};
use tarn::runner::{self, RunOptions};
use tempfile::TempDir;

struct AppState {
    counter: Mutex<u32>,
    fail_every: u32,
}

async fn handler_list() -> Json<Value> {
    Json(json!({"items": [{"id": 1, "name": "alpha"}]}))
}

async fn handler_create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> (axum::http::StatusCode, Json<Value>) {
    let mut count = state.counter.lock().unwrap();
    *count += 1;
    if state.fail_every > 0 && *count % state.fail_every == 0 {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "forced failure"})),
        );
    }
    let id = *count as i64;
    (
        axum::http::StatusCode::CREATED,
        Json(json!({"id": id, "echo": body})),
    )
}

/// Bind an in-process server on a free port; return the base URL. The
/// server keeps running for the duration of the test.
fn start_server(fail_every: u32) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener); // axum binds its own

    let state = Arc::new(AppState {
        counter: Mutex::new(0),
        fail_every,
    });

    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let app = Router::new()
                .route("/users", post(handler_create))
                .route("/users", get(handler_list))
                .with_state(state);

            let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port))
                .await
                .unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });

    // Wait briefly for the socket to come back online under axum.
    let health = format!("http://127.0.0.1:{}/users", port);
    for _ in 0..50 {
        if let Ok(client) = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
        {
            if client.get(&health).send().is_ok() {
                return format!("http://127.0.0.1:{}", port);
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("test server never came online on port {}", port);
}

fn mk_options(root: &Path, enabled: bool, retention: usize) -> RunOptions {
    RunOptions {
        fixtures: FixtureWriteConfig {
            enabled,
            workspace_root: root.to_path_buf(),
            retention,
        },
        ..RunOptions::default()
    }
}

fn write_test_file(dir: &Path, base_url: &str) -> (std::path::PathBuf, TestFile) {
    let file_path = dir.join("tests/users.tarn.yaml");
    std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    let yaml = format!(
        r#"
name: Users
env:
  base_url: "{base_url}"
tests:
  happy:
    steps:
      - name: list users
        request:
          method: GET
          url: "{{{{ env.base_url }}}}/users"
        assert:
          status: 200
"#
    );
    std::fs::write(&file_path, &yaml).unwrap();
    let tf: TestFile = serde_yaml::from_str(&yaml).unwrap();
    (file_path, tf)
}

#[test]
fn run_writes_fixture_under_dot_tarn_fixtures() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let base = start_server(0);
    let (file_path, tf) = write_test_file(&root, &base);

    let opts = mk_options(&root, true, 5);
    let env = HashMap::from([("base_url".to_string(), base.clone())]);
    let file_path_str = file_path.to_string_lossy().into_owned();

    let result = runner::run_file(&tf, &file_path_str, &env, &[], &opts).unwrap();
    assert!(result.passed, "run must pass: {:?}", result);

    let hash = file_path_hash(&root, &file_path);
    let step_dir = root
        .join(".tarn/fixtures")
        .join(&hash)
        .join(slugify_name("happy"))
        .join("0");
    assert!(step_dir.is_dir(), "expected {}", step_dir.display());

    let latest = step_dir.join("latest-passed.json");
    assert!(latest.is_file(), "latest-passed.json missing");

    let raw = std::fs::read_to_string(&latest).unwrap();
    let doc: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(doc["passed"], json!(true));
    assert_eq!(doc["request"]["method"], json!("GET"));
    let resp = &doc["response"];
    assert_eq!(resp["status"], json!(200));
    let body = &resp["body"];
    assert!(body.is_object(), "expected JSON body, got {body}");
}

#[test]
fn run_with_disabled_fixtures_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let base = start_server(0);
    let (file_path, tf) = write_test_file(&root, &base);

    let opts = mk_options(&root, false, 5);
    let env = HashMap::from([("base_url".to_string(), base.clone())]);
    let file_path_str = file_path.to_string_lossy().into_owned();
    let result = runner::run_file(&tf, &file_path_str, &env, &[], &opts).unwrap();
    assert!(result.passed);

    let base_dir = root.join(".tarn").join("fixtures");
    assert!(
        !base_dir.exists(),
        "fixtures directory should not exist when --no-fixtures is set"
    );
}

#[test]
fn retention_caps_history_to_configured_value() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let base = start_server(0);
    let (file_path, tf) = write_test_file(&root, &base);

    let opts = mk_options(&root, true, 2);
    let env = HashMap::from([("base_url".to_string(), base.clone())]);
    let file_path_str = file_path.to_string_lossy().into_owned();

    // Run the file five times — each run writes one fixture to the
    // same step directory, exercising the retention pruner.
    for _ in 0..5 {
        // Tiny delay so the millisecond-precision filename doesn't
        // collide across runs on very fast CI machines.
        std::thread::sleep(Duration::from_millis(5));
        let r = runner::run_file(&tf, &file_path_str, &env, &[], &opts).unwrap();
        assert!(r.passed);
    }

    let hash = file_path_hash(&root, &file_path);
    let step_dir = root
        .join(".tarn/fixtures")
        .join(&hash)
        .join(slugify_name("happy"))
        .join("0");
    let entries: Vec<String> = std::fs::read_dir(&step_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "latest-passed.json" && n != "_index.json" && !n.ends_with(".tmp"))
        .collect();
    assert_eq!(entries.len(), 2, "retention=2 cap should yield 2 rolling entries, got {entries:?}");
}

#[test]
fn failing_step_is_readable_via_latest_fixture_reader() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    // Every request fails — exercises the failure-capture path of the
    // writer and the fallback to the newest history entry inside
    // `read_latest_fixture` when `latest-passed.json` is absent.
    let base = start_server(1);

    let yaml = format!(
        r#"
name: Fails
env:
  base_url: "{base}"
tests:
  bad:
    steps:
      - name: create user
        request:
          method: POST
          url: "{{{{ env.base_url }}}}/users"
          body:
            name: leaky-secret
        assert:
          status: 201
"#
    );
    let file_path = root.join("tests/fails.tarn.yaml");
    std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    std::fs::write(&file_path, &yaml).unwrap();
    let tf: TestFile = serde_yaml::from_str(&yaml).unwrap();

    let opts = mk_options(&root, true, 5);
    let env = HashMap::from([("base_url".to_string(), base.clone())]);
    let file_path_str = file_path.to_string_lossy().into_owned();
    let r = runner::run_file(&tf, &file_path_str, &env, &[], &opts).unwrap();
    assert!(!r.passed);

    let fixture = fixture_writer::read_latest_fixture(&root, &file_path, "bad", 0)
        .expect("failed step must still persist a fixture");
    assert!(!fixture.passed);
    assert!(
        fixture
            .failure_message
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains("status"),
        "failure_message should reflect the status assertion: {:?}",
        fixture.failure_message
    );
    assert_eq!(
        fixture.response.as_ref().map(|r| r.status),
        Some(500),
        "fixture must retain the 500 status"
    );
}
