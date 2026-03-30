use assert_cmd::Command;
use axum::{routing::get, Json, Router};
use axum_server::tls_rustls::RustlsConfig;
use predicates::prelude::*;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command as StdCommand};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// Find a free port on localhost.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Start the demo server on a given port and return the child process.
/// Waits until the server is ready to handle HTTP requests (not just TCP).
fn start_demo_server(port: u16) -> Child {
    static BUILD_DEMO_SERVER: OnceLock<()> = OnceLock::new();
    BUILD_DEMO_SERVER.get_or_init(|| {
        let status = StdCommand::new("cargo")
            .args(["build", "-q", "-p", "demo-server"])
            .status()
            .expect("Failed to build demo-server");
        assert!(status.success(), "demo-server build failed");
    });

    let demo_bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
        .join(format!("demo-server{}", std::env::consts::EXE_SUFFIX));

    let child = StdCommand::new(&demo_bin)
        .env("PORT", port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Failed to start demo-server");

    // Wait for server to be fully ready by hitting /health
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let health_url = format!("http://127.0.0.1:{}/health", port);
    for _ in 0..50 {
        if let Ok(resp) = client.get(&health_url).send() {
            if resp.status().is_success() {
                return child;
            }
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

struct SelfSignedHttpsServer {
    _cert_dir: TempDir,
    handle: axum_server::Handle,
    thread: Option<thread::JoinHandle<()>>,
    port: u16,
}

impl SelfSignedHttpsServer {
    fn start() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let cert_dir = TempDir::new().unwrap();
        let cert_path = cert_dir.path().join("cert.pem");
        let key_path = cert_dir.path().join("key.pem");

        let cert = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
        ])
        .unwrap();
        fs::write(&cert_path, cert.cert.pem()).unwrap();
        fs::write(&key_path, cert.key_pair.serialize_pem()).unwrap();

        let port = free_port();
        let handle = axum_server::Handle::new();
        let handle_clone = handle.clone();
        let cert_path_for_thread = cert_path.clone();
        let key_path_for_thread = key_path.clone();

        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async move {
                let config =
                    RustlsConfig::from_pem_file(cert_path_for_thread, key_path_for_thread)
                        .await
                        .unwrap();
                let app = Router::new().route(
                    "/health",
                    get(|| async { Json(json!({ "status": "ok" })) }),
                );

                axum_server::bind_rustls(([127, 0, 0, 1], port).into(), config)
                    .handle(handle_clone)
                    .serve(app.into_make_service())
                    .await
                    .unwrap();
            });
        });

        let client = reqwest::blocking::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        let health_url = format!("https://127.0.0.1:{}/health", port);
        for _ in 0..50 {
            if let Ok(resp) = client.get(&health_url).send() {
                if resp.status().is_success() {
                    return Self {
                        _cert_dir: cert_dir,
                        handle,
                        thread: Some(thread),
                        port,
                    };
                }
            }
            thread::sleep(Duration::from_millis(100));
        }

        panic!("HTTPS test server failed to start on port {}", port);
    }

    fn base_url(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
    }
}

impl Drop for SelfSignedHttpsServer {
    fn drop(&mut self) {
        self.handle.graceful_shutdown(Some(Duration::from_secs(1)));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
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

fn write_nested_test_file(root: &std::path::Path, relative: &str, content: &str) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(content.as_bytes()).unwrap();
    path
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
fn json_output_reports_runtime_failures() {
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "runtime-json.tarn.yaml",
        r#"
name: Runtime JSON output test
steps:
  - name: connect failure
    request:
      method: GET
      url: "http://127.0.0.1:1/not-running"
    assert:
      status: 200
"#,
    );

    let output = tarn()
        .args(["run", &test_file, "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let step = &json["files"][0]["tests"][0]["steps"][0];
    assert_eq!(step["failure_category"], "connection_error");
    assert_eq!(step["request"]["url"], "http://127.0.0.1:1/not-running");
    assert!(step.get("response").is_none());
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

#[test]
fn plain_text_response_asserts_on_root_value() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "plain-text.tarn.yaml",
        &format!(
            r#"
name: Plain text
steps:
  - name: Plain text root body
    request:
      method: GET
      url: "{}/plain-text"
    assert:
      status: 200
      body:
        "$": "plain text response"
"#,
            server.base_url()
        ),
    );

    tarn().args(["run", &test_file]).assert().success();
}

#[test]
fn empty_response_can_assert_null_body() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "empty.tarn.yaml",
        &format!(
            r#"
name: Empty response
steps:
  - name: Empty body
    request:
      method: GET
      url: "{}/empty"
    assert:
      status: 204
      body:
        "$": null
"#,
            server.base_url()
        ),
    );

    tarn().args(["run", &test_file]).assert().success();
}

#[test]
fn redirects_are_followed_automatically() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "redirect.tarn.yaml",
        &format!(
            r#"
name: Redirect
steps:
  - name: Follow redirect
    request:
      method: GET
      url: "{}/redirect-health"
    assert:
      status: 200
      body:
        "$.status": "ok"
"#,
            server.base_url()
        ),
    );

    tarn().args(["run", &test_file]).assert().success();
}

#[test]
fn unicode_json_bodies_are_assertable() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "unicode.tarn.yaml",
        &format!(
            r#"
name: Unicode
steps:
  - name: Unicode body
    request:
      method: GET
      url: "{}/unicode"
    assert:
      status: 200
      body:
        "$.message": "Привіт, Tarn 👋"
        "$.emoji": "🌍"
"#,
            server.base_url()
        ),
    );

    tarn().args(["run", &test_file]).assert().success();
}

#[test]
fn large_json_responses_can_be_asserted() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "large.tarn.yaml",
        &format!(
            r#"
name: Large body
steps:
  - name: Large response
    request:
      method: GET
      url: "{}/large"
    assert:
      status: 200
      body:
        "$.size": 1048576
        "$.blob": {{ type: string, length: 1048576 }}
"#,
            server.base_url()
        ),
    );

    tarn().args(["run", &test_file]).assert().success();
}

#[test]
fn invalid_ssl_certificate_returns_actionable_error() {
    let server = SelfSignedHttpsServer::start();
    let error = tarn::http::execute_request(
        "GET",
        &format!("{}/health", server.base_url()),
        &HashMap::new(),
        None,
        Some(1000),
    )
    .unwrap_err();

    let message = error
        .to_string()
        .to_ascii_lowercase();
    assert!(
        message.contains("certificate")
            || message.contains("tls")
            || message.contains("ssl")
            || message.contains("unknown issuer"),
        "expected TLS-related message, got: {}",
        message
    );
}

#[test]
fn large_suites_can_run_in_parallel_dry_run_mode() {
    let dir = TempDir::new().unwrap();
    let tests_dir = dir.path().join("tests");

    for i in 0..120 {
        write_nested_test_file(
            &tests_dir,
            &format!("suite/test_{i:03}.tarn.yaml"),
            &format!(
                r#"
name: Dry run {i}
steps:
  - name: Dry run {i}
    request:
      method: GET
      url: "http://127.0.0.1:1/dry-run-{i}"
    assert:
      status: 200
"#
            ),
        );
    }

    let output = tarn()
        .current_dir(dir.path())
        .args(["run", "--dry-run", "--parallel", "--jobs", "4", "--format", "json"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["status"], "PASSED");
    assert_eq!(json["summary"]["steps"]["total"], 120);
}
