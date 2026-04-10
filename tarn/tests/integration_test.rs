use assert_cmd::Command;
use axum::{routing::get, Json, Router};
use axum_server::tls_rustls::RustlsConfig;
use predicates::prelude::*;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command as StdCommand};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tarn::assert::types::{
    AssertionResult, FailureCategory, FileResult, RequestInfo, ResponseInfo, RunResult, StepResult,
    TestResult,
};
use tarn::model::RedactionConfig;
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

    let mut child = StdCommand::new(&demo_bin)
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
    let _ = child.kill();
    let _ = child.wait();
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
    cert_path: PathBuf,
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
                let config = RustlsConfig::from_pem_file(cert_path_for_thread, key_path_for_thread)
                    .await
                    .unwrap();
                let app = Router::new()
                    .route("/health", get(|| async { Json(json!({ "status": "ok" })) }));

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
                        cert_path,
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

    fn cert_path(&self) -> &std::path::Path {
        &self.cert_path
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

struct ProxyServer {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ProxyServer {
    fn start() -> Self {
        let port = free_port();
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        listener.set_nonblocking(true).unwrap();

        let requests = Arc::new(Mutex::new(Vec::new()));
        let running = Arc::new(AtomicBool::new(true));
        let requests_for_thread = Arc::clone(&requests);
        let running_for_thread = Arc::clone(&running);

        let thread = thread::spawn(move || {
            while running_for_thread.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buffer = Vec::new();
                        let mut chunk = [0_u8; 1024];

                        loop {
                            match stream.read(&mut chunk) {
                                Ok(0) => break,
                                Ok(n) => {
                                    buffer.extend_from_slice(&chunk[..n]);
                                    if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                                        break;
                                    }
                                }
                                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                                    thread::sleep(Duration::from_millis(10));
                                }
                                Err(_) => break,
                            }
                        }

                        requests_for_thread
                            .lock()
                            .unwrap()
                            .push(String::from_utf8_lossy(&buffer).into_owned());

                        let body = r#"{"proxied":true}"#;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            port,
            requests,
            running,
            thread: Some(thread),
        }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn first_request(&self) -> Option<String> {
        self.requests.lock().unwrap().first().cloned()
    }
}

impl Drop for ProxyServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
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

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

fn read_golden(name: &str) -> String {
    fs::read_to_string(golden_dir().join(name))
        .unwrap()
        .trim_end_matches('\n')
        .to_string()
}

fn golden_run_result() -> RunResult {
    let request_headers = HashMap::from([
        (
            "Authorization".to_string(),
            "Bearer env-secret-123".to_string(),
        ),
        ("X-Session".to_string(), "capture-secret-456".to_string()),
    ]);
    let response_headers = HashMap::from([
        (
            "Set-Cookie".to_string(),
            "session=capture-secret-456; HttpOnly".to_string(),
        ),
        ("Content-Type".to_string(), "application/json".to_string()),
    ]);

    RunResult {
        duration_ms: 321,
        file_results: vec![FileResult {
            file: "tests/report-golden.tarn.yaml".into(),
            name: "Reporter Golden".into(),
            passed: false,
            duration_ms: 321,
            redaction: RedactionConfig {
                headers: vec!["authorization".into(), "set-cookie".into()],
                replacement: "[hidden]".into(),
                env_vars: vec!["api_token".into()],
                captures: vec!["session_id".into()],
            },
            redacted_values: vec!["env-secret-123".into(), "capture-secret-456".into()],
            setup_results: vec![StepResult {
                name: "Authenticate".into(),
                passed: true,
                duration_ms: 12,
                assertion_results: vec![AssertionResult::pass("status", "200", "200")],
                request_info: None,
                response_info: None,
                error_category: None,
                response_status: None,
                response_summary: None,
                captures_set: vec![],
            }],
            test_results: vec![TestResult {
                name: "smoke".into(),
                description: Some("Primary flow".into()),
                passed: false,
                duration_ms: 290,
                step_results: vec![
                    StepResult {
                        name: "Create item".into(),
                        passed: true,
                        duration_ms: 34,
                        assertion_results: vec![
                            AssertionResult::pass("status", "201", "201"),
                            AssertionResult::pass("body $.id", "\"it_123\"", "\"it_123\""),
                        ],
                        request_info: None,
                        response_info: None,
                        error_category: None,
                        response_status: None,
                        response_summary: None,
                        captures_set: vec![],
                    },
                    StepResult {
                        name: "Fetch item".into(),
                        passed: false,
                        duration_ms: 56,
                        assertion_results: vec![
                            AssertionResult::pass("status", "200", "200"),
                            AssertionResult::fail_with_diff(
                                "body $",
                                "{\"id\":\"it_123\",\"token\":\"capture-secret-456\"}",
                                "{\"id\":\"it_123\",\"token\":\"wrong-token\"}",
                                "body mismatch: expected env-secret-123 to match capture-secret-456",
                                "--- expected\n+++ actual\n-  \"token\": \"capture-secret-456\"\n+  \"token\": \"wrong-token\"\n",
                            ),
                        ],
                        request_info: Some(RequestInfo {
                            method: "GET".into(),
                            url: "https://api.example.test/items/it_123?token=env-secret-123"
                                .into(),
                            headers: request_headers,
                            body: Some(json!({
                                "trace": "capture-secret-456"
                            })),
                            multipart: None,
                        }),
                        response_info: Some(ResponseInfo {
                            status: 200,
                            headers: response_headers,
                            body: Some(json!({
                                "id": "it_123",
                                "token": "wrong-token",
                                "debug": "env-secret-123"
                            })),
                        }),
                        error_category: Some(FailureCategory::AssertionFailed),
                        response_status: None,
                        response_summary: None,
                        captures_set: vec![],
                    },
                ],
                captures: HashMap::new(),
            }],
            teardown_results: vec![StepResult {
                name: "Cleanup".into(),
                passed: true,
                duration_ms: 8,
                assertion_results: vec![],
                request_info: None,
                response_info: None,
                error_category: None,
                response_status: None,
                response_summary: None,
                captures_set: vec![],
            }],
        }],
    }
}

fn normalize_json_report(output: &str) -> String {
    let mut value: serde_json::Value = serde_json::from_str(output).unwrap();
    value["timestamp"] = json!("<timestamp>");
    serde_json::to_string_pretty(&value).unwrap()
}

fn normalize_html_report(output: &str) -> String {
    let prefix = "const DATA = ";
    let start = output.find(prefix).unwrap() + prefix.len();
    let end = start + output[start..].find(";\n</script>").unwrap();
    let normalized_json = normalize_json_report(&output[start..end]);
    format!("{}{}{}", &output[..start], normalized_json, &output[end..])
}

fn normalize_text_report(output: &str) -> String {
    output.trim_end_matches('\n').to_string()
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
fn status_capture_can_be_reused_in_following_steps() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "status-capture.tarn.yaml",
        &format!(
            r#"
name: Status capture
steps:
  - name: Capture health status
    request:
      method: GET
      url: "{base}/health"
    capture:
      status_code:
        status: true
    assert:
      status: 200

  - name: Reuse status in URL
    request:
      method: GET
      url: "{base}/slow?ms={{{{ capture.status_code }}}}"
    assert:
      status: 200
      body:
        "$.slept_ms": 200
"#,
            base = server.base_url()
        ),
    );

    tarn()
        .args(["run", &test_file])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 passed"));
}

#[test]
fn cookie_and_body_regex_captures_can_be_reused() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "cookie-body-capture.tarn.yaml",
        &format!(
            r#"
name: Cookie and body captures
steps:
  - name: Capture cookies from response
    request:
      method: GET
      url: "{base}/cookies/set"
    capture:
      session_cookie:
        cookie: "session"
      area_cookie:
        header: "set-cookie"
        regex: "area=([^;]+)"
    assert:
      status: 200

  - name: Capture from full text body
    request:
      method: GET
      url: "{base}/plain-text"
    capture:
      body_word:
        body: true
        regex: "plain (text)"
    assert:
      status: 200

  - name: Reuse captured values
    request:
      method: POST
      url: "{base}/form-echo"
      form:
        session: "{{{{ capture.session_cookie }}}}"
        area: "{{{{ capture.area_cookie }}}}"
        word: "{{{{ capture.body_word }}}}"
    assert:
      status: 200
      body:
        "$.fields.session": "abc123"
        "$.fields.area": "dashboard"
        "$.fields.word": "text"
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
fn capture_transforms_can_be_used_in_interpolation() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "capture-transforms.tarn.yaml",
        &format!(
            r#"
name: Capture transforms
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

  - name: Create tagged user
    request:
      method: POST
      url: "{base}/users"
      headers:
        Authorization: "Bearer {{{{ capture.token }}}}"
      body:
        name: "Transform User"
        email: "transform_{{{{ $random_hex(6) }}}}@example.com"
        role: "viewer"
        tags: ["alpha", "beta", "gamma"]
    capture:
      tags: "$.tags"
    assert:
      status: 201

  - name: Reuse transformed captures
    request:
      method: POST
      url: "{base}/form-echo"
      form:
        first_tag: "{{{{ capture.tags | first }}}}"
        last_tag: "{{{{ capture.tags | last }}}}"
        tag_count: "{{{{ capture.tags | count }}}}"
        joined_tags: "{{{{ capture.tags | join('|') }}}}"
    assert:
      status: 200
      body:
        "$.fields.first_tag": "alpha"
        "$.fields.last_tag": "gamma"
        "$.fields.tag_count": "3"
        "$.fields.joined_tags": "alpha|beta|gamma"
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
fn additional_capture_transforms_can_be_chained_in_interpolation() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "capture-transforms-2.tarn.yaml",
        &format!(
            r#"
name: Additional capture transforms
steps:
  - name: Capture plain text body
    request:
      method: GET
      url: "{base}/plain-text"
    capture:
      body_text:
        body: true
    assert:
      status: 200

  - name: Capture numeric text
    request:
      method: GET
      url: "{base}/slow?ms=204"
    capture:
      delay_text:
        jsonpath: "$.slept_ms"
        regex: "(\\d+)"
      status_text:
        status: true
        regex: "(\\d+)"
    assert:
      status: 200

  - name: Reuse additional transforms
    request:
      method: POST
      url: "{base}/form-echo"
      form:
        first_word: "{{{{ capture.body_text | split(' ') | first }}}}"
        word_count: "{{{{ capture.body_text | split(' ') | count }}}}"
        normalized: "{{{{ capture.body_text | replace(' response', '') }}}}"
        delay_ms: "{{{{ capture.delay_text | to_int | to_string }}}}"
        status_code: "{{{{ capture.status_text | to_int | to_string }}}}"
    assert:
      status: 200
      body:
        "$.fields.first_word": "plain"
        "$.fields.word_count": "3"
        "$.fields.normalized": "plain text"
        "$.fields.delay_ms": "204"
        "$.fields.status_code": "200"
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
fn form_requests_are_sent_as_urlencoded_payloads() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "form.tarn.yaml",
        &format!(
            r#"
name: Form request
steps:
  - name: Submit form
    request:
      method: POST
      url: "{}/form-echo"
      form:
        email: "user@example.com"
        redirect: "/dashboard home"
    assert:
      status: 200
      body:
        "$.fields.email": "user@example.com"
        "$.fields.redirect": "/dashboard home"
        "$.content_type": {{ contains: "application/x-www-form-urlencoded" }}
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
fn final_url_capture_tracks_redirect_target() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "redirect-final-url.tarn.yaml",
        &format!(
            r#"
name: Redirect final url
steps:
  - name: Follow redirect and capture final url
    request:
      method: GET
      url: "{}/redirect-health"
    capture:
      final_url:
        url: true
    assert:
      status: 200

  - name: Reuse final url directly
    request:
      method: GET
      url: "{{{{ capture.final_url }}}}"
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
        .stdout(predicate::str::contains("2 passed"));
}

#[test]
fn redirects_can_be_disabled_per_step() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "redirect-disabled.tarn.yaml",
        &format!(
            r#"
name: Redirect disabled
steps:
  - name: Do not follow redirect
    follow_redirects: false
    request:
      method: GET
      url: "{}/redirect-health"
    assert:
      status: 307
"#,
            server.base_url()
        ),
    );

    tarn().args(["run", &test_file]).assert().success();
}

#[test]
fn redirects_respect_max_redirs_limit() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "redirect-max-redirs.tarn.yaml",
        &format!(
            r#"
name: Redirect max redirs
steps:
  - name: Redirect chain stops early
    max_redirs: 1
    request:
      method: GET
      url: "{}/redirect-chain?hops=2"
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

    assert_eq!(output.status.code(), Some(3));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let step = &json["files"][0]["tests"][0]["steps"][0];
    assert_eq!(step["failure_category"], "connection_error");
    assert_eq!(
        step["request"]["url"],
        format!("{}/redirect-chain?hops=2", server.base_url())
    );
    assert!(step["assertions"]["failures"][0]["message"]
        .as_str()
        .unwrap()
        .contains("Too many redirects"));
}

#[test]
fn redirect_assertions_check_final_url_and_count() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "redirect-assertions.tarn.yaml",
        &format!(
            r#"
name: Redirect assertions
steps:
  - name: Follow redirect chain
    request:
      method: GET
      url: "{}/redirect-chain?hops=2"
    assert:
      status: 200
      redirect:
        url: "{}/health"
        count: 3
"#,
            server.base_url(),
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
fn cookies_are_persisted_and_path_scoped() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "cookies-path-scoped.tarn.yaml",
        &format!(
            r#"
name: Cookies path scoping
steps:
  - name: Issue cookies
    request:
      method: GET
      url: "{}/cookies/set"
    assert:
      status: 200
      body:
        "$.issued": true

  - name: Root path only sees root cookie
    request:
      method: GET
      url: "{}/cookies/check"
    assert:
      status: 200
      body:
        "$.session": "abc123"
        "$.area": null

  - name: Nested path sees both cookies
    request:
      method: GET
      url: "{}/cookies/area/check"
    assert:
      status: 200
      body:
        "$.session": "abc123"
        "$.area": "dashboard"
"#,
            server.base_url(),
            server.base_url(),
            server.base_url()
        ),
    );

    tarn()
        .args(["run", &test_file])
        .assert()
        .success()
        .stdout(predicate::str::contains("3 passed"));
}

#[test]
fn cookies_can_be_disabled_per_step() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "cookies-disabled-step.tarn.yaml",
        &format!(
            r#"
name: Cookies disabled per step
steps:
  - name: Issue cookies
    request:
      method: GET
      url: "{}/cookies/set"
    assert:
      status: 200

  - name: Explicitly skip cookies
    cookies: false
    request:
      method: GET
      url: "{}/cookies/check"
    assert:
      status: 200
      body:
        "$.session": null
        "$.area": null
"#,
            server.base_url(),
            server.base_url()
        ),
    );

    tarn()
        .args(["run", &test_file])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 passed"));
}

#[test]
fn timeout_failures_are_reported_in_json() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "timeout-json.tarn.yaml",
        &format!(
            r#"
name: Timeout report
steps:
  - name: Slow endpoint
    timeout: 50
    request:
      method: GET
      url: "{}/slow?ms=200"
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

    assert_eq!(output.status.code(), Some(3));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let step = &json["files"][0]["tests"][0]["steps"][0];
    assert_eq!(step["failure_category"], "timeout");
    assert_eq!(
        step["request"]["url"],
        format!("{}/slow?ms=200", server.base_url())
    );
    assert!(step["assertions"]["failures"][0]["message"]
        .as_str()
        .unwrap()
        .to_ascii_lowercase()
        .contains("timed out"));
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
    let client = tarn::http::HttpClient::new(&tarn::model::HttpTransportConfig::default()).unwrap();
    let error = tarn::http::execute_request(
        &client,
        "GET",
        &format!("{}/health", server.base_url()),
        &HashMap::new(),
        None,
        tarn::http::RequestTransportOptions {
            timeout_ms: Some(1000),
            ..tarn::http::RequestTransportOptions::default()
        },
    )
    .unwrap_err();

    let message = error.to_string().to_ascii_lowercase();
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
fn insecure_flag_allows_self_signed_https() {
    let server = SelfSignedHttpsServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "insecure.tarn.yaml",
        &format!(
            r#"
name: Insecure TLS
steps:
  - name: Self-signed health
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
        .args(["run", &test_file, "--insecure"])
        .assert()
        .success();
}

#[test]
fn cacert_allows_trusting_self_signed_https() {
    let server = SelfSignedHttpsServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "cacert.tarn.yaml",
        &format!(
            r#"
name: Custom CA
steps:
  - name: Trusted self-signed health
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
        .args([
            "run",
            &test_file,
            "--cacert",
            server.cert_path().to_str().unwrap(),
        ])
        .assert()
        .success();
}

#[test]
fn run_uses_explicit_proxy() {
    let proxy = ProxyServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "proxy.tarn.yaml",
        r#"
name: Proxy test
steps:
  - name: Through proxy
    request:
      method: GET
      url: "http://example.invalid/proxy-check"
    assert:
      status: 200
      body:
        "$.proxied": true
"#,
    );

    tarn()
        .args(["run", &test_file, "--proxy", &proxy.base_url()])
        .assert()
        .success();

    assert_eq!(proxy.request_count(), 1);
    let request = proxy.first_request().unwrap();
    assert!(request.contains("GET http://example.invalid/proxy-check HTTP/1.1"));
}

#[test]
fn run_supports_custom_http_methods() {
    let proxy = ProxyServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "custom-method.tarn.yaml",
        r#"
name: Custom method
steps:
  - name: Purge through proxy
    request:
      method: PURGE
      url: "http://example.invalid/cache"
    assert:
      status: 200
      body:
        "$.proxied": true
"#,
    );

    tarn()
        .args(["run", &test_file, "--proxy", &proxy.base_url()])
        .assert()
        .success();

    assert_eq!(proxy.request_count(), 1);
    let request = proxy.first_request().unwrap();
    assert!(request.contains("PURGE http://example.invalid/cache HTTP/1.1"));
}

#[test]
fn no_proxy_bypasses_explicit_proxy() {
    let server = DemoServer::start();
    let proxy = ProxyServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "no-proxy.tarn.yaml",
        &format!(
            r#"
name: No proxy bypass
steps:
  - name: Direct localhost
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
        .args([
            "run",
            &test_file,
            "--proxy",
            &proxy.base_url(),
            "--no-proxy",
            "127.0.0.1",
        ])
        .assert()
        .success();

    assert_eq!(proxy.request_count(), 0);
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
        .args([
            "run",
            "--dry-run",
            "--parallel",
            "--jobs",
            "4",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["status"], "PASSED");
    assert_eq!(json["summary"]["steps"]["total"], 120);
}

#[test]
fn json_report_matches_golden() {
    let actual = normalize_json_report(&tarn::report::json::render(&golden_run_result()));
    let expected = read_golden("report.json.golden");
    assert_eq!(actual, expected);
}

#[test]
fn junit_report_matches_golden() {
    let actual = normalize_text_report(&tarn::report::junit::render(&golden_run_result()));
    let expected = read_golden("report.junit.golden");
    assert_eq!(actual, expected);
}

#[test]
fn tap_report_matches_golden() {
    let actual = normalize_text_report(&tarn::report::tap::render(&golden_run_result()));
    let expected = read_golden("report.tap.golden");
    assert_eq!(actual, expected);
}

#[test]
fn html_report_matches_golden() {
    let actual = normalize_html_report(&tarn::report::html::render(&golden_run_result()));
    let expected = read_golden("report.html.golden");
    assert_eq!(actual, expected);
}

#[test]
fn only_failed_hides_passing_tests_in_human_output() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "mixed.tarn.yaml",
        &format!(
            r#"
name: Mixed suite
tests:
  happy_path:
    steps:
      - name: healthy request
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
  broken_path:
    steps:
      - name: wrong status expected
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 418
"#,
            base = server.base_url()
        ),
    );

    let output = tarn()
        .args(["run", &test_file, "--only-failed", "--no-progress"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("healthy request"),
        "passing step should be hidden: {}",
        stdout
    );
    assert!(
        stdout.contains("wrong status expected"),
        "failing step should be visible: {}",
        stdout
    );
    assert!(
        stdout.contains("1 passed"),
        "summary should still report totals: {}",
        stdout
    );
    assert!(
        stdout.contains("1 failed"),
        "summary should still report totals: {}",
        stdout
    );
}

#[test]
fn only_failed_prunes_passing_entries_from_json_output() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "mixed_json.tarn.yaml",
        &format!(
            r#"
name: Mixed JSON suite
tests:
  ok:
    steps:
      - name: healthy
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
  nope:
    steps:
      - name: wrong status
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 500
"#,
            base = server.base_url()
        ),
    );

    let output = tarn()
        .args([
            "run",
            &test_file,
            "--format",
            "json",
            "--only-failed",
            "--no-progress",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let files = parsed["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    let tests = files[0]["tests"].as_array().unwrap();
    assert_eq!(tests.len(), 1, "only the failing test should remain");
    assert_eq!(tests[0]["name"], "nope");
    let steps = tests[0]["steps"].as_array().unwrap();
    assert!(
        steps.iter().all(|s| s["status"] == "FAILED"),
        "expected only failed steps, got {:?}",
        steps
    );

    let summary = &parsed["summary"]["steps"];
    assert_eq!(summary["passed"], 1);
    assert_eq!(summary["failed"], 1);
}

#[test]
fn streaming_progress_emits_to_stderr_when_json_is_on_stdout() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "stream.tarn.yaml",
        &format!(
            r#"
name: Stream suite
tests:
  first:
    steps:
      - name: stream step one
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
"#,
            base = server.base_url()
        ),
    );

    let output = tarn()
        .args(["run", &test_file, "--format", "json"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("stream step one"),
        "progress should stream on stderr, got stderr={}",
        stderr
    );
    // stdout should remain parseable JSON (streaming must not pollute it).
    let _: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("stdout should still be pure JSON when streaming to stderr");
}

#[test]
fn no_progress_suppresses_stderr_streaming() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "nostream.tarn.yaml",
        &format!(
            r#"
name: Quiet suite
tests:
  first:
    steps:
      - name: silent step
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
"#,
            base = server.base_url()
        ),
    );

    let output = tarn()
        .args(["run", &test_file, "--format", "json", "--no-progress"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("silent step"),
        "with --no-progress stderr should stay quiet, got: {}",
        stderr
    );
}

#[test]
#[ignore]
fn dump_report_goldens() {
    println!("=== report.json.golden ===");
    println!(
        "{}",
        normalize_json_report(&tarn::report::json::render(&golden_run_result()))
    );
    println!("=== report.junit.golden ===");
    println!("{}", tarn::report::junit::render(&golden_run_result()));
    println!("=== report.tap.golden ===");
    println!("{}", tarn::report::tap::render(&golden_run_result()));
    println!("=== report.html.golden ===");
    println!(
        "{}",
        normalize_html_report(&tarn::report::html::render(&golden_run_result()))
    );
}
