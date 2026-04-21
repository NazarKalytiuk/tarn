//! Integration tests for the NAZ-407 artifact-oriented MCP tools.
//!
//! These exercise the new public entry points (`tarn_run` with
//! `report_mode` variants, `tarn_last_failures`, `tarn_get_run_artifacts`,
//! `tarn_rerun_failed`, `tarn_report`, `tarn_inspect`) against a real
//! (but local) HTTP server so we can prove:
//!
//!   - the response carries artifact paths the caller can open back,
//!   - the artifacts actually exist on disk once the tool returns,
//!   - `tarn_rerun_failed` produces a *new* run_id (not a pointer to
//!     the source run) and a new archive directory,
//!   - error payloads carry the structured `{code, message, data}`
//!     triple rather than stringified-anything.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

use serde_json::json;

// Tests here flip the process-wide current directory, which is racy
// under `cargo test`'s default parallel runner. A single mutex
// serialises every test in this file.
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Tiny HTTP server that can be configured to return 200 always, or a
/// failing status on the first N hits then 200 — used to simulate a
/// transient failure so rerun tests have something to fail on.
struct FlakyServer {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FlakyServer {
    fn always_200() -> Self {
        Self::start(0)
    }

    fn fails_first_n(n: usize) -> Self {
        Self::start(n)
    }

    fn start(initial_failures: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let counter = Arc::new(AtomicUsize::new(0));

        let handle = thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let n = counter.fetch_add(1, Ordering::SeqCst);
                        handle_one(stream, n < initial_failures);
                    }
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

impl Drop for FlakyServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

fn handle_one(mut stream: TcpStream, fail: bool) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut buf = [0u8; 2048];
    let _ = stream.read(&mut buf);
    let body = b"{\"ok\":true}";
    let status_line = if fail {
        "HTTP/1.1 500 Internal Server Error"
    } else {
        "HTTP/1.1 200 OK"
    };
    let response = format!(
        "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn scaffold_project(dir: &Path, base_url: &str) {
    std::fs::write(dir.join("tarn.config.yaml"), "test_dir: tests\n").unwrap();
    std::fs::write(dir.join("tarn.env.yaml"), format!("base_url: {base_url}\n")).unwrap();
    let tests_dir = dir.join("tests");
    std::fs::create_dir_all(&tests_dir).unwrap();
    std::fs::write(
        tests_dir.join("health.tarn.yaml"),
        r#"name: health
tests:
  smoke:
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

#[test]
fn tarn_run_agent_mode_is_default_and_returns_agent_schema() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let server = FlakyServer::always_200();
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    let params = json!({ "cwd": tmp.path().to_string_lossy(), "path": "tests" });
    let resp = tarn_mcp::tools::tarn_run(&params).expect("agent-mode run succeeds");

    // AgentReport envelope identifying fields.
    assert_eq!(
        resp.get("report_mode").and_then(|v| v.as_str()),
        Some("agent")
    );
    let inner = resp.get("report").expect("report block present");
    assert!(
        inner.get("schema_version").is_some(),
        "AgentReport carries schema_version: {inner:#}"
    );
    assert_eq!(inner.get("status").and_then(|v| v.as_str()), Some("passed"));

    // Every artifact path must exist on disk after the run.
    let artifacts = resp.get("artifacts").expect("artifacts block");
    for key in &["report", "summary", "failures", "state", "events"] {
        let p = artifacts.get(*key).and_then(|v| v.as_str()).unwrap();
        assert!(
            std::path::Path::new(p).is_file(),
            "artifact {key} should exist on disk: {p}"
        );
    }
}

#[test]
fn tarn_run_rejects_unknown_report_mode() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), "http://127.0.0.1:1");
    let err = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
        "report_mode": "nope",
    }))
    .expect_err("unknown report_mode must be rejected");
    assert_eq!(err.code, tarn_mcp::tools::ERR_INVALID_PARAM);
    assert!(err.data.is_some(), "error must carry structured data");
}

#[test]
fn tarn_last_failures_returns_empty_groups_on_clean_run() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let server = FlakyServer::always_200();
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    let _ = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    }))
    .expect("run succeeds");

    let resp = tarn_mcp::tools::tarn_last_failures(&json!({
        "cwd": tmp.path().to_string_lossy(),
    }))
    .expect("last_failures on clean run");

    let groups = resp
        .pointer("/failures/groups")
        .and_then(|v| v.as_array())
        .expect("failures.groups array");
    assert!(groups.is_empty(), "clean run should yield zero groups");
    assert!(resp.get("run_id").is_some());
    assert!(resp.get("artifacts").is_some());
}

#[test]
fn tarn_last_failures_groups_failures_on_failing_run() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Server always fails so the initial run has real failures.
    let server = FlakyServer::fails_first_n(usize::MAX);
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    let run = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    }))
    .expect("run returns even on failure");
    assert_eq!(
        run.get("exit_code").and_then(|v| v.as_i64()),
        Some(1),
        "failing assertion should yield exit code 1"
    );

    let resp = tarn_mcp::tools::tarn_last_failures(&json!({
        "cwd": tmp.path().to_string_lossy(),
    }))
    .expect("last_failures after failure");
    let groups = resp
        .pointer("/failures/groups")
        .and_then(|v| v.as_array())
        .expect("failures.groups array");
    assert!(!groups.is_empty(), "failing run must produce groups");
    // Every group carries a fingerprint string per the NAZ-402 contract.
    for g in groups {
        assert!(
            g.get("fingerprint")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .is_some(),
            "group must carry non-empty fingerprint: {g:#}"
        );
    }
}

#[test]
fn tarn_get_run_artifacts_resolves_last_alias_and_explicit_id() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let server = FlakyServer::always_200();
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    let run = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    }))
    .expect("run succeeds");
    let run_id = run
        .get("run_id")
        .and_then(|v| v.as_str())
        .expect("run carries run_id")
        .to_string();

    // Alias `last` resolves to the freshly-produced run.
    let last = tarn_mcp::tools::tarn_get_run_artifacts(&json!({
        "cwd": tmp.path().to_string_lossy(),
    }))
    .expect("last alias resolves");
    assert_eq!(
        last.get("run_id").and_then(|v| v.as_str()),
        Some(run_id.as_str())
    );
    assert_eq!(
        last.pointer("/exists/report").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        last.pointer("/exists/summary").and_then(|v| v.as_bool()),
        Some(true)
    );

    // Explicit id gets the same artifacts.
    let explicit = tarn_mcp::tools::tarn_get_run_artifacts(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "run_id": run_id,
    }))
    .expect("explicit id resolves");
    assert_eq!(
        explicit.get("run_id").and_then(|v| v.as_str()),
        Some(run_id.as_str())
    );
    assert_eq!(
        explicit.get("report_path"),
        last.get("report_path"),
        "explicit id must yield identical paths"
    );
}

#[test]
fn tarn_rerun_failed_produces_new_run_id_and_fresh_artifacts() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Fail the first request, succeed on every one after that. The
    // initial `tarn_run` therefore fails; the subsequent
    // `tarn_rerun_failed` hits the same endpoint once and passes. We
    // verify the rerun wrote a brand-new run directory distinct from
    // the source.
    let server = FlakyServer::fails_first_n(1);
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    let initial = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    }))
    .expect("run succeeds (even when failing internally)");
    let initial_id = initial
        .get("run_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    assert_eq!(
        initial.get("exit_code").and_then(|v| v.as_i64()),
        Some(1),
        "initial run must have failed"
    );

    let rerun = tarn_mcp::tools::tarn_rerun_failed(&json!({
        "cwd": tmp.path().to_string_lossy(),
    }))
    .expect("rerun_failed runs");
    let rerun_id = rerun
        .get("run_id")
        .and_then(|v| v.as_str())
        .expect("rerun carries its own run_id")
        .to_string();

    assert_ne!(
        initial_id, rerun_id,
        "rerun must mint a fresh run_id rather than reuse the source"
    );
    // The rerun run directory must exist on disk.
    let rerun_dir = rerun
        .pointer("/artifacts/run_dir")
        .and_then(|v| v.as_str())
        .expect("rerun artifacts include run_dir");
    assert!(
        std::path::Path::new(rerun_dir).is_dir(),
        "rerun run directory must exist on disk: {rerun_dir}"
    );
    // Provenance: the rerun response echoes the source run that seeded it.
    assert!(
        rerun.get("rerun_source").is_some(),
        "rerun response should carry rerun_source: {rerun:#}"
    );
}

#[test]
fn tarn_rerun_failed_errors_when_no_failures_on_record() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let server = FlakyServer::always_200();
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    // Clean run populates `failures.json` with an empty list.
    let _ = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    }))
    .unwrap();

    let err = tarn_mcp::tools::tarn_rerun_failed(&json!({
        "cwd": tmp.path().to_string_lossy(),
    }))
    .expect_err("empty failures.json should surface a structured error");
    assert_eq!(err.code, tarn_mcp::tools::ERR_RERUN_EMPTY);
    assert!(
        err.data.is_some(),
        "rerun empty error must carry structured data"
    );
}

#[test]
fn tarn_report_reads_concise_envelope_from_disk() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let server = FlakyServer::always_200();
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    let _ = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    }))
    .unwrap();

    let resp = tarn_mcp::tools::tarn_report(&json!({
        "cwd": tmp.path().to_string_lossy(),
    }))
    .expect("concise report reads");
    assert_eq!(
        resp.pointer("/report/exit_code").and_then(|v| v.as_i64()),
        Some(0)
    );
    assert!(resp.get("artifacts").is_some());
}

#[test]
fn tarn_inspect_returns_run_level_view_by_default() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let server = FlakyServer::always_200();
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    let _ = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    }))
    .unwrap();

    let resp = tarn_mcp::tools::tarn_inspect(&json!({
        "cwd": tmp.path().to_string_lossy(),
    }))
    .expect("inspect resolves `last`");
    let view = resp.get("view").expect("view block");
    assert_eq!(
        view.get("target").and_then(|v| v.as_str()),
        Some("run"),
        "default target is run-level"
    );
}

#[test]
fn malformed_cwd_returns_structured_error_payload() {
    let err = tarn_mcp::tools::tarn_last_failures(&json!({ "cwd": "not/absolute" }))
        .expect_err("relative cwd rejected");
    assert_eq!(err.code, tarn_mcp::tools::ERR_INVALID_CWD);
    let data = err.data.clone().expect("structured data present");
    // Structured data names the offending input so agents don't need
    // to parse the message.
    assert!(data.get("cwd").is_some());

    let json = err.to_tool_call_json();
    let obj = json.as_object().expect("tool-call payload is an object");
    // The triple must always be present on the transport surface.
    assert!(obj.contains_key("code"));
    assert!(obj.contains_key("message"));
    assert!(obj.contains_key("data"));
}

/// Parity witness: an MCP run invoked from process cwd inside the
/// scaffolded project picks up the same `base_url` from `tarn.env.yaml`
/// as an explicit-cwd run invoked from *outside* that project. This is
/// the core NAZ-407 requirement — env-file resolution must behave the
/// same regardless of how the agent expressed the working directory.
#[test]
fn env_resolution_parity_between_process_cwd_and_explicit_cwd() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let server = FlakyServer::always_200();
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    // 1. Explicit cwd from *outside* the project.
    let outside = tempfile::TempDir::new().unwrap();
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(outside.path()).unwrap();
    let explicit = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
        "report_mode": "summary",
    }));
    // 2. Process cwd = the project, no explicit cwd.
    std::env::set_current_dir(tmp.path()).unwrap();
    let process_cwd = tarn_mcp::tools::tarn_run(&json!({
        "path": "tests",
        "report_mode": "summary",
    }));
    std::env::set_current_dir(prev).unwrap();

    let explicit = explicit.expect("explicit-cwd run succeeds");
    let process_cwd = process_cwd.expect("process-cwd run succeeds");

    // Both runs must have passed — the `{{ env.base_url }}` token has
    // to resolve to the FlakyServer address in both. If env resolution
    // silently fell back to the process cwd in the first case, the
    // template would have exploded.
    assert_eq!(explicit.get("exit_code").and_then(|v| v.as_i64()), Some(0));
    assert_eq!(
        process_cwd.get("exit_code").and_then(|v| v.as_i64()),
        Some(0)
    );
}
