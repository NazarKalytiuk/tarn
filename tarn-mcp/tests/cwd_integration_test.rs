//! Integration tests for the `cwd` MCP tool parameter (NAZ-248).
//!
//! These exercise the end-to-end wiring between the public
//! `tarn_mcp::tools::tarn_run` entry point,
//! `tarn::env::resolve_env_with_profiles`, and
//! `tarn::config::load_config` — specifically verifying that:
//!
//!   (a) an explicit absolute `cwd` is used as the project root for
//!       `tarn.config.yaml` + `tarn.env.yaml` discovery,
//!   (b) when no `cwd` is given, the server falls back to the process
//!       current directory,
//!   (c) an explicit `cwd` that contains no `tarn.config.yaml` produces
//!       a fail-fast error whose message names the resolved path.
//!
//! A minimal HTTP server runs on an ephemeral port so the tests can
//! confirm that `{{ env.base_url }}` expansion actually reaches the
//! server (proving env resolution picked up the right env file),
//! without needing a real backend.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

/// Tests here flip the process-wide current directory with
/// `set_current_dir`, which is inherently racy when cargo runs tests in
/// parallel. Serializing every test in this file through a single
/// mutex keeps their cwd changes from leaking into each other.
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Tiny HTTP/1.1 server that accepts connections and returns `200 OK`
/// with a short JSON body. Used only to give `tarn_run` a real socket
/// to hit — the body itself is not the point of the test, the
/// `env.base_url` interpolation reaching the server is.
struct SimpleServer {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl SimpleServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener
            .set_nonblocking(true)
            .expect("set listener nonblocking");
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();

        let handle = thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => handle_one(stream),
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            port,
            stop,
            thread: Some(handle),
        }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for SimpleServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

fn handle_one(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut buf = [0u8; 2048];
    let _ = stream.read(&mut buf);
    let body = b"{\"ok\":true}";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// Write a minimal Tarn project into `dir`:
///   - `tarn.config.yaml` (so an explicit cwd passes the fail-fast check)
///   - `tarn.env.yaml`   (supplies `{{ env.base_url }}`)
///   - `tests/health.tarn.yaml` (a single-step GET that expects 200)
fn scaffold_project(dir: &Path, base_url: &str) {
    std::fs::write(dir.join("tarn.config.yaml"), "test_dir: tests\n").unwrap();
    std::fs::write(
        dir.join("tarn.env.yaml"),
        format!("base_url: {base_url}\n"),
    )
    .unwrap();
    let tests_dir = dir.join("tests");
    std::fs::create_dir_all(&tests_dir).unwrap();
    std::fs::write(
        tests_dir.join("health.tarn.yaml"),
        r#"name: health
steps:
  - name: ping
    request:
      method: GET
      url: "{{ env.base_url }}/health"
    assert:
      status: 200
"#,
    )
    .unwrap();
}

/// Extract the outcome status for the first step of the run report, so
/// tests can assert on pass/fail without hand-parsing the full tree.
fn first_step_status(report: &Value) -> &str {
    report
        .pointer("/files/0/tests/0/steps/0/status")
        .and_then(|v| v.as_str())
        .or_else(|| {
            report
                .pointer("/files/0/setup/0/status")
                .and_then(|v| v.as_str())
        })
        .unwrap_or("UNKNOWN")
}

#[test]
fn tarn_run_uses_explicit_cwd_for_config_and_env() {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let server = SimpleServer::start();
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    // Deliberately set the process cwd to somewhere else so that any
    // accidental fallback to process cwd would fail to find the env
    // file — the whole point is to prove `cwd` is honored.
    let other = tempfile::TempDir::new().unwrap();
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(other.path()).unwrap();

    let params = json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    });
    let result = tarn_mcp::tools::tarn_run(&params);

    // Always restore cwd before asserting to avoid leaving other tests
    // with a surprising working directory.
    std::env::set_current_dir(previous).unwrap();

    let report = result.expect("tarn_run should succeed with explicit cwd");
    assert_eq!(first_step_status(&report), "PASSED", "report: {report:#}");
}

#[test]
fn tarn_run_defaults_to_process_cwd_when_cwd_omitted() {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let server = SimpleServer::start();
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let params = json!({ "path": "tests" });
    let result = tarn_mcp::tools::tarn_run(&params);

    std::env::set_current_dir(previous).unwrap();

    let report = result.expect("tarn_run should succeed with default cwd = process cwd");
    assert_eq!(first_step_status(&report), "PASSED", "report: {report:#}");
}

#[test]
fn tarn_run_errors_when_explicit_cwd_lacks_tarn_config() {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // A real, existing directory — but no tarn.config.yaml inside.
    let tmp = tempfile::TempDir::new().unwrap();
    let params = json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    });

    let err = tarn_mcp::tools::tarn_run(&params)
        .expect_err("tarn_run must fail fast when explicit cwd has no tarn.config.yaml");
    assert!(err.contains("tarn.config.yaml"), "got: {err}");
    // The error must name the exact path we looked at — that is
    // requirement #4 (do not silently fall back) and is what lets an
    // agent fix its own tool call without extra probing.
    let expected = tmp.path().join("tarn.config.yaml");
    assert!(
        err.contains(&expected.display().to_string()),
        "error `{err}` should mention resolved path `{}`",
        expected.display()
    );
}

#[test]
fn tarn_run_rejects_relative_cwd() {
    let err = tarn_mcp::tools::tarn_run(&json!({ "cwd": "not/absolute" }))
        .expect_err("relative cwd must be rejected");
    assert!(err.to_lowercase().contains("absolute"), "got: {err}");
}

#[test]
fn tarn_run_rejects_nonexistent_cwd() {
    let err = tarn_mcp::tools::tarn_run(&json!({
        "cwd": "/definitely/not/a/real/tarn/cwd/naz-248"
    }))
    .expect_err("missing cwd must be rejected");
    assert!(err.contains("does not exist"), "got: {err}");
}
