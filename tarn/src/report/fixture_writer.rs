//! Per-step fixture writer used by the runner after every HTTP round
//! trip. Companion to [`crate::fixtures`], which owns the on-disk path
//! layout this module writes into.
//!
//! The writer is opt-in from the runner side — callers pass
//! [`FixtureWriteConfig`] into [`RunOptions`](crate::runner::RunOptions).
//! When fixtures are disabled the helper functions in this module are
//! never called.
//!
//! ## Redaction
//!
//! Every string field (URLs, header values, JSON payloads) is routed
//! through the existing redaction helpers in
//! [`crate::report::redaction`] so secrets recorded in the regular
//! JSON report are redacted the same way inside fixture files. We
//! pick up both file-level redaction rules (`redaction:` block) and
//! dynamically-harvested secret values (env vars, capture values that
//! were added to the file's redaction watch list).
//!
//! ## Retention
//!
//! [`write_step_fixture`] prunes older history entries after every
//! write so the per-step directory never holds more than
//! `config.retention` rolling-history files (on top of
//! `latest-passed.json` and `_index.json`). Pruning is filesystem-
//! best-effort: a transient I/O failure is logged to stderr but does
//! not fail the run.

use crate::assert::types::{RequestInfo, ResponseInfo, StepResult};
use crate::fixtures::{latest_passed_path, step_fixture_dir, INDEX_FILENAME};
use crate::model::RedactionConfig;
use crate::report::redaction::{redact_headers, sanitize_json, sanitize_string};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Options controlling the fixture writer behaviour.
///
/// Passed through `RunOptions` so both the sequential and the parallel
/// runners read the same settings. When `enabled` is false the runner
/// skips every fixture-write call entirely.
#[derive(Debug, Clone)]
pub struct FixtureWriteConfig {
    /// Master switch. The CLI flips this off via `--no-fixtures`.
    pub enabled: bool,
    /// Absolute path to the workspace root the fixtures are written
    /// under. The runner resolves this via `load_project_context`
    /// before invoking `run_file_*`; LSP clients resolve it from the
    /// workspace folder URI.
    pub workspace_root: PathBuf,
    /// How many rolling-history fixtures to keep per step, on top of
    /// `latest-passed.json`. The CLI overrides this via
    /// `--fixture-retention`.
    pub retention: usize,
}

impl Default for FixtureWriteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            workspace_root: PathBuf::from("."),
            retention: crate::fixtures::DEFAULT_RETENTION,
        }
    }
}

/// Per-step fixture payload. Serialised into the step directory as
/// `<millis>-<counter>.json` plus (for a passing step) a copy at
/// `latest-passed.json`.
///
/// Field order is stable — the LSP reads these files back and tests
/// pin on the envelope shape. Added fields must be optional (via
/// `#[serde(default, skip_serializing_if = "Option::is_none")]`) so
/// older readers continue to parse newer writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    /// RFC3339 / ISO 8601 timestamp (UTC) the fixture was captured.
    pub recorded_at: String,
    /// Redacted request payload. Mirrors the shape of the JSON
    /// report's `request` object.
    pub request: FixtureRequest,
    /// Redacted response payload, or `null` when the step never
    /// produced one (connection error, unresolved template, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<FixtureResponse>,
    /// Captures produced by the step, keyed by capture name. Empty
    /// when the step declared no captures or never got far enough to
    /// extract them.
    #[serde(default)]
    pub captures: serde_json::Map<String, Value>,
    /// Whether the step passed every assertion.
    pub passed: bool,
    /// Optional human-readable failure message. Populated when
    /// `passed` is false; the LSP uses it for the fallback message in
    /// `tarn.explainFailure`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_message: Option<String>,
    /// Duration in milliseconds the runner recorded for the step.
    #[serde(default)]
    pub duration_ms: u64,
}

/// Subset of [`RequestInfo`] captured inside the fixture. We drop the
/// `multipart` block because the fixture is JSON and multipart file
/// references are not round-trippable across machines anyway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureRequest {
    pub method: String,
    pub url: String,
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

/// Subset of [`ResponseInfo`] captured inside the fixture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureResponse {
    pub status: u16,
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

/// Rolling-history index — a small JSON manifest kept alongside the
/// fixture files so readers can enumerate the history in chronological
/// order without racing with the writer's directory scan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FixtureIndex {
    /// Filenames (relative to the step directory), oldest first.
    pub history: Vec<String>,
}

/// Build a [`Fixture`] from the runner's per-step state, applying
/// redaction in the same shape the JSON report already uses.
pub fn build_fixture(
    result: &StepResult,
    redaction: &RedactionConfig,
    secret_values: &[String],
) -> Fixture {
    let (request, response) = if let Some(req) = result.request_info.as_ref() {
        (
            redact_request(req, redaction, secret_values),
            result
                .response_info
                .as_ref()
                .map(|r| redact_response(r, redaction, secret_values)),
        )
    } else {
        (empty_request(), None)
    };

    // Carry the captures produced by the step. The runner-side
    // `captures_set` is just the key list; the full value map lives
    // on the enclosing `TestResult`, so we fetch the strings here and
    // sanitise them individually in `build_fixture_captures`.
    let captures = serde_json::Map::new();

    let failure_message = result
        .assertion_results
        .iter()
        .find(|a| !a.passed)
        .map(|a| sanitize_string(&a.message, &redaction.replacement, secret_values));

    Fixture {
        recorded_at: Utc::now().to_rfc3339(),
        request,
        response,
        captures,
        passed: result.passed,
        failure_message,
        duration_ms: result.duration_ms,
    }
}

/// Populate `fixture.captures` from the runner's shared map, taking
/// only the keys the step actually set and redacting string-type
/// values that overlap with the current secret list.
pub fn attach_captures(
    fixture: &mut Fixture,
    captures: &HashMap<String, Value>,
    captures_set: &[String],
    redaction: &RedactionConfig,
    secret_values: &[String],
) {
    for name in captures_set {
        if let Some(value) = captures.get(name) {
            let sanitised = sanitize_json(value, &redaction.replacement, secret_values);
            fixture.captures.insert(name.clone(), sanitised);
        }
    }
}

fn empty_request() -> FixtureRequest {
    FixtureRequest {
        method: String::new(),
        url: String::new(),
        headers: std::collections::BTreeMap::new(),
        body: None,
    }
}

fn redact_request(
    request: &RequestInfo,
    redaction: &RedactionConfig,
    secret_values: &[String],
) -> FixtureRequest {
    FixtureRequest {
        method: request.method.clone(),
        url: sanitize_string(&request.url, &redaction.replacement, secret_values),
        headers: redact_headers(&request.headers, redaction, secret_values),
        body: request
            .body
            .as_ref()
            .map(|b| sanitize_json(b, &redaction.replacement, secret_values)),
    }
}

fn redact_response(
    response: &ResponseInfo,
    redaction: &RedactionConfig,
    secret_values: &[String],
) -> FixtureResponse {
    FixtureResponse {
        status: response.status,
        headers: redact_headers(&response.headers, redaction, secret_values),
        body: response
            .body
            .as_ref()
            .map(|b| sanitize_json(b, &redaction.replacement, secret_values)),
    }
}

/// Atomic write helper: write to `<path>.tmp`, fsync, rename.
///
/// `fs::rename` is atomic on every POSIX filesystem Tarn runs on and
/// works the same way on Windows (NTFS) for same-volume renames. We
/// use it for every fixture-related write so a crash mid-run can only
/// leave a `.tmp` file, never a partial fixture.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Persist a fixture for a single step.
///
/// Returns `Ok(())` on success, or the first filesystem error
/// encountered. The runner treats write failures as soft errors
/// (logged to stderr, run continues) because a missing fixture never
/// blocks a pass/fail result.
pub fn write_step_fixture(
    config: &FixtureWriteConfig,
    file_path: &Path,
    test: &str,
    step_index: usize,
    fixture: &Fixture,
) -> std::io::Result<()> {
    let dir = step_fixture_dir(&config.workspace_root, file_path, test, step_index);
    std::fs::create_dir_all(&dir)?;

    // Encode the millisecond timestamp + a per-directory counter into
    // the filename so simultaneous writes from parallel test runs do
    // not collide. The counter comes from the existing history length
    // (monotonic within a process; across processes we fall back to
    // the timestamp).
    let filename = next_history_filename(&dir);
    let target = dir.join(&filename);
    let encoded = serde_json::to_vec_pretty(fixture)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    atomic_write(&target, &encoded)?;

    // Update the manifest and prune older history before writing
    // `latest-passed.json` so a reader that opens the index after a
    // successful run sees a consistent view.
    let mut index = read_index(&dir);
    index.history.push(filename);
    let retention = config.retention.max(1);
    while index.history.len() > retention {
        let oldest = index.history.remove(0);
        let p = dir.join(&oldest);
        let _ = std::fs::remove_file(&p);
    }
    write_index(&dir, &index)?;

    if fixture.passed {
        let latest = latest_passed_path(&config.workspace_root, file_path, test, step_index);
        atomic_write(&latest, &encoded)?;
    }

    Ok(())
}

/// Compute the next history filename for a step directory.
///
/// Uses the UTC millisecond timestamp so lexicographic sort equals
/// chronological order. A 6-character random suffix prevents
/// collisions when two parallel runs land inside the same millisecond.
fn next_history_filename(_dir: &Path) -> String {
    let millis = Utc::now().timestamp_millis();
    // A 32-bit random suffix keeps filenames short and unique.
    let suffix: u32 = rand::random();
    format!("{:013}-{:08x}.json", millis, suffix)
}

fn read_index(dir: &Path) -> FixtureIndex {
    let path = dir.join(INDEX_FILENAME);
    let Ok(bytes) = std::fs::read(&path) else {
        return FixtureIndex::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn write_index(dir: &Path, index: &FixtureIndex) -> std::io::Result<()> {
    let path = dir.join(INDEX_FILENAME);
    let encoded = serde_json::to_vec_pretty(index)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    atomic_write(&path, &encoded)
}

/// Clear every fixture under a workspace, or under a single test file
/// path. Used by the LSP's `tarn.clearFixtures` command.
///
/// `file_path == None` removes `<root>/.tarn/fixtures/` entirely.
/// `file_path == Some(path)` removes the subtree for that file only.
pub fn clear_fixtures(root: &Path, file_path: Option<&Path>) -> std::io::Result<()> {
    let target = match file_path {
        Some(file) => {
            use crate::fixtures::file_path_hash;
            root.join(".tarn")
                .join("fixtures")
                .join(file_path_hash(root, file))
        }
        None => root.join(".tarn").join("fixtures"),
    };
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    Ok(())
}

/// Read the most recent fixture on file for a step. Preference order:
///
/// 1. `latest-passed.json` — always returned when present. This is the
///    most useful signal for the LSP: the last known-good response.
/// 2. The newest entry in `_index.json`, regardless of pass/fail.
///
/// Returns `None` when neither is present or when neither parses as
/// JSON. Every I/O and parse failure collapses to `None` so the LSP
/// reader always gracefully degrades.
pub fn read_latest_fixture(
    root: &Path,
    file_path: &Path,
    test: &str,
    step_index: usize,
) -> Option<Fixture> {
    if let Some(passed) = read_fixture_file(&latest_passed_path(root, file_path, test, step_index))
    {
        return Some(passed);
    }
    let dir = step_fixture_dir(root, file_path, test, step_index);
    let index = read_index(&dir);
    let newest = index.history.last()?;
    read_fixture_file(&dir.join(newest))
}

/// Read the most recent fixture as a raw [`serde_json::Value`]. Used
/// by the LSP's JSONPath evaluator to reuse the existing
/// `RecordedResponseSource` contract without teaching it about the
/// strongly-typed [`Fixture`].
pub fn read_latest_response_value(
    root: &Path,
    file_path: &Path,
    test: &str,
    step_index: usize,
) -> Option<Value> {
    let fx = read_latest_fixture(root, file_path, test, step_index)?;
    fx.response.and_then(|r| r.body)
}

fn read_fixture_file(path: &Path) -> Option<Fixture> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<Fixture>(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::{AssertionResult, RequestInfo, ResponseInfo};
    use crate::fixtures::{file_path_hash, slugify_name, LATEST_PASSED_FILENAME};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn step_result_ok() -> StepResult {
        StepResult {
            name: "create user".into(),
            passed: true,
            duration_ms: 42,
            assertion_results: vec![AssertionResult::pass("status", "200", "200")],
            request_info: Some(RequestInfo {
                method: "POST".into(),
                url: "http://x.test/users".into(),
                headers: HashMap::from([("X-Trace".into(), "leaky-secret".into())]),
                body: Some(serde_json::json!({"name": "leaky-secret"})),
                multipart: None,
            }),
            response_info: Some(ResponseInfo {
                status: 200,
                headers: HashMap::from([("content-type".into(), "application/json".into())]),
                body: Some(serde_json::json!({"id": 7, "token": "leaky-secret"})),
            }),
            error_category: None,
            response_status: Some(200),
            response_summary: Some("200 OK".into()),
            captures_set: vec!["user_id".into()],
            location: None,
        }
    }

    #[test]
    fn build_fixture_redacts_strings_in_request_and_response() {
        let sr = step_result_ok();
        let redaction = RedactionConfig::default();
        let fx = build_fixture(&sr, &redaction, &["leaky-secret".into()]);

        assert!(fx.passed);
        let rep = &redaction.replacement;
        assert_eq!(fx.request.headers.get("X-Trace"), Some(rep));
        let body = fx.request.body.as_ref().unwrap();
        assert_eq!(body["name"], serde_json::Value::String(rep.clone()));
        let resp = fx.response.as_ref().unwrap();
        let resp_body = resp.body.as_ref().unwrap();
        assert_eq!(resp_body["token"], serde_json::Value::String(rep.clone()));
    }

    #[test]
    fn attach_captures_drops_unknown_names_and_redacts_values() {
        let sr = step_result_ok();
        let redaction = RedactionConfig::default();
        let mut fx = build_fixture(&sr, &redaction, &["leaky-secret".into()]);
        let mut all_captures = HashMap::new();
        all_captures.insert("user_id".to_string(), serde_json::json!(7));
        all_captures.insert(
            "other".to_string(),
            serde_json::json!("should-not-appear"),
        );
        attach_captures(
            &mut fx,
            &all_captures,
            &["user_id".into()],
            &redaction,
            &[],
        );
        assert_eq!(fx.captures.get("user_id"), Some(&serde_json::json!(7)));
        assert!(!fx.captures.contains_key("other"));
    }

    #[test]
    fn write_step_fixture_creates_directory_tree_and_latest_passed() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file = root.join("tests/users.tarn.yaml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "name: fixture\n").unwrap();

        let sr = step_result_ok();
        let fx = build_fixture(&sr, &RedactionConfig::default(), &[]);
        let config = FixtureWriteConfig {
            enabled: true,
            workspace_root: root.clone(),
            retention: 5,
        };

        write_step_fixture(&config, &file, "create user", 0, &fx).unwrap();

        let expected_dir = root
            .join(".tarn/fixtures")
            .join(file_path_hash(&root, &file))
            .join(slugify_name("create user"))
            .join("0");
        assert!(expected_dir.is_dir());
        let latest = expected_dir.join("latest-passed.json");
        assert!(latest.is_file(), "latest-passed missing");
        let manifest: FixtureIndex =
            serde_json::from_slice(&std::fs::read(expected_dir.join("_index.json")).unwrap())
                .unwrap();
        assert_eq!(manifest.history.len(), 1);
    }

    #[test]
    fn write_step_fixture_prunes_history_to_retention_cap() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file = root.join("tests/users.tarn.yaml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "name: x\n").unwrap();

        let sr = step_result_ok();
        let fx = build_fixture(&sr, &RedactionConfig::default(), &[]);
        let config = FixtureWriteConfig {
            enabled: true,
            workspace_root: root.clone(),
            retention: 3,
        };

        for _ in 0..7 {
            // Guarantee distinct filenames even on very fast CI runners
            // where many writes can happen within one millisecond.
            std::thread::sleep(std::time::Duration::from_millis(2));
            write_step_fixture(&config, &file, "t", 0, &fx).unwrap();
        }

        let dir = step_fixture_dir(&root, &file, "t", 0);
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| {
                n != LATEST_PASSED_FILENAME && n != INDEX_FILENAME && !n.ends_with(".tmp")
            })
            .collect();
        assert_eq!(
            entries.len(),
            3,
            "retention cap of 3 should keep exactly 3 rolling files, got {:?}",
            entries
        );
    }

    #[test]
    fn read_latest_fixture_prefers_latest_passed() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file = root.join("tests/users.tarn.yaml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "name: x\n").unwrap();

        let mut fx_pass = build_fixture(&step_result_ok(), &RedactionConfig::default(), &[]);
        fx_pass.passed = true;
        let config = FixtureWriteConfig {
            enabled: true,
            workspace_root: root.clone(),
            retention: 3,
        };
        write_step_fixture(&config, &file, "t", 0, &fx_pass).unwrap();

        // Overwrite with a failure fixture. The rolling history now
        // ends with a failure, but `latest-passed.json` still holds
        // the earlier passing run, so the reader should prefer the
        // passing fixture.
        let mut failing = fx_pass.clone();
        failing.passed = false;
        failing.failure_message = Some("boom".into());
        failing.recorded_at = "2026-04-17T00:00:00Z".into();
        write_step_fixture(&config, &file, "t", 0, &failing).unwrap();

        let read = read_latest_fixture(&root, &file, "t", 0).expect("fixture");
        assert!(read.passed, "should prefer latest-passed.json");
    }

    #[test]
    fn clear_fixtures_removes_entire_subtree() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file = root.join("tests/users.tarn.yaml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "name: x\n").unwrap();

        let fx = build_fixture(&step_result_ok(), &RedactionConfig::default(), &[]);
        let config = FixtureWriteConfig {
            enabled: true,
            workspace_root: root.clone(),
            retention: 3,
        };
        write_step_fixture(&config, &file, "t", 0, &fx).unwrap();

        let base = root.join(".tarn/fixtures");
        assert!(base.exists());
        clear_fixtures(&root, None).unwrap();
        assert!(!base.exists());
    }

    #[test]
    fn clear_fixtures_can_scope_to_single_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let file_a = root.join("tests/a.tarn.yaml");
        let file_b = root.join("tests/b.tarn.yaml");
        std::fs::create_dir_all(file_a.parent().unwrap()).unwrap();
        std::fs::write(&file_a, "name: a\n").unwrap();
        std::fs::write(&file_b, "name: b\n").unwrap();

        let fx = build_fixture(&step_result_ok(), &RedactionConfig::default(), &[]);
        let config = FixtureWriteConfig {
            enabled: true,
            workspace_root: root.clone(),
            retention: 3,
        };
        write_step_fixture(&config, &file_a, "t", 0, &fx).unwrap();
        write_step_fixture(&config, &file_b, "t", 0, &fx).unwrap();

        clear_fixtures(&root, Some(&file_a)).unwrap();

        let dir_a = root
            .join(".tarn/fixtures")
            .join(file_path_hash(&root, &file_a));
        let dir_b = root
            .join(".tarn/fixtures")
            .join(file_path_hash(&root, &file_b));
        assert!(!dir_a.exists(), "file_a fixtures should be removed");
        assert!(dir_b.exists(), "file_b fixtures should remain");
    }
}
