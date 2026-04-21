//! Integration tests for the NAZ-416 high-level MCP tools.
//!
//! These exercise the new `tarn_impact`, `tarn_scaffold`, `tarn_run_agent`,
//! `tarn_last_root_causes`, and `tarn_pack_context` handlers against
//! either pure-library invocations or a real (local) HTTP server so we
//! can prove the whole "inner loop" — impact → scaffold → run → inspect
//! root causes → pack context — works end-to-end through MCP alone.

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

// NAZ-407's integration harness flips process cwd. We hold the same lock
// here for two reasons: (1) the cwd-defaulting branches in these tools go
// through the same `resolve_cwd` fallback as the artifact tools, and
// (2) `tarn_run_agent` writes `.tarn/runs/<id>/` directories under the
// resolved workspace root so interleaved runs would race on the filesystem.
static CWD_LOCK: Mutex<()> = Mutex::new(());

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

// Minimal scaffolded project mirroring the one NAZ-407's tests use so the
// inner-loop parity holds across both test suites.
fn scaffold_project(dir: &Path, base_url: &str) {
    std::fs::write(dir.join("tarn.config.yaml"), "test_dir: tests\n").unwrap();
    std::fs::write(dir.join("tarn.env.yaml"), format!("base_url: {base_url}\n")).unwrap();
    let tests_dir = dir.join("tests");
    std::fs::create_dir_all(&tests_dir).unwrap();
    std::fs::write(
        tests_dir.join("users.tarn.yaml"),
        r#"name: users
tests:
  get_user:
    steps:
      - name: GET /users/42
        request:
          method: GET
          url: "{{ env.base_url }}/users/42"
        assert:
          status: 200
"#,
    )
    .unwrap();
}

#[test]
fn tarn_impact_endpoint_match_is_high_confidence() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), "http://127.0.0.1:1");

    // Arrange: a `GET /users/:id` endpoint change should land on the
    // `get_user` test above at high confidence.
    let resp = tarn_mcp::tools::tarn_impact(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "endpoints": [{ "method": "GET", "path": "/users/:id" }],
    }))
    .expect("impact returns");

    // Assert: at least one high-confidence match cites the endpoint.
    let matches = resp
        .get("matches")
        .and_then(|v| v.as_array())
        .expect("matches array present");
    assert!(!matches.is_empty(), "expected at least one match");
    let top = &matches[0];
    assert_eq!(
        top.get("confidence").and_then(|v| v.as_str()),
        Some("high"),
        "top match should be high-confidence: {top:#}"
    );
    assert_eq!(resp.get("schema_version").and_then(|v| v.as_u64()), Some(1));
    // `cwd` echoes the resolved workspace root for traceability.
    assert!(resp.get("cwd").is_some());
}

#[test]
fn tarn_impact_missing_inputs_returns_structured_error() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), "http://127.0.0.1:1");

    // No `diff`, `files`, `endpoints`, or `openapi_ops` at all — the
    // tool must refuse rather than return an empty match list, because
    // that would mask a misconfigured caller.
    let err = tarn_mcp::tools::tarn_impact(&json!({
        "cwd": tmp.path().to_string_lossy(),
    }))
    .expect_err("missing inputs must error");
    assert_eq!(err.code, tarn_mcp::tools::ERR_IMPACT_INVALID_INPUT);
    let data = err.data.expect("error carries structured data");
    assert!(data.get("hint").is_some(), "hint must be present");
}

#[test]
fn tarn_scaffold_explicit_writes_parseable_yaml() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("tarn.config.yaml"), "test_dir: tests\n").unwrap();

    let out_path = tmp.path().join("tests").join("new.tarn.yaml");
    let resp = tarn_mcp::tools::tarn_scaffold(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "mode": "explicit",
        "explicit": { "method": "GET", "url": "http://127.0.0.1:1/widgets" },
        "out": out_path.to_string_lossy(),
    }))
    .expect("scaffold succeeds");

    // Response carries the structural fields the ticket calls out.
    assert_eq!(resp.get("schema_version").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        resp.get("source_mode").and_then(|v| v.as_str()),
        Some("explicit")
    );
    assert_eq!(
        resp.get("written_to").and_then(|v| v.as_str()),
        Some(out_path.to_string_lossy().as_ref())
    );

    // The written YAML must round-trip through the real parser — the
    // strongest guarantee that scaffold's output is executable.
    let on_disk = std::fs::read_to_string(&out_path).expect("written file");
    assert!(on_disk.contains("method: GET"));
    tarn::parser::parse_str(&on_disk, &out_path).expect("scaffolded YAML parses");
}

#[test]
fn tarn_scaffold_missing_mode_errors() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("tarn.config.yaml"), "test_dir: tests\n").unwrap();

    let err = tarn_mcp::tools::tarn_scaffold(&json!({
        "cwd": tmp.path().to_string_lossy(),
    }))
    .expect_err("missing mode must error");
    assert_eq!(err.code, tarn_mcp::tools::ERR_SCAFFOLD_INVALID_INPUT);
    assert!(err.data.is_some(), "structured data carries param name");
}

#[test]
fn tarn_run_agent_returns_agent_report_with_artifacts() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Fail every request so the run produces a root cause the agent
    // report will enumerate.
    let server = FlakyServer::fails_first_n(usize::MAX);
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    let resp = tarn_mcp::tools::tarn_run_agent(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    }))
    .expect("run_agent succeeds on failing suite");

    // AgentReport envelope fields.
    assert_eq!(
        resp.get("report_mode").and_then(|v| v.as_str()),
        Some("agent")
    );
    let report = resp.get("report").expect("report block present");
    assert_eq!(
        report.get("status").and_then(|v| v.as_str()),
        Some("failed")
    );
    // At least one root cause must be surfaced — that is the whole
    // point of the agent view.
    let root_causes = report
        .get("root_causes")
        .and_then(|v| v.as_array())
        .expect("root_causes array");
    assert!(
        !root_causes.is_empty(),
        "failing run must yield root causes"
    );

    // Every artifact file must exist after the run.
    let artifacts = resp.get("artifacts").expect("artifacts block");
    for key in &["report", "summary", "failures", "state", "events"] {
        let p = artifacts
            .get(*key)
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("artifact path missing: {key}"));
        assert!(
            std::path::Path::new(p).is_file(),
            "artifact {key} should exist on disk: {p}"
        );
    }
}

#[test]
fn tarn_last_root_causes_returns_empty_on_passing_run() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let server = FlakyServer::always_200();
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    // Arrange: clean run first, so `failures.json` is present but empty.
    let run = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    }))
    .expect("run succeeds");
    assert_eq!(run.get("exit_code").and_then(|v| v.as_i64()), Some(0));

    let resp = tarn_mcp::tools::tarn_last_root_causes(&json!({
        "cwd": tmp.path().to_string_lossy(),
    }))
    .expect("last_root_causes on clean run");

    assert_eq!(resp.get("total_failures").and_then(|v| v.as_u64()), Some(0));
    let groups = resp
        .get("groups")
        .and_then(|v| v.as_array())
        .expect("groups array");
    assert!(groups.is_empty(), "clean run must yield zero groups");
    assert_eq!(resp.get("schema_version").and_then(|v| v.as_u64()), Some(1));
}

#[test]
fn tarn_last_root_causes_groups_fingerprinted_failures() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Every request fails so the grouping has real input.
    let server = FlakyServer::fails_first_n(usize::MAX);
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    let _ = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    }))
    .expect("run returns even on failure");

    let resp = tarn_mcp::tools::tarn_last_root_causes(&json!({
        "cwd": tmp.path().to_string_lossy(),
    }))
    .expect("root causes after failure");

    let groups = resp
        .get("groups")
        .and_then(|v| v.as_array())
        .expect("groups array");
    assert!(!groups.is_empty(), "failing run must produce groups");
    // Every group carries a fingerprint per the NAZ-402 contract —
    // that is what makes the grouping stable.
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
fn tarn_pack_context_json_bundle_has_failing_yaml_snippet() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let server = FlakyServer::fails_first_n(usize::MAX);
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());

    // Arrange: failing run populates the whole artifact triad
    // (summary + failures + report + state) the bundle reads from.
    let _ = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    }))
    .unwrap();

    let resp = tarn_mcp::tools::tarn_pack_context(&json!({
        "cwd": tmp.path().to_string_lossy(),
    }))
    .expect("pack-context on failing run");

    assert_eq!(resp.get("schema_version").and_then(|v| v.as_u64()), Some(1));
    // The JSON branch carries a structured `bundle` — we assert the
    // bundle exposes at least one entry with a populated YAML snippet
    // pointing back at the failing step.
    let bundle = resp.get("bundle").expect("bundle present");
    let entries = bundle
        .get("entries")
        .and_then(|v| v.as_array())
        .expect("entries array");
    assert!(!entries.is_empty(), "failing run must pack entries");
    let with_snippet = entries.iter().find(|e| {
        e.get("yaml_snippet")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    });
    assert!(
        with_snippet.is_some(),
        "at least one failing entry must carry yaml_snippet: {bundle:#}"
    );
}

#[test]
fn tarn_pack_context_markdown_format_returns_string_body() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let server = FlakyServer::fails_first_n(usize::MAX);
    let tmp = tempfile::TempDir::new().unwrap();
    scaffold_project(tmp.path(), &server.base_url());
    let _ = tarn_mcp::tools::tarn_run(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "path": "tests",
    }))
    .unwrap();

    let resp = tarn_mcp::tools::tarn_pack_context(&json!({
        "cwd": tmp.path().to_string_lossy(),
        "format": "markdown",
    }))
    .expect("markdown pack-context");
    let md = resp
        .get("markdown")
        .and_then(|v| v.as_str())
        .expect("markdown payload");
    // Sanity: the markdown carries at least one code fence (step excerpt).
    assert!(
        md.contains("```"),
        "markdown output should include fenced blocks"
    );
}
