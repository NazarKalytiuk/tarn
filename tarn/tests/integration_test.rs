use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;
use std::net::TcpListener;
use std::process::{Child, Command as StdCommand};
use std::time::Duration;
use tempfile::TempDir;

/// Find a free port on localhost.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Start the demo server on a given port and return the child process.
/// Waits until the server is ready.
fn start_demo_server(port: u16) -> Child {
    // The demo-server binary is built alongside tarn
    let demo_bin = std::path::Path::new(env!("CARGO_BIN_EXE_tarn"))
        .parent()
        .unwrap()
        .join("demo-server");

    let child = StdCommand::new(&demo_bin)
        .env("PORT", port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Failed to start demo-server");

    // Wait for server to be ready
    for _ in 0..50 {
        if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            return child;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("Demo server failed to start on port {}", port);
}

struct DemoServer {
    child: Child,
    port: u16,
}

impl DemoServer {
    fn start() -> Self {
        let port = free_port();
        let child = start_demo_server(port);
        Self { child, port }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for DemoServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_test_file(dir: &TempDir, name: &str, content: &str) -> String {
    let path = dir.path().join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
    path.display().to_string()
}

fn tarn() -> Command {
    Command::cargo_bin("tarn").unwrap()
}

// ============================================================
// Tests
// ============================================================

#[test]
fn health_check_passes() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "health.tarn.yaml",
        &format!(
            r#"
name: Health check
steps:
  - name: GET /health
    request:
      method: GET
      url: "{}/health"
    assert:
      status: 200
      body:
        "$.status": "ok"
"#,
            server.base_url()
        ),
    );

    tarn()
        .args(["run", &test_file])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 passed"));
}

#[test]
fn failing_assertion_exits_with_1() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "fail.tarn.yaml",
        &format!(
            r#"
name: Should fail
steps:
  - name: Wrong status
    request:
      method: GET
      url: "{}/health"
    assert:
      status: 404
"#,
            server.base_url()
        ),
    );

    tarn()
        .args(["run", &test_file])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("1 failed"));
}

#[test]
fn json_output_is_valid() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "json.tarn.yaml",
        &format!(
            r#"
name: JSON output test
steps:
  - name: health
    request:
      method: GET
      url: "{}/health"
    assert:
      status: 200
"#,
            server.base_url()
        ),
    );

    let output = tarn()
        .args(["run", &test_file, "--format", "json"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["status"], "PASSED");
    assert_eq!(json["summary"]["steps"]["total"], 1);
}

#[test]
fn capture_and_chaining_works() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "chain.tarn.yaml",
        &format!(
            r#"
name: Capture chain
defaults:
  headers:
    Content-Type: "application/json"
steps:
  - name: Login
    request:
      method: POST
      url: "{base}/auth/login"
      body:
        email: "admin@example.com"
        password: "password123"
    capture:
      token: "$.token"
    assert:
      status: 200

  - name: Create user
    request:
      method: POST
      url: "{base}/users"
      headers:
        Authorization: "Bearer {{{{ capture.token }}}}"
      body:
        name: "Test User"
        email: "test_{{{{ $random_hex(6) }}}}@example.com"
        role: "viewer"
    capture:
      user_id: "$.id"
    assert:
      status: 201

  - name: Get user
    request:
      method: GET
      url: "{base}/users/{{{{ capture.user_id }}}}"
      headers:
        Authorization: "Bearer {{{{ capture.token }}}}"
    assert:
      status: 200
      body:
        "$.name": "Test User"
"#,
            base = server.base_url()
        ),
    );

    tarn()
        .args(["run", &test_file])
        .assert()
        .success()
        .stdout(predicate::str::contains("3 passed"));
}

#[test]
fn validate_command_checks_yaml() {
    let dir = TempDir::new().unwrap();

    let good_file = write_test_file(
        &dir,
        "good.tarn.yaml",
        r#"
name: Valid
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
"#,
    );

    tarn().args(["validate", &good_file]).assert().success();

    let bad_file = write_test_file(&dir, "bad.tarn.yaml", "not valid yaml: [");

    tarn().args(["validate", &bad_file]).assert().code(2);
}

#[test]
fn dry_run_does_not_send_requests() {
    // No server running — dry run should not fail
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "dryrun.tarn.yaml",
        r#"
name: Dry run
steps:
  - name: This would fail without dry-run
    request:
      method: GET
      url: "http://127.0.0.1:1/this-port-is-not-open"
    assert:
      status: 200
"#,
    );

    tarn()
        .args(["run", &test_file, "--dry-run"])
        .assert()
        .success();
}

#[test]
fn tag_filter_skips_unmatched_tests() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "tags.tarn.yaml",
        &format!(
            r#"
name: Tag test
tests:
  smoke_test:
    tags: [smoke]
    steps:
      - name: Smoke
        request:
          method: GET
          url: "{}/health"
        assert:
          status: 200
  slow_test:
    tags: [slow]
    steps:
      - name: Slow
        request:
          method: GET
          url: "{}/health"
        assert:
          status: 200
"#,
            server.base_url(),
            server.base_url()
        ),
    );

    // Run only smoke tests
    let output = tarn()
        .args(["run", &test_file, "--tag", "smoke", "--format", "json"])
        .output()
        .unwrap();

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // Only 1 test should run (smoke), not 2
    assert_eq!(json["summary"]["steps"]["total"], 1);
}

#[test]
fn junit_output_is_valid_xml() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "junit.tarn.yaml",
        &format!(
            r#"
name: JUnit test
steps:
  - name: health
    request:
      method: GET
      url: "{}/health"
    assert:
      status: 200
"#,
            server.base_url()
        ),
    );

    tarn()
        .args(["run", &test_file, "--format", "junit"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("<?xml version=\"1.0\""));
}

#[test]
fn tap_output_format() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "tap.tarn.yaml",
        &format!(
            r#"
name: TAP test
steps:
  - name: health
    request:
      method: GET
      url: "{}/health"
    assert:
      status: 200
"#,
            server.base_url()
        ),
    );

    tarn()
        .args(["run", &test_file, "--format", "tap"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("TAP version 13"));
}

#[test]
fn unauthorized_without_token() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "unauth.tarn.yaml",
        &format!(
            r#"
name: Auth test
steps:
  - name: No token
    request:
      method: GET
      url: "{}/users"
    assert:
      status: 401
      body:
        "$.error": "unauthorized"
"#,
            server.base_url()
        ),
    );

    tarn()
        .args(["run", &test_file])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 passed"));
}

#[test]
fn setup_and_teardown_lifecycle() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "lifecycle.tarn.yaml",
        &format!(
            r#"
name: Lifecycle test
defaults:
  headers:
    Content-Type: "application/json"
setup:
  - name: Login
    request:
      method: POST
      url: "{base}/auth/login"
      body:
        email: "admin@example.com"
        password: "password123"
    capture:
      token: "$.token"
    assert:
      status: 200
teardown:
  - name: Cleanup
    request:
      method: POST
      url: "{base}/test/cleanup"
tests:
  create_user:
    steps:
      - name: Create
        request:
          method: POST
          url: "{base}/users"
          headers:
            Authorization: "Bearer {{{{ capture.token }}}}"
          body:
            name: "Test"
            email: "t@t.com"
        assert:
          status: 201
"#,
            base = server.base_url()
        ),
    );

    tarn()
        .args(["run", &test_file])
        .assert()
        .success()
        .stdout(predicate::str::contains("Setup"))
        .stdout(predicate::str::contains("Teardown"));
}

#[test]
fn missing_file_exits_with_error() {
    tarn()
        .args(["run", "/nonexistent/file.tarn.yaml"])
        .assert()
        .code(2);
}

#[test]
fn completions_generates_output() {
    tarn()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tarn"));
}
