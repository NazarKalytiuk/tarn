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
///
/// Racy: the returned port is not held open, so a parallel test (or the
/// kernel's ephemeral pool) may grab it before the caller rebinds. Only use
/// this when the caller hands the port to a subprocess or external server that
/// cannot accept a pre-bound `TcpListener`. In-process listeners should use
/// `bind_ephemeral_listener` instead.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Bind a localhost listener on an OS-chosen port without a drop gap.
///
/// Returns the bound listener and its port. Use this whenever the caller will
/// drive the listener in-process — it closes the TOCTOU race `free_port` has
/// against parallel tests and ephemeral outbound sockets.
fn bind_ephemeral_listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
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
        let (listener, port) = bind_ephemeral_listener();
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

/// Accepts either the legacy human summary (`"N passed"`) or the new
/// llm summary (`"PASS N/M steps"`) — `tarn run` now auto-selects llm
/// when stdout is piped (assert_cmd captures stdout, so every test
/// subcommand runs without a TTY). Tests that care about a specific
/// format still pass `--format human` explicitly.
fn passed_summary_predicate(count: usize) -> predicates::BoxPredicate<str> {
    let human = format!("{} passed", count);
    let llm = format!("PASS {}/", count);
    predicates::BoxPredicate::new(predicate::str::contains(human).or(predicate::str::contains(llm)))
}

/// Companion of [`passed_summary_predicate`] for the `FAILED` summary
/// line. Accepts either the legacy human trailer (`"N failed"`) or the
/// grep-friendly llm line (`"tarn: FAIL ..., N failed"`).
fn failed_summary_predicate(count: usize) -> predicates::BoxPredicate<str> {
    let human = format!("{} failed", count);
    let llm = format!(", {} failed", count);
    predicates::BoxPredicate::new(predicate::str::contains(human).or(predicate::str::contains(llm)))
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
                description: None,
                debug: false,
                passed: true,
                duration_ms: 12,
                assertion_results: vec![AssertionResult::pass("status", "200", "200")],
                request_info: None,
                response_info: None,
                error_category: None,
                response_status: None,
                response_summary: None,
                captures_set: vec![],
                location: None,
            }],
            test_results: vec![TestResult {
                name: "smoke".into(),
                description: Some("Primary flow".into()),
                passed: false,
                duration_ms: 290,
                step_results: vec![
                    StepResult {
                        name: "Create item".into(),
                        description: None,
                        debug: false,
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
                        location: None,
                    },
                    StepResult {
                        name: "Fetch item".into(),
                        description: None,
                        debug: false,
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
                        location: None,
                    },
                ],
                captures: HashMap::new(),
            }],
            teardown_results: vec![StepResult {
                name: "Cleanup".into(),
                description: None,
                debug: false,
                passed: true,
                duration_ms: 8,
                assertion_results: vec![],
                request_info: None,
                response_info: None,
                error_category: None,
                response_status: None,
                response_summary: None,
                captures_set: vec![],
                location: None,
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
        .stdout(passed_summary_predicate(1));
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
        .stdout(failed_summary_predicate(1));
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
fn tarn_faker_seed_makes_interpolation_byte_identical_across_processes() {
    // Two invocations of the tarn binary with the same TARN_FAKER_SEED
    // should produce the exact same interpolated URLs. The test
    // deliberately uses a dead-letter host so the step fails fast —
    // the JSON report then exposes `request.url` for the failed step,
    // which is where we read the interpolated faker value back out.
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "faker-seed.tarn.yaml",
        r#"
name: Faker seed determinism
steps:
  - name: faker interpolated URL
    request:
      method: GET
      url: "http://127.0.0.1:1/{{ $email }}/{{ $uuid_v4 }}/{{ $random_int(1, 1000000) }}/{{ $slug }}/{{ $choice(red, green, blue) }}"
    assert:
      status: 200
"#,
    );

    let capture_url = |seed: &str| -> String {
        let output = tarn()
            .args(["run", &test_file, "--format", "json"])
            .env("TARN_FAKER_SEED", seed)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(3));
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        json["files"][0]["tests"][0]["steps"][0]["request"]["url"]
            .as_str()
            .expect("failed step should carry request.url")
            .to_owned()
    };

    let run_a = capture_url("42");
    let run_b = capture_url("42");
    assert_eq!(
        run_a, run_b,
        "same seed must produce byte-identical interpolated URLs"
    );

    let run_c = capture_url("999");
    assert_ne!(
        run_a, run_c,
        "different seeds should normally produce different URLs"
    );
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
        .stdout(passed_summary_predicate(3));
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
        .stdout(passed_summary_predicate(2));
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
        .stdout(passed_summary_predicate(3));
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
        .stdout(passed_summary_predicate(3));
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
        .stdout(passed_summary_predicate(3));
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
        .stdout(passed_summary_predicate(1));
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
        .args(["run", &test_file, "--format", "human"])
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
        .stdout(passed_summary_predicate(2));
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
        .stdout(passed_summary_predicate(1));
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
        .stdout(passed_summary_predicate(3));
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
        .stdout(passed_summary_predicate(2));
}

/// Build a named-test file that (1) sets a session cookie in test A and
/// (2) asserts in test B that the session cookie is *absent*. The
/// assertion only holds when the default cookie jar is reset between
/// named tests.
fn per_test_cookies_fixture(base_url: &str, mode_line: &str) -> String {
    format!(
        r#"
name: Per-test cookie isolation
{mode_line}
tests:
  login:
    steps:
      - name: Issue session cookie
        request:
          method: GET
          url: "{base}/cookies/set"
        assert:
          status: 200
          body:
            "$.issued": true
      - name: Same test still sees the cookie
        request:
          method: GET
          url: "{base}/cookies/check"
        assert:
          status: 200
          body:
            "$.session": "abc123"
  isolated:
    steps:
      - name: Second test must not see the login session
        request:
          method: GET
          url: "{base}/cookies/check"
        assert:
          status: 200
          body:
            "$.session": null
            "$.area": null
"#,
        base = base_url,
        mode_line = mode_line,
    )
}

#[test]
fn cookies_leak_between_named_tests_by_default() {
    // Baseline: without per-test mode the session cookie from `login`
    // leaks into `isolated` and the isolation assertion fails. This
    // proves the new mode is doing real work.
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "cookies-default-leaks.tarn.yaml",
        &per_test_cookies_fixture(&server.base_url(), ""),
    );

    tarn().args(["run", &test_file]).assert().failure().stdout(
        passed_summary_predicate(1)
            .or(predicate::str::contains("failed"))
            .or(predicate::str::contains("FAIL ")),
    );
}

#[test]
fn cookies_per_test_mode_isolates_named_tests() {
    // `cookies: per-test` in the file clears the default jar between
    // named tests, so the session set in `login` never reaches
    // `isolated`. Both tests pass.
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "cookies-per-test.tarn.yaml",
        &per_test_cookies_fixture(&server.base_url(), "cookies: \"per-test\""),
    );

    tarn()
        .args(["run", &test_file])
        .assert()
        .success()
        .stdout(passed_summary_predicate(3));
}

#[test]
fn cookie_jar_per_test_cli_flag_overrides_file_default() {
    // The `--cookie-jar-per-test` CLI flag must override a file that
    // does not declare per-test mode.
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "cookies-cli-per-test.tarn.yaml",
        &per_test_cookies_fixture(&server.base_url(), ""),
    );

    tarn()
        .args(["run", &test_file, "--cookie-jar-per-test"])
        .assert()
        .success()
        .stdout(passed_summary_predicate(3));
}

#[test]
fn cookies_off_wins_over_cookie_jar_per_test_cli_flag() {
    // `cookies: "off"` is a hard disable and must not be silently
    // re-enabled by --cookie-jar-per-test. With cookies off, the
    // session is never captured, so the isolated test naturally sees
    // no session cookie — the whole file passes.
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "cookies-off-vs-cli.tarn.yaml",
        &format!(
            r#"
name: Cookies off wins
cookies: "off"
tests:
  login:
    steps:
      - name: Issue session cookie
        request:
          method: GET
          url: "{}/cookies/set"
        assert:
          status: 200
      - name: Cookies are off so nothing is seen
        request:
          method: GET
          url: "{}/cookies/check"
        assert:
          status: 200
          body:
            "$.session": null
  isolated:
    steps:
      - name: Second test also sees nothing
        request:
          method: GET
          url: "{}/cookies/check"
        assert:
          status: 200
          body:
            "$.session": null
"#,
            server.base_url(),
            server.base_url(),
            server.base_url()
        ),
    );

    tarn()
        .args(["run", &test_file, "--cookie-jar-per-test"])
        .assert()
        .success()
        .stdout(passed_summary_predicate(3));
}

#[test]
fn cookies_per_test_rejects_invalid_mode() {
    // An unknown cookies mode must be a parse-time validation error, not
    // a silent fallback to auto. Guards future additions to the enum.
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "cookies-invalid.tarn.yaml",
        r#"
name: Bad cookies value
cookies: "sometimes"
steps:
  - name: noop
    request:
      method: GET
      url: "http://127.0.0.1:1/"
"#,
    );

    tarn()
        .args(["run", &test_file])
        .assert()
        .failure()
        .stderr(predicate::str::contains("per-test"));
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
        .args([
            "run",
            &test_file,
            "--format",
            "human",
            "--only-failed",
            "--no-progress",
        ])
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

// ============================================================================
// T51: --select FILE[::TEST[::STEP]]
// ============================================================================

fn select_fixture_file(dir: &TempDir, base_url: &str) -> String {
    write_test_file(
        dir,
        "select.tarn.yaml",
        &format!(
            r#"
name: Select fixture
tests:
  login:
    tags: [auth]
    steps:
      - name: step one
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
      - name: step two
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
  logout:
    tags: [auth]
    steps:
      - name: bye
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
  admin:
    tags: [admin]
    steps:
      - name: admin only
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
"#,
            base = base_url
        ),
    )
}

#[test]
fn select_test_runs_only_the_selected_test() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = select_fixture_file(&dir, &server.base_url());

    let output = tarn()
        .args([
            "run",
            &file,
            "--select",
            "select.tarn.yaml::login",
            "--format",
            "json",
            "--no-progress",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let tests = parsed["files"][0]["tests"].as_array().unwrap();
    assert_eq!(tests.len(), 1, "only login should run: {:?}", tests);
    assert_eq!(tests[0]["name"], "login");
    let steps = tests[0]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2, "login has two steps");
}

#[test]
fn select_step_runs_only_the_selected_step() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = select_fixture_file(&dir, &server.base_url());

    let output = tarn()
        .args([
            "run",
            &file,
            "--select",
            "select.tarn.yaml::login::step two",
            "--format",
            "json",
            "--no-progress",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let tests = parsed["files"][0]["tests"].as_array().unwrap();
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0]["name"], "login");
    let steps = tests[0]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1, "only step two should run: {:?}", steps);
    assert_eq!(steps[0]["name"], "step two");
}

#[test]
fn select_step_by_numeric_index() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = select_fixture_file(&dir, &server.base_url());

    let output = tarn()
        .args([
            "run",
            &file,
            "--select",
            "select.tarn.yaml::login::0",
            "--format",
            "json",
            "--no-progress",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let steps = parsed["files"][0]["tests"][0]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["name"], "step one");
}

#[test]
fn multiple_selectors_union() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = select_fixture_file(&dir, &server.base_url());

    let output = tarn()
        .args([
            "run",
            &file,
            "--select",
            "select.tarn.yaml::login",
            "--select",
            "select.tarn.yaml::admin",
            "--format",
            "json",
            "--no-progress",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let tests = parsed["files"][0]["tests"].as_array().unwrap();
    let names: Vec<&str> = tests.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["login", "admin"]);
}

#[test]
fn select_and_tag_filter_and_together() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = select_fixture_file(&dir, &server.base_url());

    // --select picks login and admin, --tag auth keeps only login
    let output = tarn()
        .args([
            "run",
            &file,
            "--select",
            "select.tarn.yaml::login",
            "--select",
            "select.tarn.yaml::admin",
            "--tag",
            "auth",
            "--format",
            "json",
            "--no-progress",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let tests = parsed["files"][0]["tests"].as_array().unwrap();
    let names: Vec<&str> = tests.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["login"], "auth tag should keep only login");
}

#[test]
fn malformed_selector_exits_with_code_two() {
    let dir = TempDir::new().unwrap();
    // Any .tarn.yaml will do since we expect to fail before running.
    let file = write_test_file(
        &dir,
        "x.tarn.yaml",
        r#"
name: stub
tests:
  t:
    steps:
      - name: s
        request:
          method: GET
          url: "http://127.0.0.1:0/"
"#,
    );

    let output = tarn()
        .args(["run", &file, "--select", "::broken", "--no-progress"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid --select"),
        "expected parse error, got: {}",
        stderr
    );
}

#[test]
fn selector_file_mismatch_skips_file() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = select_fixture_file(&dir, &server.base_url());

    // Selector targets a different file that is not in the run set.
    let output = tarn()
        .args([
            "run",
            &file,
            "--select",
            "nonexistent.tarn.yaml::login",
            "--format",
            "json",
            "--no-progress",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let tests = parsed["files"][0]["tests"].as_array().unwrap();
    assert!(tests.is_empty(), "no tests should run: {:?}", tests);
}

// ============================================================================
// T53: --ndjson streaming reporter
// ============================================================================

#[test]
fn ndjson_streams_events_to_stdout_and_emits_done() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = write_test_file(
        &dir,
        "ndjson.tarn.yaml",
        &format!(
            r#"
name: NDJSON fixture
tests:
  good:
    steps:
      - name: health
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
  bad:
    steps:
      - name: wrong status
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 418
"#,
            base = server.base_url()
        ),
    );

    let output = tarn().args(["run", &file, "--ndjson"]).output().unwrap();

    assert_eq!(output.status.code(), Some(1), "suite has a failing test");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let events: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("every ndjson line must parse"))
        .collect();

    let names: Vec<&str> = events
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();

    assert!(
        names.contains(&"file_started"),
        "missing file_started: {:?}",
        names
    );
    assert!(
        names.contains(&"step_finished"),
        "missing step_finished: {:?}",
        names
    );
    assert!(
        names.contains(&"test_finished"),
        "missing test_finished: {:?}",
        names
    );
    assert!(
        names.contains(&"file_finished"),
        "missing file_finished: {:?}",
        names
    );
    assert_eq!(
        names.last().copied(),
        Some("done"),
        "done must be the final event: {:?}",
        names
    );

    let done = events.iter().find(|e| e["event"] == "done").unwrap();
    assert_eq!(done["summary"]["status"], "FAILED");
    assert_eq!(done["summary"]["files"], 1);
    assert_eq!(done["summary"]["steps"]["total"], 2);
    assert_eq!(done["summary"]["steps"]["passed"], 1);
    assert_eq!(done["summary"]["steps"]["failed"], 1);

    // Failed step carries failure category and assertion details.
    let failing_step = events
        .iter()
        .find(|e| e["event"] == "step_finished" && e["status"] == "FAILED")
        .expect("missing failed step_finished event");
    assert_eq!(failing_step["test"], "bad");
    assert_eq!(failing_step["failure_category"], "assertion_failed");
    assert_eq!(failing_step["error_code"], "assertion_mismatch");
    let failures = failing_step["assertion_failures"].as_array().unwrap();
    assert_eq!(failures[0]["assertion"], "status");
}

#[test]
fn ndjson_conflicts_with_non_human_stdout_format() {
    let dir = TempDir::new().unwrap();
    let file = write_test_file(
        &dir,
        "x.tarn.yaml",
        r#"
name: Stub
tests:
  t:
    steps:
      - name: s
        request:
          method: GET
          url: "http://127.0.0.1:1/"
"#,
    );

    // --ndjson + --format json on stdout is a hard conflict — two
    // different structured streams cannot share stdout. Expect exit 2.
    let output = tarn()
        .args(["run", &file, "--ndjson", "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--ndjson") && stderr.contains("stdout"),
        "expected stdout-conflict error, got: {}",
        stderr
    );
}

#[test]
fn ndjson_silently_suppresses_default_human_on_stdout() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = write_test_file(
        &dir,
        "default_human.tarn.yaml",
        &format!(
            r#"
name: Default human
tests:
  t:
    steps:
      - name: ping
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
"#,
            base = server.base_url()
        ),
    );

    let output = tarn().args(["run", &file, "--ndjson"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8(output.stdout).unwrap();
    // Every non-empty line must be a JSON object.
    for line in stdout.lines().filter(|l| !l.is_empty()) {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|e| panic!("non-JSON line leaked into stdout: {} ({})", line, e));
    }
}

#[test]
fn ndjson_composes_with_file_bound_json_format() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = write_test_file(
        &dir,
        "compose.tarn.yaml",
        &format!(
            r#"
name: Compose
tests:
  t:
    steps:
      - name: ping
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
"#,
            base = server.base_url()
        ),
    );
    let report_path = dir.path().join("run.json");
    let output = tarn()
        .args([
            "run",
            &file,
            "--ndjson",
            "--format",
            &format!("json={}", report_path.display()),
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));

    // stdout should be NDJSON.
    let stdout = String::from_utf8(output.stdout).unwrap();
    let events: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(events.iter().any(|e| e["event"] == "done"));

    // Final JSON report should still be written to the file.
    let report = std::fs::read_to_string(&report_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["summary"]["status"], "PASSED");
}

// ============================================================================
// T52: tarn validate --format json
// ============================================================================

#[test]
fn validate_json_reports_all_files_as_valid_when_no_errors() {
    let dir = TempDir::new().unwrap();
    let file = write_test_file(
        &dir,
        "ok.tarn.yaml",
        r#"
version: "1"
name: OK
tests:
  t:
    steps:
      - name: ping
        request:
          method: GET
          url: "http://localhost/"
        assert:
          status: 200
"#,
    );

    let output = tarn()
        .args(["validate", &file, "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let files = parsed["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["valid"], true);
    assert_eq!(files[0]["errors"].as_array().unwrap().len(), 0);
}

#[test]
fn validate_json_reports_yaml_parse_error_with_line_and_column() {
    let dir = TempDir::new().unwrap();
    let file = write_test_file(
        &dir,
        "broken.tarn.yaml",
        r#"name: "Broken
tests:
  t:
    steps:
      - name: unclosed
"#,
    );

    let output = tarn()
        .args(["validate", &file, "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let file_entry = &parsed["files"][0];
    assert_eq!(file_entry["valid"], false);
    let errors = file_entry["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0]["line"].as_u64().is_some(),
        "expected line number, got: {}",
        errors[0]
    );
    assert!(
        errors[0]["column"].as_u64().is_some(),
        "expected column number, got: {}",
        errors[0]
    );
    assert!(errors[0]["message"].as_str().unwrap().contains("quoted"));
}

#[test]
fn validate_json_reports_unknown_field_error() {
    let dir = TempDir::new().unwrap();
    let file = write_test_file(
        &dir,
        "unknown.tarn.yaml",
        r#"
name: Unknown field
tests:
  t:
    steps:
      - name: bad
        requestt:
          method: GET
          url: "http://localhost/"
"#,
    );

    let output = tarn()
        .args(["validate", &file, "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let file_entry = &parsed["files"][0];
    assert_eq!(file_entry["valid"], false);
    let errors = file_entry["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    let msg = errors[0]["message"].as_str().unwrap();
    assert!(
        msg.to_ascii_lowercase().contains("unknown field"),
        "expected unknown-field message, got: {}",
        msg
    );
    assert!(
        msg.contains("requestt"),
        "expected the offending field name in the message, got: {}",
        msg
    );
}

#[test]
fn validate_json_returns_structured_result_for_mixed_directory() {
    let dir = TempDir::new().unwrap();
    write_test_file(
        &dir,
        "ok.tarn.yaml",
        r#"
name: OK
tests:
  t:
    steps:
      - name: ok
        request:
          method: GET
          url: "http://localhost/"
"#,
    );
    write_test_file(
        &dir,
        "bad.tarn.yaml",
        r#"name: "Bad
tests:
"#,
    );

    let output = tarn()
        .args(["validate", dir.path().to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let files = parsed["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    let mut map: HashMap<String, bool> = HashMap::new();
    for f in files {
        let path = f["file"].as_str().unwrap().to_string();
        let valid = f["valid"].as_bool().unwrap();
        let stem = std::path::Path::new(&path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        map.insert(stem, valid);
    }
    assert_eq!(map["ok.tarn.yaml"], true);
    assert_eq!(map["bad.tarn.yaml"], false);
}

#[test]
fn validate_json_rejects_unknown_format_value() {
    let dir = TempDir::new().unwrap();
    let file = write_test_file(
        &dir,
        "x.tarn.yaml",
        r#"
name: X
tests:
  t:
    steps: []
"#,
    );

    let output = tarn()
        .args(["validate", &file, "--format", "yaml"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown validate format"),
        "expected format error, got: {}",
        stderr
    );
}

#[test]
fn validate_human_format_unchanged_by_default() {
    let dir = TempDir::new().unwrap();
    let file = write_test_file(
        &dir,
        "ok.tarn.yaml",
        r#"
name: OK
tests:
  t:
    steps:
      - name: ok
        request:
          method: GET
          url: "http://localhost/"
"#,
    );

    let output = tarn().args(["validate", &file]).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("✓"),
        "expected human checkmark, got: {}",
        stdout
    );
}

// ============================================================================
// T56: tarn env --json
// ============================================================================

fn write_env_fixture(dir: &TempDir, contents: &str) -> std::path::PathBuf {
    let path = dir.path().join("tarn.config.yaml");
    std::fs::write(&path, contents).unwrap();
    dir.path().to_path_buf()
}

#[test]
fn env_json_emits_stable_schema() {
    let dir = TempDir::new().unwrap();
    let root = write_env_fixture(
        &dir,
        r#"
environments:
  staging:
    vars:
      base_url: "https://staging.example.com"
  production:
    vars:
      base_url: "https://prod.example.com"
"#,
    );

    let output = tarn()
        .current_dir(&root)
        .args(["env", "--json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    assert!(parsed["project_root"].is_string(), "project_root missing");
    assert!(
        parsed["default_env_file"].is_string(),
        "default_env_file missing"
    );
    let envs = parsed["environments"].as_array().unwrap();
    assert_eq!(envs.len(), 2, "expected two environments: {:?}", envs);

    for env in envs {
        assert!(env["name"].is_string(), "name missing: {}", env);
        assert!(
            env["source_file"].is_string(),
            "source_file missing: {}",
            env
        );
        assert!(env["vars"].is_object(), "vars missing: {}", env);
    }

    let names: Vec<&str> = envs.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["production", "staging"], "expected alpha sort");
}

#[test]
fn env_json_redacts_configured_env_var_keys() {
    let dir = TempDir::new().unwrap();
    let root = write_env_fixture(
        &dir,
        r#"
redaction:
  env: [api_token]
environments:
  staging:
    vars:
      base_url: "https://staging.example.com"
      api_token: "super-secret"
      API_TOKEN: "also-secret"
"#,
    );

    let output = tarn()
        .current_dir(&root)
        .args(["env", "--json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let vars = &parsed["environments"][0]["vars"];
    assert_eq!(vars["base_url"], "https://staging.example.com");
    assert_eq!(vars["api_token"], "***");
    assert_eq!(vars["API_TOKEN"], "***", "match should be case-insensitive");
}

#[test]
fn env_json_honors_custom_redaction_replacement() {
    let dir = TempDir::new().unwrap();
    let root = write_env_fixture(
        &dir,
        r#"
redaction:
  env: [api_token]
  replacement: "[hidden]"
environments:
  staging:
    vars:
      api_token: "super-secret"
"#,
    );

    let output = tarn()
        .current_dir(&root)
        .args(["env", "--json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["environments"][0]["vars"]["api_token"], "[hidden]");
}

#[test]
fn env_json_handles_empty_environments_block() {
    let dir = TempDir::new().unwrap();
    let root = write_env_fixture(
        &dir,
        r#"
test_dir: tests
env_file: tarn.env.yaml
"#,
    );

    let output = tarn()
        .current_dir(&root)
        .args(["env", "--json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["environments"].as_array().unwrap().len(), 0);
}

#[test]
fn env_human_output_unchanged_by_default() {
    let dir = TempDir::new().unwrap();
    let root = write_env_fixture(
        &dir,
        r#"
environments:
  staging:
    vars:
      base_url: "https://staging.example.com"
"#,
    );

    let output = tarn().current_dir(&root).args(["env"]).output().unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Named environments:"));
    assert!(stdout.contains("staging"));
}

/// NAZ-260 / T55: every `StepResult` (setup, named test step, teardown)
/// and every `AssertionFailure` must carry a
/// `location: { file, line, column }` object pointing at the YAML node
/// that defined it. The file path must be absolute (or at least match
/// what tarn prints in other report fields), and line/column must be
/// 1-based. The field is optional for backwards compatibility but MUST
/// be present whenever the parser can resolve the source position —
/// which is the case for files without `include:` expansion.
#[test]
fn json_output_includes_source_locations_for_steps_and_assertions() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "locations.tarn.yaml",
        &format!(
            r#"name: Location metadata
setup:
  - name: warm up
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 200
tests:
  check:
    steps:
      - name: expect failure
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 418
          duration: "< 10000ms"
teardown:
  - name: cool down
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

    // At least one assertion fails (status 200 vs expected 418), so
    // the overall exit code is 1 — not 0 — but the JSON payload is
    // still well-formed.
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    let file = &json["files"][0];
    let setup_step = &file["setup"][0];
    let teardown_step = &file["teardown"][0];
    let test_step = &file["tests"][0]["steps"][0];

    // Every executed step surfaces a location object.
    for (label, step) in [
        ("setup", setup_step),
        ("test", test_step),
        ("teardown", teardown_step),
    ] {
        let loc = &step["location"];
        assert!(
            loc.is_object(),
            "{}: step location should be an object, got {:?}",
            label,
            loc
        );
        assert!(
            loc["file"]
                .as_str()
                .unwrap()
                .ends_with("locations.tarn.yaml"),
            "{}: file should end with locations.tarn.yaml, got {:?}",
            label,
            loc["file"]
        );
        assert!(
            loc["line"].as_u64().unwrap() >= 1,
            "{}: line must be 1-based",
            label
        );
        assert!(
            loc["column"].as_u64().unwrap() >= 1,
            "{}: column must be 1-based",
            label
        );
    }

    // Setup and teardown name nodes live on different source lines.
    let setup_line = setup_step["location"]["line"].as_u64().unwrap();
    let test_line = test_step["location"]["line"].as_u64().unwrap();
    let teardown_line = teardown_step["location"]["line"].as_u64().unwrap();
    assert!(setup_line < test_line);
    assert!(test_line < teardown_line);

    // The failing assertion (status mismatch) must carry a location
    // that points at the `status:` key — which sits on the line after
    // the step's `assert:` key, not the step's `name:` line.
    let failures = test_step["assertions"]["failures"]
        .as_array()
        .expect("failures array");
    assert!(!failures.is_empty(), "expected at least one failure");
    let status_failure = failures
        .iter()
        .find(|f| f["assertion"].as_str() == Some("status"))
        .expect("status failure");
    let status_loc = &status_failure["location"];
    assert!(
        status_loc.is_object(),
        "status failure should have a location object"
    );
    assert!(status_loc["file"]
        .as_str()
        .unwrap()
        .ends_with("locations.tarn.yaml"));
    let status_line = status_loc["line"].as_u64().unwrap();
    assert!(
        status_line > test_line,
        "status assertion should be below its step name line ({} > {})",
        status_line,
        test_line
    );

    // The detailed assertion entries also carry locations for both
    // status and duration (operator keys that the parser recognises).
    let details = test_step["assertions"]["details"]
        .as_array()
        .expect("details array");
    let status_detail = details
        .iter()
        .find(|d| d["assertion"].as_str() == Some("status"))
        .expect("status detail");
    assert!(status_detail["location"].is_object());
    let duration_detail = details
        .iter()
        .find(|d| d["assertion"].as_str() == Some("duration"))
        .expect("duration detail");
    assert!(duration_detail["location"].is_object());
    assert_eq!(
        duration_detail["location"]["line"].as_u64().unwrap(),
        status_line + 1,
        "duration key is one line below status key"
    );
}

// ============================================================================
// T57: tarn list --file PATH --format json
// ============================================================================

#[test]
fn list_file_json_emits_single_file_entry() {
    let dir = TempDir::new().unwrap();
    let file = write_test_file(
        &dir,
        "scoped.tarn.yaml",
        r#"
name: Scoped discovery
tags: [smoke, http]
setup:
  - name: warm
    request:
      method: GET
      url: "http://localhost/"
teardown:
  - name: cleanup
    request:
      method: POST
      url: "http://localhost/cleanup"
tests:
  happy:
    description: happy-path user flow
    tags: [critical]
    steps:
      - name: login
        request:
          method: POST
          url: "http://localhost/login"
      - name: fetch_profile
        request:
          method: GET
          url: "http://localhost/me"
"#,
    );

    let output = tarn()
        .args(["list", "--file", &file, "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    let files = parsed["files"].as_array().expect("files array");
    assert_eq!(files.len(), 1, "scoped listing should cover a single file");

    let entry = &files[0];
    assert_eq!(entry["file"].as_str().unwrap(), file);
    assert_eq!(entry["name"].as_str().unwrap(), "Scoped discovery");

    let tags: Vec<&str> = entry["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(tags, vec!["smoke", "http"]);

    // Setup and teardown carry step names only (shape is minimal).
    let setup = entry["setup"].as_array().unwrap();
    assert_eq!(setup.len(), 1);
    assert_eq!(setup[0]["name"].as_str().unwrap(), "warm");
    let teardown = entry["teardown"].as_array().unwrap();
    assert_eq!(teardown.len(), 1);
    assert_eq!(teardown[0]["name"].as_str().unwrap(), "cleanup");

    // No simple top-level steps.
    let flat_steps = entry["steps"].as_array().unwrap();
    assert!(flat_steps.is_empty());

    // One named test group with its two steps.
    let tests = entry["tests"].as_array().unwrap();
    assert_eq!(tests.len(), 1);
    let happy = &tests[0];
    assert_eq!(happy["name"].as_str().unwrap(), "happy");
    assert_eq!(
        happy["description"].as_str().unwrap(),
        "happy-path user flow"
    );
    let group_tags: Vec<&str> = happy["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(group_tags, vec!["critical"]);

    let group_steps = happy["steps"].as_array().unwrap();
    assert_eq!(group_steps.len(), 2);
    assert_eq!(group_steps[0]["name"].as_str().unwrap(), "login");
    assert_eq!(group_steps[1]["name"].as_str().unwrap(), "fetch_profile");
}

#[test]
fn list_file_json_exits_2_for_unknown_file() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("does_not_exist.tarn.yaml");

    let output = tarn()
        .args([
            "list",
            "--file",
            missing.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "unknown --file must exit with config error code 2"
    );

    // Even on config error the JSON wrapper shape is still a files array,
    // so callers do not need a special error path to read stdout.
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(parsed["files"].is_array());
    assert_eq!(parsed["files"].as_array().unwrap().len(), 0);
    let err = parsed["error"].as_str().unwrap();
    assert!(
        err.to_ascii_lowercase().contains("not found"),
        "expected a 'not found' error, got: {}",
        err
    );
}

#[test]
fn list_file_json_resolves_relative_path_outside_current_dir() {
    // Writes a fixture under a subdirectory and asks `tarn list --file`
    // with a path that is relative but does not live in `.` — the
    // scoped listing must still resolve it without requiring the editor
    // to normalise the path first.
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("nested/tests")).unwrap();
    let nested_path = dir.path().join("nested/tests/api.tarn.yaml");
    std::fs::write(
        &nested_path,
        r#"
name: Nested file
steps:
  - name: ping
    request:
      method: GET
      url: "http://localhost/ping"
"#,
    )
    .unwrap();

    // Drive tarn from `dir` so the argument `nested/tests/api.tarn.yaml`
    // is relative to a working directory different from where the
    // invocation would "naturally" sit, proving that scoped listing
    // does not depend on the workspace glob.
    let output = tarn()
        .current_dir(dir.path())
        .args([
            "list",
            "--file",
            "nested/tests/api.tarn.yaml",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let files = parsed["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    let entry = &files[0];
    assert_eq!(
        entry["file"].as_str().unwrap(),
        "nested/tests/api.tarn.yaml"
    );
    assert_eq!(entry["name"].as_str().unwrap(), "Nested file");
    let flat_steps = entry["steps"].as_array().unwrap();
    assert_eq!(flat_steps.len(), 1);
    assert_eq!(flat_steps[0]["name"].as_str().unwrap(), "ping");
}

#[test]
fn list_file_human_format_prints_single_file_only() {
    let dir = TempDir::new().unwrap();
    let file = write_test_file(
        &dir,
        "scoped_human.tarn.yaml",
        r#"
name: Scoped human
steps:
  - name: ping
    request:
      method: GET
      url: "http://localhost/"
"#,
    );
    // A second file in the same directory must NOT appear in the output
    // when --file targets only the first.
    write_test_file(
        &dir,
        "other.tarn.yaml",
        r#"
name: Other
steps:
  - name: should-not-appear
    request:
      method: GET
      url: "http://localhost/"
"#,
    );

    let output = tarn().args(["list", "--file", &file]).output().unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Scoped human"), "stdout: {}", stdout);
    assert!(stdout.contains("ping"), "stdout: {}", stdout);
    assert!(
        !stdout.contains("should-not-appear"),
        "unrelated file leaked into scoped list: {}",
        stdout
    );
}

#[test]
fn list_rejects_unknown_format_value() {
    let dir = TempDir::new().unwrap();
    let file = write_test_file(
        &dir,
        "x.tarn.yaml",
        r#"
name: X
tests:
  t:
    steps: []
"#,
    );

    let output = tarn()
        .args(["list", "--file", &file, "--format", "yaml"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown list format"),
        "expected format error, got: {}",
        stderr
    );
}

/// Helper: read the request headers rendered on the only failed step in a
/// JSON run result. Keeps the --redact-header tests concise and single-
/// purpose.
fn failed_step_request_headers(stdout: &[u8]) -> serde_json::Map<String, serde_json::Value> {
    let json: serde_json::Value = serde_json::from_slice(stdout)
        .unwrap_or_else(|e| panic!("invalid json: {e}: {}", String::from_utf8_lossy(stdout)));
    let step = &json["files"][0]["tests"][0]["steps"][0];
    assert_eq!(step["status"], "FAILED", "expected failed step: {step}");
    let request = step
        .get("request")
        .cloned()
        .unwrap_or_else(|| panic!("failed step has no request block: {step}"));
    request["headers"]
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("request block has no headers object: {request}"))
}

/// Baseline: custom headers are NOT redacted without `--redact-header`.
/// This is the control case that proves the extension's ask was real — a
/// literal `X-Custom-Token: secret-value` lands in the report.
#[test]
fn redact_header_flag_defaults_leave_custom_headers_unredacted() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "redact-default.tarn.yaml",
        &format!(
            r#"
name: Redact default
steps:
  - name: health
    request:
      method: GET
      url: "{}/health"
      headers:
        X-Custom-Token: "secret-value"
        X-Debug: "debug-value"
    assert:
      status: 404
"#,
            server.base_url()
        ),
    );

    let output = tarn()
        .args(["run", &test_file, "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let headers = failed_step_request_headers(&output.stdout);
    assert_eq!(
        headers.get("X-Custom-Token").and_then(|v| v.as_str()),
        Some("secret-value"),
        "custom header must appear unredacted by default: {headers:?}"
    );
    assert_eq!(
        headers.get("X-Debug").and_then(|v| v.as_str()),
        Some("debug-value"),
        "x-debug header must appear unredacted by default: {headers:?}"
    );
}

/// Core T58 behavior: a single `--redact-header` rewrites the matching
/// header value to the configured replacement. Uses deliberately-mixed
/// casing on both sides to prove matching is case-insensitive.
#[test]
fn redact_header_flag_redacts_custom_header_case_insensitively() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "redact-flag.tarn.yaml",
        &format!(
            r#"
name: Redact flag
steps:
  - name: health
    request:
      method: GET
      url: "{}/health"
      headers:
        X-Custom-Token: "secret-value"
        X-Debug: "debug-value"
    assert:
      status: 404
"#,
            server.base_url()
        ),
    );

    let output = tarn()
        .args([
            "run",
            &test_file,
            "--format",
            "json",
            // Lowercase flag value vs. mixed-case header name on the request.
            "--redact-header",
            "x-custom-token",
            // Uppercase flag value vs. mixed-case header name on the request.
            "--redact-header",
            "X-DEBUG",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let headers = failed_step_request_headers(&output.stdout);
    assert_eq!(
        headers.get("X-Custom-Token").and_then(|v| v.as_str()),
        Some("***"),
        "custom header must be redacted when passed via --redact-header: {headers:?}"
    );
    assert_eq!(
        headers.get("X-Debug").and_then(|v| v.as_str()),
        Some("***"),
        "x-debug must be redacted despite different casing: {headers:?}"
    );
}

/// `--redact-header` MUST merge with — never narrow — the built-in
/// defaults. Passing only a custom header must still redact
/// `Authorization` and other default headers.
#[test]
fn redact_header_flag_never_narrows_default_list() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "redact-merge.tarn.yaml",
        &format!(
            r#"
name: Redact merge
steps:
  - name: health
    request:
      method: GET
      url: "{}/health"
      headers:
        Authorization: "Bearer shhh"
        X-Custom-Token: "secret-value"
    assert:
      status: 404
"#,
            server.base_url()
        ),
    );

    let output = tarn()
        .args([
            "run",
            &test_file,
            "--format",
            "json",
            "--redact-header",
            "x-custom-token",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let headers = failed_step_request_headers(&output.stdout);
    assert_eq!(
        headers.get("Authorization").and_then(|v| v.as_str()),
        Some("***"),
        "default header redaction must still apply: {headers:?}"
    );
    assert_eq!(
        headers.get("X-Custom-Token").and_then(|v| v.as_str()),
        Some("***"),
        "custom header from CLI must be redacted: {headers:?}"
    );
}

/// `--redact-header` MUST merge with — never narrow — any `redaction:`
/// block already declared on the test file. This proves the CLI widens
/// on top of a file-level override that disables the default list.
#[test]
fn redact_header_flag_widens_file_level_redaction_block() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "redact-file-block.tarn.yaml",
        &format!(
            r#"
name: Redact file block
redaction:
  headers:
    - x-file-secret
steps:
  - name: health
    request:
      method: GET
      url: "{}/health"
      headers:
        X-File-Secret: "file-secret-value"
        X-Custom-Token: "secret-value"
    assert:
      status: 404
"#,
            server.base_url()
        ),
    );

    let output = tarn()
        .args([
            "run",
            &test_file,
            "--format",
            "json",
            "--redact-header",
            "X-Custom-Token",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let headers = failed_step_request_headers(&output.stdout);
    assert_eq!(
        headers.get("X-File-Secret").and_then(|v| v.as_str()),
        Some("***"),
        "file-level redaction block must still apply: {headers:?}"
    );
    assert_eq!(
        headers.get("X-Custom-Token").and_then(|v| v.as_str()),
        Some("***"),
        "CLI header must widen the file-level block: {headers:?}"
    );
}

#[test]
fn last_run_json_artifact_is_written_in_human_mode() {
    // Human-mode runs must still leave a machine-readable artifact so
    // failed runs can be inspected programmatically without rerunning.
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "last-run.tarn.yaml",
        r#"
name: Last-run artifact
steps:
  - name: connect failure
    request:
      method: GET
      url: "http://127.0.0.1:1/missing"
    assert:
      status: 200
"#,
    );

    let output = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));

    let artifact = dir.path().join(".tarn").join("last-run.json");
    assert!(
        artifact.is_file(),
        "expected {} to exist after a human-mode run",
        artifact.display()
    );

    let body = fs::read_to_string(&artifact).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("artifact is valid JSON");
    assert_eq!(parsed["summary"]["status"], "FAILED");
    assert_eq!(parsed["summary"]["steps"]["failed"], 1);

    // The terminal output should tell the user where the artifact went.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("json report saved to") && stderr.contains(".tarn/last-run.json"),
        "expected last-run.json path to be announced on stderr; got: {stderr}"
    );
}

#[test]
fn last_run_json_artifact_can_be_disabled() {
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "opt-out.tarn.yaml",
        r#"
name: Opt out
steps:
  - name: connect failure
    request:
      method: GET
      url: "http://127.0.0.1:1/missing"
    assert:
      status: 200
"#,
    );

    let output = tarn()
        .args(["run", &test_file, "--no-last-run-json"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));
    let artifact = dir.path().join(".tarn").join("last-run.json");
    assert!(
        !artifact.exists(),
        "--no-last-run-json must suppress the artifact; found {}",
        artifact.display()
    );
}

#[test]
fn downstream_steps_skip_when_prior_capture_fails() {
    // Classic cascade: first step captures `user_id` from a response
    // that doesn't contain it; every later step that references
    // `{{ capture.user_id }}` should be marked skipped, not flooded
    // with unresolved-template failures (NAZ-342).
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "cascade.tarn.yaml",
        &format!(
            r#"
name: Cascade skip
env:
  base_url: "{}"
steps:
  - name: capture missing id
    request:
      method: GET
      url: "{{{{ env.base_url }}}}/health"
    capture:
      user_id: "$.nonexistent"
    assert:
      status: 200
  - name: uses failed capture
    request:
      method: GET
      url: "{{{{ env.base_url }}}}/users/{{{{ capture.user_id }}}}"
    assert:
      status: 200
  - name: also uses failed capture
    request:
      method: GET
      url: "{{{{ env.base_url }}}}/users/{{{{ capture.user_id }}}}"
    assert:
      status: 200
"#,
            server.base_url(),
        ),
    );

    let output = tarn()
        .args(["run", &test_file, "--format", "json"])
        .output()
        .unwrap();

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let steps = json["files"][0]["tests"][0]["steps"].as_array().unwrap();

    // Step 1: real capture failure.
    assert_eq!(steps[0]["failure_category"], "capture_error");

    // Steps 2 and 3: cascade fallout.
    for (i, step) in steps.iter().enumerate().skip(1).take(2) {
        assert_eq!(
            step["failure_category"], "skipped_due_to_failed_capture",
            "step {i} should be classified as skip, got {step:#?}"
        );
        assert_eq!(step["error_code"], "skipped_dependency");
        let message = step["assertions"]["failures"][0]["message"]
            .as_str()
            .unwrap_or("");
        assert!(
            message.contains("user_id"),
            "expected message to name the missing capture: {message:?}"
        );
    }

    // Exit code stays at 3 because the root cause is a CaptureError —
    // the cascade skips must not escalate the run further.
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn exists_where_asserts_identity_without_array_index() {
    // Hits /users which returns an array with a known set of entries.
    // The test selects by email (a stable identifier) instead of
    // `$[0]`/exact length — the brittleness pattern NAZ-341 targets.
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "exists-where.tarn.yaml",
        &format!(
            r#"
name: Identity-based array assertion
env:
  base_url: "{}"
steps:
  - name: login
    request:
      method: POST
      url: "{{{{ env.base_url }}}}/auth/login"
      headers:
        Content-Type: application/json
      body:
        email: admin@example.com
        password: secret
    assert:
      status: 200
    capture:
      bearer: "$.token"
  - name: create user A
    request:
      method: POST
      url: "{{{{ env.base_url }}}}/users"
      headers:
        Authorization: "Bearer {{{{ capture.bearer }}}}"
        Content-Type: application/json
      body:
        name: Alice
        email: alice@example.com
    assert:
      status: 201
  - name: create user B
    request:
      method: POST
      url: "{{{{ env.base_url }}}}/users"
      headers:
        Authorization: "Bearer {{{{ capture.bearer }}}}"
        Content-Type: application/json
      body:
        name: Bob
        email: bob@example.com
    assert:
      status: 201
  - name: list users
    request:
      method: GET
      url: "{{{{ env.base_url }}}}/users"
      headers:
        Authorization: "Bearer {{{{ capture.bearer }}}}"
    assert:
      status: 200
      body:
        "$.data":
          exists_where:
            email: "alice@example.com"
          not_exists_where:
            email: "ghost@example.com"
"#,
            server.base_url(),
        ),
    );

    let output = tarn()
        .args(["run", &test_file, "--format", "json"])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "expected success; stdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn poll_timeout_includes_final_observed_value() {
    // /users/99999 always 404s — polling for status:200 will exhaust
    // attempts, and the new diagnostic must include the last response's
    // actual status so the operator can tell "stuck" from "progressing
    // but never matches".
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    let test_file = write_test_file(
        &dir,
        "poll-timeout.tarn.yaml",
        &format!(
            r#"
name: Poll timeout diagnostics
steps:
  - name: Poll missing user
    request:
      method: GET
      url: "{}/users/99999"
    poll:
      until:
        status: 200
      interval: 10ms
      max_attempts: 2
    assert:
      status: 200
"#,
            server.base_url(),
        ),
    );

    let output = tarn()
        .args(["run", &test_file, "--format", "json"])
        .output()
        .unwrap();

    // Polling timeouts are classified as runtime failures (exit code 3)
    // to match how CI systems surface "the endpoint never reached the
    // expected state" vs. a plain assertion mismatch.
    assert_eq!(output.status.code(), Some(3));

    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let step = &report["files"][0]["tests"][0]["steps"][0];

    assert_eq!(step["failure_category"], "timeout");
    assert_eq!(step["error_code"], "poll_condition_not_met");
    // Richer diagnostic must carry the final response's status/summary so
    // the JSON consumer (and the human renderer) can tell stuck from
    // brittle. Prior behavior left both fields null on timeout.
    let final_status = step["response_status"]
        .as_u64()
        .expect("final status present");
    assert!(
        (400..500).contains(&final_status),
        "expected a 4xx final status from /users/99999, got {final_status}"
    );
    let summary = step["response_summary"].as_str().unwrap_or("");
    assert!(
        summary.contains(&final_status.to_string()),
        "expected response_summary to echo the final status {}, got {:?}",
        final_status,
        summary
    );

    // The failing `poll.until` predicates must appear alongside the
    // top-level timeout message with their actual observed values.
    let details = step["assertions"]["details"].as_array().unwrap();
    let final_status_str = final_status.to_string();
    let final_state_emitted = details.iter().any(|d| {
        d["assertion"]
            .as_str()
            .map(|s| s.starts_with("poll final:"))
            .unwrap_or(false)
            && d["actual"]
                .as_str()
                .unwrap_or("")
                .contains(&final_status_str)
    });
    assert!(
        final_state_emitted,
        "expected a `poll final:` assertion surfacing the actual status {}, got {:#?}",
        final_status, details
    );
}

#[test]
fn last_run_json_artifact_path_can_be_overridden() {
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "override-path.tarn.yaml",
        r#"
name: Override
steps:
  - name: connect failure
    request:
      method: GET
      url: "http://127.0.0.1:1/missing"
    assert:
      status: 200
"#,
    );

    let custom = dir.path().join("reports").join("custom-run.json");
    let output = tarn()
        .args([
            "run",
            &test_file,
            "--report-json",
            &custom.display().to_string(),
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));
    assert!(custom.is_file(), "expected {} to exist", custom.display());
    // Default path should NOT be created when override is set.
    assert!(!dir.path().join(".tarn").join("last-run.json").exists());
}

// ============================================================
// NAZ-400: immutable per-run artifact directories
// ============================================================

fn write_failing_fixture(dir: &TempDir, name: &str) -> String {
    write_test_file(
        dir,
        name,
        r#"
name: Persistent artifact
steps:
  - name: connect failure
    request:
      method: GET
      url: "http://127.0.0.1:1/missing"
    assert:
      status: 200
"#,
    )
}

/// A fresh run must produce `.tarn/runs/<run_id>/` containing at
/// minimum `report.json` and `state.json`, and both files must carry
/// the same `run_id` that is announced on stderr.
#[test]
fn run_dir_artifacts_are_written_with_stable_run_id() {
    let dir = TempDir::new().unwrap();
    let test_file = write_failing_fixture(&dir, "runs-stable.tarn.yaml");

    let output = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let run_id = stderr
        .lines()
        .find_map(|line| line.strip_prefix("run id: "))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| panic!("expected `run id:` line on stderr; got: {stderr}"));

    assert!(
        stderr.contains("run artifacts: "),
        "expected `run artifacts:` line on stderr; got: {stderr}"
    );

    let run_dir_path = dir.path().join(".tarn").join("runs").join(&run_id);
    assert!(
        run_dir_path.is_dir(),
        "expected run directory at {}",
        run_dir_path.display()
    );

    let report = run_dir_path.join("report.json");
    let state = run_dir_path.join("state.json");
    assert!(report.is_file(), "expected {} to exist", report.display());
    assert!(state.is_file(), "expected {} to exist", state.display());

    let report_body = fs::read_to_string(&report).unwrap();
    let report_json: serde_json::Value = serde_json::from_str(&report_body).unwrap();
    assert_eq!(
        report_json["run_id"].as_str(),
        Some(run_id.as_str()),
        "report.json must embed the announced run_id"
    );
    assert_eq!(report_json["summary"]["status"], "FAILED");

    let state_body = fs::read_to_string(&state).unwrap();
    let state_json: serde_json::Value = serde_json::from_str(&state_body).unwrap();
    assert_eq!(
        state_json["run_id"].as_str(),
        Some(run_id.as_str()),
        "state.json must embed the same run_id"
    );
    assert_eq!(state_json["last_run"]["exit_code"], 3);
}

/// A second run must not destroy the first run's directory — that is
/// the whole point of the immutable archive.
#[test]
fn run_dir_is_preserved_across_consecutive_runs() {
    let dir = TempDir::new().unwrap();
    let test_file = write_failing_fixture(&dir, "runs-preserved.tarn.yaml");

    let first = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let first_stderr = String::from_utf8_lossy(&first.stderr).to_string();
    let first_run_id = first_stderr
        .lines()
        .find_map(|l| l.strip_prefix("run id: "))
        .unwrap()
        .trim()
        .to_string();

    // Sleep enough to drift past any same-second collision window,
    // though the random suffix also prevents collision.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let second = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let second_stderr = String::from_utf8_lossy(&second.stderr).to_string();
    let second_run_id = second_stderr
        .lines()
        .find_map(|l| l.strip_prefix("run id: "))
        .unwrap()
        .trim()
        .to_string();

    assert_ne!(first_run_id, second_run_id, "run ids must differ");

    let first_dir = dir.path().join(".tarn").join("runs").join(&first_run_id);
    let second_dir = dir.path().join(".tarn").join("runs").join(&second_run_id);
    assert!(
        first_dir.join("report.json").is_file(),
        "first run's report must survive the second run"
    );
    assert!(
        second_dir.join("report.json").is_file(),
        "second run must have its own report"
    );
}

/// The legacy `.tarn/last-run.json` pointer must always mirror the
/// latest run's `report.json`.
#[test]
fn last_run_pointer_mirrors_most_recent_run_dir() {
    let dir = TempDir::new().unwrap();
    let test_file = write_failing_fixture(&dir, "runs-pointer.tarn.yaml");

    let output = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let run_id = stderr
        .lines()
        .find_map(|l| l.strip_prefix("run id: "))
        .unwrap()
        .trim()
        .to_string();

    let run_report = dir
        .path()
        .join(".tarn")
        .join("runs")
        .join(&run_id)
        .join("report.json");
    let pointer = dir.path().join(".tarn").join("last-run.json");
    assert!(run_report.is_file());
    assert!(pointer.is_file());

    let run_body = fs::read_to_string(&run_report).unwrap();
    let pointer_body = fs::read_to_string(&pointer).unwrap();
    assert_eq!(
        run_body, pointer_body,
        "last-run.json pointer must be a byte-for-byte copy of the archived report"
    );
}

/// `--no-last-run-json` must suppress both the pointer and the
/// immutable archive — the user asked for a transient run.
#[test]
fn no_last_run_json_suppresses_run_dir_and_pointer() {
    let dir = TempDir::new().unwrap();
    let test_file = write_failing_fixture(&dir, "runs-optout.tarn.yaml");

    let output = tarn()
        .args(["run", &test_file, "--no-last-run-json"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));
    assert!(
        !dir.path().join(".tarn").join("last-run.json").exists(),
        "pointer must stay suppressed"
    );
    let runs_root = dir.path().join(".tarn").join("runs");
    if runs_root.exists() {
        let mut entries = fs::read_dir(&runs_root).unwrap();
        assert!(
            entries.next().is_none(),
            "runs dir must be empty under --no-last-run-json"
        );
    }
}

// ============================================================
// NAZ-401: summary.json and failures.json triage artifacts
// ============================================================

fn write_passing_fixture(dir: &TempDir, name: &str, base_url: &str) -> String {
    write_test_file(
        dir,
        name,
        &format!(
            r#"
name: Triage passing
tests:
  happy:
    steps:
      - name: health
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
"#,
            base = base_url
        ),
    )
}

fn run_id_from_stderr(stderr: &str) -> String {
    stderr
        .lines()
        .find_map(|l| l.strip_prefix("run id: "))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| panic!("expected `run id:` line on stderr; got: {stderr}"))
}

/// A passing run must still emit `summary.json` (with zero failures) and
/// `failures.json` (with an empty array) so tooling can always find the
/// artifacts without conditional discovery logic.
#[test]
fn summary_and_failures_emitted_on_passing_run() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_passing_fixture(&dir, "triage-pass.tarn.yaml", &server.base_url());

    let output = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let run_id = run_id_from_stderr(&stderr);
    let run_dir = dir.path().join(".tarn").join("runs").join(&run_id);

    let summary_path = run_dir.join("summary.json");
    let failures_path = run_dir.join("failures.json");
    assert!(summary_path.is_file(), "summary.json must exist in run dir");
    assert!(
        failures_path.is_file(),
        "failures.json must exist in run dir"
    );

    let summary: serde_json::Value =
        serde_json::from_slice(&fs::read(&summary_path).unwrap()).unwrap();
    assert_eq!(summary["exit_code"], 0);
    assert_eq!(summary["run_id"].as_str(), Some(run_id.as_str()));
    assert_eq!(summary["failed"]["files"], 0);
    assert_eq!(summary["failed"]["tests"], 0);
    assert_eq!(summary["failed"]["steps"], 0);
    assert!(summary["failed_files"].as_array().unwrap().is_empty());
    assert_eq!(summary["totals"]["files"], 1);
    assert!(summary["totals"]["steps"].as_u64().unwrap() >= 1);

    let failures: serde_json::Value =
        serde_json::from_slice(&fs::read(&failures_path).unwrap()).unwrap();
    assert_eq!(failures["run_id"].as_str(), Some(run_id.as_str()));
    assert!(failures["failures"].as_array().unwrap().is_empty());
}

/// A failing run populates `failures.json` with per-step entries that
/// include request method/url, response status, and a body excerpt —
/// the triage surface required by the acceptance criteria.
#[test]
fn failures_json_carries_request_response_and_counts() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "triage-fail.tarn.yaml",
        &format!(
            r#"
name: Triage failing
tests:
  happy:
    steps:
      - name: health
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 999
"#,
            base = server.base_url()
        ),
    );

    let output = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let run_id = run_id_from_stderr(&stderr);
    let run_dir = dir.path().join(".tarn").join("runs").join(&run_id);

    let summary: serde_json::Value =
        serde_json::from_slice(&fs::read(run_dir.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["exit_code"], 1);
    assert_eq!(summary["failed"]["files"], 1);
    assert_eq!(summary["failed"]["tests"], 1);
    assert_eq!(summary["failed"]["steps"], 1);
    let failed_files = summary["failed_files"].as_array().unwrap();
    assert_eq!(failed_files.len(), 1);
    assert!(
        failed_files[0]
            .as_str()
            .unwrap()
            .ends_with("triage-fail.tarn.yaml"),
        "failed_files should list the failing test file"
    );

    let failures: serde_json::Value =
        serde_json::from_slice(&fs::read(run_dir.join("failures.json")).unwrap()).unwrap();
    let entries = failures["failures"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["test"], "happy");
    assert_eq!(entry["step"], "health");
    assert_eq!(entry["failure_category"], "assertion_failed");
    assert_eq!(entry["request"]["method"], "GET");
    assert!(entry["request"]["url"]
        .as_str()
        .unwrap()
        .ends_with("/health"));
    let status = entry["response"]["status"].as_u64().unwrap();
    assert_eq!(status, 200);
    assert!(entry["response"]["body_excerpt"].is_string());
    assert!(
        entry["message"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("status"),
        "message should describe the failure"
    );
}

/// The workspace pointer files must mirror the archive byte-for-byte,
/// same invariant `.tarn/last-run.json` holds for `report.json`.
#[test]
fn summary_and_failures_pointers_mirror_run_dir() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_passing_fixture(&dir, "triage-pointer.tarn.yaml", &server.base_url());

    let output = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let run_id = run_id_from_stderr(&stderr);
    let run_dir = dir.path().join(".tarn").join("runs").join(&run_id);
    let pointer_dir = dir.path().join(".tarn");

    for name in ["summary.json", "failures.json"] {
        let archive = fs::read(run_dir.join(name)).unwrap();
        let pointer = fs::read(pointer_dir.join(name)).unwrap();
        assert_eq!(
            archive, pointer,
            "{name} pointer must be a byte-for-byte copy of the archived artifact",
        );
    }
}

/// `--no-last-run-json` must suppress `summary.json` and `failures.json`
/// in both the run directory and the workspace pointer, matching the
/// existing behavior for `last-run.json` and the run dir itself.
#[test]
fn no_last_run_json_suppresses_summary_and_failures() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "triage-optout.tarn.yaml",
        &format!(
            r#"
name: Triage opt-out
tests:
  happy:
    steps:
      - name: list
        request:
          method: GET
          url: "{base}/users"
        assert:
          status: 999
"#,
            base = server.base_url()
        ),
    );

    let output = tarn()
        .args(["run", &test_file, "--no-last-run-json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));

    assert!(
        !dir.path().join(".tarn").join("summary.json").exists(),
        "pointer summary.json must stay suppressed"
    );
    assert!(
        !dir.path().join(".tarn").join("failures.json").exists(),
        "pointer failures.json must stay suppressed"
    );

    let runs_root = dir.path().join(".tarn").join("runs");
    if runs_root.exists() {
        let mut entries = fs::read_dir(&runs_root).unwrap();
        assert!(
            entries.next().is_none(),
            "runs dir must stay empty under --no-last-run-json",
        );
    }
}

/// Long response bodies must be truncated past ~500 chars with a
/// marker so `failures.json` stays triage-sized.
#[test]
fn failures_json_truncates_long_body_excerpt() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    // /users returns an array large enough to overflow the excerpt cap.
    let test_file = write_test_file(
        &dir,
        "triage-truncate.tarn.yaml",
        &format!(
            r#"
name: Triage truncate
tests:
  happy:
    steps:
      - name: list
        request:
          method: GET
          url: "{base}/large"
        assert:
          status: 999
"#,
            base = server.base_url()
        ),
    );

    let output = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let run_id = run_id_from_stderr(&stderr);
    let run_dir = dir.path().join(".tarn").join("runs").join(&run_id);

    let failures: serde_json::Value =
        serde_json::from_slice(&fs::read(run_dir.join("failures.json")).unwrap()).unwrap();
    let entries = failures["failures"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    let excerpt = entries[0]["response"]["body_excerpt"]
        .as_str()
        .expect("body_excerpt should be populated");
    assert!(
        excerpt.contains("…[truncated,"),
        "long body must carry truncation marker, got: {}",
        excerpt
    );
}

// ============================================================
// NAZ-240 / NAZ-349 / NAZ-244: compact, llm, verbose-responses
// ============================================================

fn write_mixed_suite(dir: &TempDir, server: &DemoServer) -> String {
    write_test_file(
        dir,
        "mixed.tarn.yaml",
        &format!(
            r#"
name: Mixed suite
tests:
  happy:
    steps:
      - name: healthy
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
  broken:
    steps:
      - name: wrong status
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 418
"#,
            base = server.base_url()
        ),
    )
}

#[test]
fn compact_format_renders_header_and_fail_line() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_mixed_suite(&dir, &server);

    let output = tarn()
        .args(["run", &test_file, "--format", "compact"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("tarn: 1 file, 2 tests, 1/2 steps passed"),
        "header should summarise files/tests/steps: {}",
        stdout
    );
    assert!(
        stdout.contains("FAIL:"),
        "failure line expected in compact output: {}",
        stdout
    );
    // Grouping uses the actual status from the response, so a
    // `status: 418` assertion failing against a real 200 surfaces as
    // `HTTP 200: N`. That is deliberate — readers want to bucket by
    // what the server really did, not what the test expected.
    assert!(
        stdout.contains("HTTP 200: 1"),
        "trailing group summary expected: {}",
        stdout
    );
}

#[test]
fn compact_format_with_only_failed_hides_passing_files() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    // Two files: one fully passing, one failing
    let ok = write_test_file(
        &dir,
        "ok.tarn.yaml",
        &format!(
            r#"
name: ok
steps:
  - name: ping
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 200
"#,
            base = server.base_url()
        ),
    );
    let bad = write_test_file(
        &dir,
        "bad.tarn.yaml",
        &format!(
            r#"
name: bad
steps:
  - name: wrong
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 500
"#,
            base = server.base_url()
        ),
    );

    // `tarn run` takes a single path; the directory contains both
    // files, so let discovery pick them up.
    let _ = (ok, bad);
    let output = tarn()
        .args([
            "run",
            dir.path().to_str().unwrap(),
            "--format",
            "compact",
            "--only-failed",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("ok.tarn.yaml"), "got: {}", stdout);
    assert!(stdout.contains("bad.tarn.yaml"), "got: {}", stdout);
}

#[test]
fn llm_format_first_line_is_grep_friendly() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_mixed_suite(&dir, &server);

    let output = tarn()
        .args(["run", &test_file, "--format", "llm"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.lines().next().unwrap();
    assert!(
        first.starts_with("tarn: FAIL 1/2 steps, 1 failed, 1 file,"),
        "first line must be grep-friendly, got: {}",
        first
    );
    assert!(
        stdout.contains("FAIL "),
        "failure block missing in llm output: {}",
        stdout
    );
    assert!(
        stdout.contains("failure summary:"),
        "trailing summary missing: {}",
        stdout
    );
}

#[test]
fn llm_format_auto_selected_when_no_format_and_piped() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "auto.tarn.yaml",
        &format!(
            r#"
name: Auto llm
steps:
  - name: ping
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 200
"#,
            base = server.base_url()
        ),
    );

    // assert_cmd always captures stdout, so stdout is not a TTY. Without
    // `--format`, the run must pick `llm`.
    let output = tarn().args(["run", &test_file]).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.lines().next().unwrap();
    assert!(
        first.starts_with("tarn: PASS 1/1 steps"),
        "piped default should be llm, got: {}",
        first
    );
}

#[test]
fn verbose_responses_embeds_body_on_passing_step_in_json() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "vr.tarn.yaml",
        &format!(
            r#"
name: Verbose responses
steps:
  - name: healthy
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 200
"#,
            base = server.base_url()
        ),
    );

    let report_path = dir.path().join("out.json");
    let output = tarn()
        .args([
            "run",
            &test_file,
            "--verbose-responses",
            "--format",
            &format!("json={}", report_path.display()),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let content = fs::read_to_string(&report_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    // A file written with top-level `steps:` is rendered as a synthetic
    // test whose name is the file name, so the path is
    // files[0].tests[0].steps[0].
    let step = &parsed["files"][0]["tests"][0]["steps"][0];
    assert!(
        step["response"].is_object(),
        "passing step should include response when --verbose-responses is set: {}",
        parsed
    );
    assert!(step["response"]["body"].is_object() || step["response"]["body"].is_string());
}

#[test]
fn step_level_debug_true_embeds_body_without_global_flag() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "debug.tarn.yaml",
        &format!(
            r#"
name: Debug step
tests:
  t:
    steps:
      - name: debug step
        debug: true
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
      - name: plain step
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
"#,
            base = server.base_url()
        ),
    );

    let report_path = dir.path().join("out.json");
    let output = tarn()
        .args([
            "run",
            &test_file,
            "--format",
            &format!("json={}", report_path.display()),
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    let content = fs::read_to_string(&report_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    let steps = &parsed["files"][0]["tests"][0]["steps"];

    // `debug: true` step retains response_info...
    assert!(
        steps[0]["response"].is_object(),
        "debug:true step should embed response: {}",
        steps
    );
    // ...the plain sibling does not (global flag not set, step not debug).
    assert!(
        steps[1].get("response").is_none(),
        "plain step should not embed response: {}",
        steps
    );
}

#[test]
fn max_body_truncates_response_body_with_marker() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "trunc.tarn.yaml",
        &format!(
            r#"
name: Truncation
steps:
  - name: fat body
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 200
"#,
            base = server.base_url()
        ),
    );

    let report_path = dir.path().join("out.json");
    let output = tarn()
        .args([
            "run",
            &test_file,
            "--verbose-responses",
            "--max-body",
            "4",
            "--format",
            &format!("json={}", report_path.display()),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let content = fs::read_to_string(&report_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    let body = &parsed["files"][0]["tests"][0]["steps"][0]["response"]["body"];
    // Body must be a string containing the truncation marker once we
    // capped at 4 bytes — /health returns a multi-field JSON object.
    let as_str = body.as_str().unwrap_or_default();
    assert!(
        as_str.contains("<truncated:"),
        "expected truncation marker, got: {:?}",
        body
    );
}

#[test]
fn summary_subcommand_round_trips_last_run_json() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "summary.tarn.yaml",
        &format!(
            r#"
name: Summary round-trip
tests:
  t:
    steps:
      - name: ok
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 200
      - name: bad
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 418
"#,
            base = server.base_url()
        ),
    );

    let report_path = dir.path().join("run.json");
    let _ = tarn()
        .args([
            "run",
            &test_file,
            "--format",
            &format!("json={}", report_path.display()),
        ])
        .output()
        .unwrap();
    assert!(report_path.is_file());

    let output = tarn()
        .args(["summary", &report_path.display().to_string()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("tarn: FAIL 1/2 steps, 1 failed, 1 file,"),
        "summary should emit llm first line, got: {}",
        stdout
    );
    assert!(
        stdout.contains("failure summary:"),
        "summary block missing: {}",
        stdout
    );
}

#[test]
fn summary_subcommand_accepts_stdin() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "stdin.tarn.yaml",
        &format!(
            r#"
name: stdin summary
steps:
  - name: ok
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 200
"#,
            base = server.base_url()
        ),
    );

    // Produce a JSON report.
    let output = tarn()
        .args(["run", &test_file, "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let report = String::from_utf8(output.stdout).unwrap();

    let mut child = std::process::Command::new(tarn().get_program())
        .args(["summary", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(report.as_bytes())
        .unwrap();
    let done = child.wait_with_output().unwrap();
    assert_eq!(done.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&done.stdout);
    assert!(
        stdout.starts_with("tarn: PASS 1/1 steps"),
        "stdin summary failed: {}",
        stdout
    );
}

// ============================================================================
// NAZ-256 Req A: --test-filter and --step-filter shorthand flags
// ============================================================================

#[test]
fn test_filter_runs_only_the_named_test() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = select_fixture_file(&dir, &server.base_url());

    let output = tarn()
        .args([
            "run",
            &file,
            "--test-filter",
            "login",
            "--format",
            "json",
            "--no-progress",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let tests = parsed["files"][0]["tests"].as_array().unwrap();
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0]["name"], "login");
}

#[test]
fn step_filter_by_numeric_index_runs_single_step() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = select_fixture_file(&dir, &server.base_url());

    let output = tarn()
        .args([
            "run",
            &file,
            "--test-filter",
            "login",
            "--step-filter",
            "0",
            "--format",
            "json",
            "--no-progress",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let steps = parsed["files"][0]["tests"][0]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["name"], "step one");
}

#[test]
fn step_filter_by_name_runs_single_step() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = select_fixture_file(&dir, &server.base_url());

    let output = tarn()
        .args([
            "run",
            &file,
            "--test-filter",
            "login",
            "--step-filter",
            "step two",
            "--format",
            "json",
            "--no-progress",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let steps = parsed["files"][0]["tests"][0]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["name"], "step two");
}

#[test]
fn last_run_json_is_augmented_with_args_env_working_directory() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let file = select_fixture_file(&dir, &server.base_url());

    let output = tarn()
        .current_dir(dir.path())
        .args(["run", &file, "--no-progress"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let artifact = dir.path().join(".tarn").join("last-run.json");
    assert!(artifact.exists(), "last-run.json must exist");
    let raw = std::fs::read_to_string(&artifact).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();

    assert!(parsed["args"].is_array(), "args should be an array");
    assert_eq!(parsed["env_name"], serde_json::Value::Null);
    assert!(
        parsed["working_directory"].is_string(),
        "working_directory should be a string"
    );
    assert!(parsed["start_time"].is_string());
    assert!(parsed["end_time"].is_string());
}

// ============================================================================
// NAZ-402: tarn failures — root-cause grouping and cascade suppression
// ============================================================================

/// Running `tarn failures --format json` after a failing run must
/// group the failures and render the documented envelope.
#[test]
fn failures_subcommand_groups_failures_from_latest_run() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "grouping.tarn.yaml",
        &format!(
            r#"
name: Grouping
tests:
  t:
    steps:
      - name: bad
        request:
          method: GET
          url: "{base}/health"
        assert:
          status: 418
"#,
            base = server.base_url()
        ),
    );

    let run = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(1));

    let output = tarn()
        .args(["failures", "--format", "json", "--no-color"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));

    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["total_failures"], 1);
    assert_eq!(parsed["total_cascades"], 0);
    let groups = parsed["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    let fp = groups[0]["fingerprint"].as_str().unwrap();
    assert!(
        fp.starts_with("status:418:200:GET:"),
        "unexpected fingerprint: {}",
        fp
    );
    assert_eq!(groups[0]["occurrences"], 1);
    assert_eq!(groups[0]["root_cause"]["step"], "bad");
}

/// `tarn failures --run <id>` must read `.tarn/runs/<id>/failures.json`
/// directly, so the user can triage any historical run, not just the
/// latest one.
#[test]
fn failures_subcommand_loads_specific_run_by_id() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "by-run.tarn.yaml",
        &format!(
            r#"
name: By run id
steps:
  - name: bad
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 599
"#,
            base = server.base_url()
        ),
    );

    let run = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    let run_id = run_id_from_stderr(&stderr);

    let output = tarn()
        .args([
            "failures",
            "--run",
            &run_id,
            "--format",
            "json",
            "--no-color",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed["run_id"].as_str(), Some(run_id.as_str()));
    assert!(parsed["source"]
        .as_str()
        .unwrap()
        .contains(&format!(".tarn/runs/{}/failures.json", run_id)));
    assert_eq!(parsed["total_failures"], 1);
}

/// A nonexistent `--run` id must exit 2 with a clear message rather
/// than silently falling back to the latest run.
#[test]
fn failures_subcommand_unknown_run_id_exits_two() {
    let dir = TempDir::new().unwrap();
    // Seed `.tarn/` so the workspace is recognized as a tarn project.
    fs::create_dir_all(dir.path().join(".tarn")).unwrap();
    fs::write(dir.path().join("tarn.config.yaml"), "").unwrap();

    let output = tarn()
        .args(["failures", "--run", "does-not-exist", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does-not-exist") && stderr.contains("failures.json"),
        "error should reference the missing run and artifact: {}",
        stderr
    );
}

/// A passing run leaves `failures.json` populated with an empty list;
/// `tarn failures` on it must exit 0 and report no failures.
#[test]
fn failures_subcommand_exits_zero_when_no_failures() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "clean.tarn.yaml",
        &format!(
            r#"
name: Clean run
steps:
  - name: ok
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 200
"#,
            base = server.base_url()
        ),
    );

    let run = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(0));

    let output = tarn()
        .args(["failures", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed["total_failures"], 0);
    assert!(parsed["groups"].as_array().unwrap().is_empty());
}

/// A cascade failure must be counted in `total_cascades` and surface
/// as a `blocked_steps` entry on the upstream group rather than as a
/// second occurrence — this is the core promise of the command.
#[test]
fn failures_subcommand_collapses_cascade_fallout_under_root_cause() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "cascade-failures.tarn.yaml",
        &format!(
            r#"
name: Cascade skip
env:
  base_url: "{base}"
steps:
  - name: capture missing id
    request:
      method: GET
      url: "{{{{ env.base_url }}}}/health"
    capture:
      user_id: "$.nonexistent"
    assert:
      status: 200
  - name: uses failed capture
    request:
      method: GET
      url: "{{{{ env.base_url }}}}/users/{{{{ capture.user_id }}}}"
    assert:
      status: 200
  - name: also uses failed capture
    request:
      method: GET
      url: "{{{{ env.base_url }}}}/users/{{{{ capture.user_id }}}}"
    assert:
      status: 200
"#,
            base = server.base_url()
        ),
    );

    let run = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    // Capture error exits with 3; cascade skips do not escalate further.
    assert_eq!(run.status.code(), Some(3));

    let output = tarn()
        .args(["failures", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(
        parsed["total_cascades"].as_u64().unwrap() >= 2,
        "expected at least two cascades, got: {}",
        parsed
    );
    let groups = parsed["groups"].as_array().unwrap();
    // Root cause has exactly one occurrence; cascades do not inflate it.
    let root_group = groups
        .iter()
        .find(|g| g["root_cause"]["step"] == "capture missing id")
        .expect("root-cause group present");
    assert_eq!(root_group["occurrences"], 1);
    let blocked = root_group["blocked_steps"].as_array().unwrap();
    let blocked_names: Vec<&str> = blocked
        .iter()
        .map(|b| b["step"].as_str().unwrap())
        .collect();
    assert!(
        blocked_names.contains(&"uses failed capture")
            && blocked_names.contains(&"also uses failed capture"),
        "both downstream skips must surface as blocked_steps: {:?}",
        blocked_names
    );
}

// ============================================================================
// NAZ-403: tarn rerun — rerun the failing subset of a prior run
// ============================================================================

/// Read the "status" string (PASSED / FAILED) out of a `.tarn/last-run.json`
/// report so tests can assert on the rerun's shape without pulling in the
/// full `RunResult` deserializer.
fn read_last_run_report(dir: &std::path::Path) -> serde_json::Value {
    let path = dir.join(".tarn").join("last-run.json");
    let raw =
        std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
    serde_json::from_slice(&raw).unwrap()
}

/// Count failing tests in `.tarn/runs/<run_id>/report.json`.
fn count_executed_tests(report: &serde_json::Value) -> usize {
    report["files"]
        .as_array()
        .map(|files| {
            files
                .iter()
                .map(|f| f["tests"].as_array().map(|t| t.len()).unwrap_or(0))
                .sum()
        })
        .unwrap_or(0)
}

/// Running `tarn rerun --failed` after a mixed pass/fail run reruns only
/// the failing test, announces the selection on stderr, stamps the
/// source run id onto `rerun_source`, and produces a fresh run archive.
#[test]
fn rerun_failed_only_reruns_failing_tests_from_latest_run() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    // Mixed suite: one passing, one failing test in the same file. The
    // rerun must pick up only the failing one.
    let test_file = write_test_file(
        &dir,
        "mixed.tarn.yaml",
        &format!(
            r#"
name: Mixed suite
env:
  base_url: "{base}"
tests:
  happy:
    steps:
      - name: health ok
        request:
          method: GET
          url: "{{{{ env.base_url }}}}/health"
        assert:
          status: 200
  sad:
    steps:
      - name: health bad
        request:
          method: GET
          url: "{{{{ env.base_url }}}}/health"
        assert:
          status: 418
"#,
            base = server.base_url()
        ),
    );

    let first = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(1));
    let first_stderr = String::from_utf8_lossy(&first.stderr).to_string();
    let first_run_id = run_id_from_stderr(&first_stderr);

    let rerun = tarn()
        .args(["rerun", "--failed"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(rerun.status.code(), Some(1));
    let rerun_stderr = String::from_utf8_lossy(&rerun.stderr).to_string();

    assert!(
        rerun_stderr.contains("rerun: selected 1 test from run"),
        "expected announcement header, got: {}",
        rerun_stderr
    );
    assert!(
        rerun_stderr.contains("::sad"),
        "expected `::sad` in selection bullet list, got: {}",
        rerun_stderr
    );
    assert!(
        !rerun_stderr.contains("::happy"),
        "passing test must not be in the rerun selection: {}",
        rerun_stderr
    );

    let new_run_id = run_id_from_stderr(&rerun_stderr);
    assert_ne!(new_run_id, first_run_id, "rerun must mint a fresh run_id");

    // Original archive is preserved.
    assert!(
        dir.path()
            .join(".tarn")
            .join("runs")
            .join(&first_run_id)
            .is_dir(),
        "original run archive must persist across rerun"
    );

    // New archive exists and only executed the failing test.
    let new_report_path = dir
        .path()
        .join(".tarn")
        .join("runs")
        .join(&new_run_id)
        .join("report.json");
    assert!(new_report_path.is_file());
    let new_report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&new_report_path).unwrap()).unwrap();
    assert_eq!(
        count_executed_tests(&new_report),
        1,
        "rerun must execute exactly one test, got: {}",
        new_report
    );

    // rerun_source provenance is stamped onto the report.
    let rerun_src = &new_report["rerun_source"];
    assert_eq!(rerun_src["run_id"].as_str(), Some(first_run_id.as_str()));
    assert_eq!(rerun_src["selected_count"], 1);
    assert!(rerun_src["source_path"]
        .as_str()
        .unwrap()
        .ends_with(".tarn/failures.json"));

    // summary.json also gets the source stamp.
    let summary_path = dir
        .path()
        .join(".tarn")
        .join("runs")
        .join(&new_run_id)
        .join("summary.json");
    let summary: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&summary_path).unwrap()).unwrap();
    assert_eq!(summary["rerun_source"]["run_id"], first_run_id);

    // Pointer at `.tarn/last-run.json` is refreshed to the rerun.
    let pointer = read_last_run_report(dir.path());
    assert_eq!(pointer["run_id"].as_str(), Some(new_run_id.as_str()));
    assert_eq!(pointer["rerun_source"]["run_id"], first_run_id);
}

/// `tarn rerun --failed --run <id>` loads the specified archive instead
/// of the latest pointer, which lets the user replay an older run after
/// a subsequent run has overwritten the pointer.
#[test]
fn rerun_failed_with_explicit_run_id_uses_archive() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let failing = write_test_file(
        &dir,
        "failing.tarn.yaml",
        &format!(
            r#"
name: Failing
steps:
  - name: boom
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 599
"#,
            base = server.base_url()
        ),
    );
    let passing = write_test_file(
        &dir,
        "passing.tarn.yaml",
        &format!(
            r#"
name: Passing
steps:
  - name: ok
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 200
"#,
            base = server.base_url()
        ),
    );

    // Run the failing suite first, then the passing one. The latest
    // pointer now points at a clean run, but the failing run's archive
    // must still be reachable via `--run <id>`.
    let first = tarn()
        .args(["run", &failing])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(1));
    let failing_run_id = run_id_from_stderr(&String::from_utf8_lossy(&first.stderr));

    let second = tarn()
        .args(["run", &passing])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(0));

    let rerun = tarn()
        .args(["rerun", "--failed", "--run", &failing_run_id])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(rerun.status.code(), Some(1));
    let rerun_stderr = String::from_utf8_lossy(&rerun.stderr).to_string();
    assert!(
        rerun_stderr.contains(&format!("from run {}", failing_run_id)),
        "expected explicit run id in announcement, got: {}",
        rerun_stderr
    );

    let new_run_id = run_id_from_stderr(&rerun_stderr);
    let new_report: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            dir.path()
                .join(".tarn")
                .join("runs")
                .join(&new_run_id)
                .join("report.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(new_report["rerun_source"]["run_id"], failing_run_id);
    assert!(new_report["rerun_source"]["source_path"]
        .as_str()
        .unwrap()
        .contains(&format!(".tarn/runs/{}/failures.json", failing_run_id)));
}

/// An unknown `--run <id>` must exit 2 with a clear error, not silently
/// fall back to the latest run (which could produce surprising reruns).
#[test]
fn rerun_with_unknown_run_id_exits_two() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".tarn")).unwrap();
    std::fs::write(dir.path().join("tarn.config.yaml"), "").unwrap();

    let output = tarn()
        .args(["rerun", "--failed", "--run", "does-not-exist"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does-not-exist") || stderr.contains("failures.json"),
        "error should reference the missing archive: {}",
        stderr
    );
}

/// A source run where every test passed has no failing subset to rerun.
/// The command must exit 0 and MUST NOT create an empty run artifact,
/// so `.tarn/runs/` remains a faithful record of "runs that did work."
#[test]
fn rerun_on_clean_source_exits_zero_without_new_artifacts() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "clean.tarn.yaml",
        &format!(
            r#"
name: Clean
steps:
  - name: ok
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 200
"#,
            base = server.base_url()
        ),
    );

    let first = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(0));

    let runs_dir = dir.path().join(".tarn").join("runs");
    let runs_before: Vec<_> = std::fs::read_dir(&runs_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|d| d.file_name()))
        .collect();
    assert_eq!(runs_before.len(), 1, "exactly one archive after first run");

    let rerun = tarn()
        .args(["rerun", "--failed"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(rerun.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&rerun.stderr);
    assert!(
        stderr.contains("no failing tests to rerun"),
        "expected the 'no failures to rerun' message, got: {}",
        stderr
    );
    assert!(
        !stderr.contains("run id:"),
        "must not create a new run archive: {}",
        stderr
    );

    let runs_after: Vec<_> = std::fs::read_dir(&runs_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|d| d.file_name()))
        .collect();
    assert_eq!(
        runs_after, runs_before,
        "`.tarn/runs/` must be unchanged when there is nothing to rerun"
    );
}

/// `tarn rerun` without `--failed` is an error: we refuse to silently
/// run the full suite because the subcommand's name implies a subset.
#[test]
fn rerun_without_failed_flag_exits_two() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".tarn")).unwrap();
    std::fs::write(dir.path().join("tarn.config.yaml"), "").unwrap();

    let output = tarn()
        .args(["rerun"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--failed"),
        "error must hint at the required flag: {}",
        stderr
    );
}

// ============================================================================
// NAZ-405: tarn inspect + tarn diff — drill-down and run comparison
// ============================================================================

/// Run a mixed pass/fail workload and inspect the resulting archive at
/// the run level. The JSON output must carry the expected counts and
/// list the failing file so a caller can drill down without opening the
/// full report.
#[test]
fn inspect_run_level_json_reports_failed_file_and_counts() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "mixed.tarn.yaml",
        &format!(
            r#"
name: Mixed
env:
  base_url: "{base}"
tests:
  happy:
    steps:
      - name: health_ok
        request:
          method: GET
          url: "{{{{ env.base_url }}}}/health"
        assert:
          status: 200
  sad:
    steps:
      - name: health_bad
        request:
          method: GET
          url: "{{{{ env.base_url }}}}/health"
        assert:
          status: 418
"#,
            base = server.base_url()
        ),
    );

    let first = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(1));
    let run_id = run_id_from_stderr(&String::from_utf8_lossy(&first.stderr));

    let inspect = tarn()
        .args(["inspect", &run_id, "--format", "json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(inspect.status.code(), Some(0));
    let view: serde_json::Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(view["target"], "run");
    assert_eq!(view["run_id"], run_id);
    assert_eq!(view["failed"]["tests"], 1);
    assert_eq!(view["failed"]["files"], 1);
    let failed_files = view["failed_files"].as_array().unwrap();
    assert_eq!(failed_files.len(), 1);
    assert!(failed_files[0]["file"]
        .as_str()
        .unwrap()
        .ends_with("mixed.tarn.yaml"));
}

/// `tarn inspect <id> FILE::TEST::STEP` must surface the step's
/// request, response, and assertion details — the canonical "open a
/// failing step without parsing the whole report" path from the
/// acceptance criteria.
#[test]
fn inspect_step_level_exposes_request_response_and_assertions() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "bad.tarn.yaml",
        &format!(
            r#"
name: Bad
env:
  base_url: "{base}"
tests:
  sad:
    steps:
      - name: boom
        request:
          method: GET
          url: "{{{{ env.base_url }}}}/health"
        assert:
          status: 418
"#,
            base = server.base_url()
        ),
    );

    let first = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(1));
    let run_id = run_id_from_stderr(&String::from_utf8_lossy(&first.stderr));

    let inspect = tarn()
        .args([
            "inspect",
            &run_id,
            &format!("{}::sad::boom", test_file),
            "--format",
            "json",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(inspect.status.code(), Some(0));
    let view: serde_json::Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(view["target"], "step");
    assert_eq!(view["step"]["status"], "FAILED");
    assert_eq!(view["step"]["request"]["method"], "GET");
    assert_eq!(view["step"]["response"]["status"], 200);
    let asserts = view["step"]["assertions"].as_array().unwrap();
    assert!(asserts.iter().any(|a| a["assertion"] == "status"));
    assert_eq!(view["step"]["failure_category"], "assertion_failed");
}

/// `tarn inspect last` must resolve to the most recent archive so the
/// user can drill down without copy-pasting a run id.
#[test]
fn inspect_last_alias_targets_most_recent_archive() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "ok.tarn.yaml",
        &format!(
            r#"
name: Ok
steps:
  - name: ok
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 200
"#,
            base = server.base_url()
        ),
    );
    let run = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(0));
    let run_id = run_id_from_stderr(&String::from_utf8_lossy(&run.stderr));

    let inspect = tarn()
        .args(["inspect", "last", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(inspect.status.code(), Some(0));
    let view: serde_json::Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(view["run_id"], run_id);
}

/// An unknown run id must exit 2 rather than silently falling back to
/// the latest archive — otherwise automation that passes a stale id
/// would inspect a different run without knowing it.
#[test]
fn inspect_unknown_run_id_exits_two() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".tarn")).unwrap();
    std::fs::write(dir.path().join("tarn.config.yaml"), "").unwrap();
    let output = tarn()
        .args(["inspect", "bogus-id"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bogus-id") || stderr.contains("unknown run id"),
        "error must point at the bad id: {}",
        stderr
    );
}

/// Given two runs where the 2nd fixes one failure, introduces a new
/// one, and keeps an existing one, `tarn diff` must classify each into
/// the right bucket (new / fixed / persistent). This matches the
/// "improved, regressed, or shifted" acceptance criterion.
#[test]
fn diff_classifies_new_fixed_and_persistent_failures() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();

    // Run A: two tests, one fails on /health (418), one fails asserting
    // a missing JSONPath on /health-ish response.
    let first_yaml = format!(
        r#"
name: A
env:
  base_url: "{base}"
tests:
  persistent_fail:
    steps:
      - name: persistent
        request:
          method: GET
          url: "{{{{ env.base_url }}}}/health"
        assert:
          status: 418
  fixed_later:
    steps:
      - name: soon_fixed
        request:
          method: GET
          url: "{{{{ env.base_url }}}}/health"
        assert:
          status: 419
"#,
        base = server.base_url()
    );
    let path_a = write_test_file(&dir, "suite.tarn.yaml", &first_yaml);
    let a = tarn()
        .args(["run", &path_a])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(a.status.code(), Some(1));
    let run_a = run_id_from_stderr(&String::from_utf8_lossy(&a.stderr));

    // Run B: the `fixed_later` test is now passing, a new failure is
    // introduced under `new_fail`, and `persistent_fail` still fails.
    let second_yaml = format!(
        r#"
name: B
env:
  base_url: "{base}"
tests:
  persistent_fail:
    steps:
      - name: persistent
        request:
          method: GET
          url: "{{{{ env.base_url }}}}/health"
        assert:
          status: 418
  fixed_later:
    steps:
      - name: soon_fixed
        request:
          method: GET
          url: "{{{{ env.base_url }}}}/health"
        assert:
          status: 200
  new_fail:
    steps:
      - name: freshly_broken
        request:
          method: GET
          url: "{{{{ env.base_url }}}}/health"
        assert:
          status: 420
"#,
        base = server.base_url()
    );
    std::fs::write(&path_a, second_yaml).unwrap();
    let b = tarn()
        .args(["run", &path_a])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(b.status.code(), Some(1));
    let run_b = run_id_from_stderr(&String::from_utf8_lossy(&b.stderr));

    let diff = tarn()
        .args(["diff", &run_a, &run_b, "--format", "json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(
        diff.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&diff.stderr)
    );
    let view: serde_json::Value = serde_json::from_slice(&diff.stdout).unwrap();
    assert_eq!(view["from"]["run_id"], run_a);
    assert_eq!(view["to"]["run_id"], run_b);

    let new = view["new"].as_array().unwrap();
    let fixed = view["fixed"].as_array().unwrap();
    let persistent = view["persistent"].as_array().unwrap();
    assert_eq!(
        new.len(),
        1,
        "expected exactly one new failure group; view: {}",
        view
    );
    assert_eq!(
        fixed.len(),
        1,
        "expected exactly one fixed failure group; view: {}",
        view
    );
    assert_eq!(
        persistent.len(),
        1,
        "expected exactly one persistent failure group; view: {}",
        view
    );
    assert_eq!(
        view["totals_delta"]["failed_tests"], 0,
        "tests shifted but count stayed at 2 → delta is 0; view: {}",
        view
    );
}

/// `tarn diff last prev` must resolve both aliases against the
/// `.tarn/runs/` directory so users don't have to copy run ids.
#[test]
fn diff_last_prev_aliases_resolve_to_recent_archives() {
    let server = DemoServer::start();
    let dir = TempDir::new().unwrap();
    let test_file = write_test_file(
        &dir,
        "simple.tarn.yaml",
        &format!(
            r#"
name: simple
steps:
  - name: ok
    request:
      method: GET
      url: "{base}/health"
    assert:
      status: 200
"#,
            base = server.base_url()
        ),
    );
    let first = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(0));
    let run_a = run_id_from_stderr(&String::from_utf8_lossy(&first.stderr));

    // The run_id prefix is second-resolution, so back-to-back runs in
    // the same second would tiebreak by the random suffix. Sleep past
    // the boundary so lexical and chronological order agree and the
    // `prev` / `last` aliases map unambiguously to run_a / run_b.
    std::thread::sleep(Duration::from_millis(1100));
    let second = tarn()
        .args(["run", &test_file])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(0));
    let run_b = run_id_from_stderr(&String::from_utf8_lossy(&second.stderr));
    assert!(
        run_b > run_a,
        "lexical order must track chronological order: a={} b={}",
        run_a,
        run_b
    );

    let diff = tarn()
        .args(["diff", "prev", "last", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(diff.status.code(), Some(0));
    let view: serde_json::Value = serde_json::from_slice(&diff.stdout).unwrap();
    assert_eq!(view["from"]["run_id"], run_a);
    assert_eq!(view["to"]["run_id"], run_b);
}
