//! One-shot demo test that prints the `tarn.explainFailure` envelope
//! for a 500-response fixture. Run with:
//!
//!   cargo test -p tarn-lsp --test example_explain -- --ignored --nocapture

use serde_json::json;
use tarn::report::fixture_writer::{self, Fixture, FixtureRequest, FixtureResponse, FixtureWriteConfig};
use tarn_lsp::explain_failure::{workspace_explain_failure, EXPLAIN_FAILURE_COMMAND};
use tarn_lsp::server::ServerState;
use tempfile::TempDir;

#[test]
#[ignore]
fn example_explain() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let file = root.join("tests/users.tarn.yaml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        r#"
name: Users
tests:
  happy:
    steps:
      - name: login
        request:
          method: POST
          url: http://127.0.0.1:9000/login
        capture:
          token:
            jsonpath: $.token
      - name: list
        request:
          method: GET
          url: http://127.0.0.1:9000/users
          headers:
            Authorization: "Bearer {{ capture.token }}"
        assert:
          status: 200
"#,
    )
    .unwrap();

    let login_fx = Fixture {
        recorded_at: "2026-04-17T18:55:39Z".into(),
        request: FixtureRequest {
            method: "POST".into(),
            url: "http://127.0.0.1:9000/login".into(),
            headers: Default::default(),
            body: None,
        },
        response: Some(FixtureResponse {
            status: 200,
            headers: Default::default(),
            body: Some(json!({"token": "abc123"})),
        }),
        captures: {
            let mut m = serde_json::Map::new();
            m.insert("token".into(), json!("abc123"));
            m
        },
        passed: true,
        failure_message: None,
        duration_ms: 12,
    };

    let failure_fx = Fixture {
        recorded_at: "2026-04-17T18:55:40Z".into(),
        request: FixtureRequest {
            method: "GET".into(),
            url: "http://127.0.0.1:9000/users".into(),
            headers: [("Authorization".to_string(), "Bearer abc123".to_string())]
                .iter()
                .cloned()
                .collect(),
            body: None,
        },
        response: Some(FixtureResponse {
            status: 500,
            headers: [("content-type".to_string(), "application/json".to_string())]
                .iter()
                .cloned()
                .collect(),
            body: Some(json!({"error": "database unavailable"})),
        }),
        captures: Default::default(),
        passed: false,
        failure_message: Some("status: expected 200, got 500".into()),
        duration_ms: 9,
    };

    let cfg = FixtureWriteConfig {
        enabled: true,
        workspace_root: root.clone(),
        retention: 5,
    };
    fixture_writer::write_step_fixture(&cfg, &file, "happy", 0, &login_fx).unwrap();
    fixture_writer::write_step_fixture(&cfg, &file, "happy", 1, &failure_fx).unwrap();

    let params = lsp_types::ExecuteCommandParams {
        command: EXPLAIN_FAILURE_COMMAND.into(),
        arguments: vec![json!({
            "file": file.to_string_lossy(),
            "test": "happy",
            "step": "list",
        })],
        work_done_progress_params: Default::default(),
    };
    let state = ServerState::new();
    let resp = workspace_explain_failure(&state, params).expect("ok");
    let pretty = serde_json::to_string_pretty(&resp).unwrap();
    eprintln!("=== example tarn.explainFailure output ===\n{pretty}");
}
