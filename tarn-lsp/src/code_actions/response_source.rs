//! Pluggable recorded-response source for the **scaffold-assert**
//! code action (NAZ-304, Phase L3.3) and every downstream consumer
//! that needs to read the last recorded response for a step.
//!
//! ## On-disk convention (NAZ-252)
//!
//! Fixtures are written by `tarn`'s runner into a workspace-relative
//! directory tree:
//!
//! ```text
//! <workspace-root>/.tarn/fixtures/<file-path-hash>/<test-slug>/<step-index>/
//!   latest-passed.json     # the most recent passing fixture
//!   <millis>-<nonce>.json  # rolling history (latest N per step)
//!   _index.json            # history manifest, oldest-first
//! ```
//!
//! The `[latest-passed.json]` file is always the LSP's first lookup
//! target because it represents a known-good response; only when it
//! is absent does the reader fall back to the newest history entry.
//! Setup / teardown steps use the sentinel test slugs exported from
//! [`tarn::fixtures`] (`setup` / `teardown` / `<flat>`).
//!
//! ## Back-compat with the L3.3 sidecar
//!
//! Existing tests / fixtures generated during Phase L3.3 still live
//! at `<file>.tarn.yaml.last-run/<test-slug>/<step-slug>.response.json`.
//! The disk reader consults that layout last so integration tests
//! that pre-dated NAZ-252 keep passing without modification.
//!
//! ## Why a trait
//!
//! The `RecordedResponseSource` trait exists so the pure
//! `scaffold_assert_code_action` renderer can be unit-tested with a
//! synthetic in-memory implementation that returns a pre-baked JSON
//! value regardless of disk layout. Tests never touch `/tmp` and
//! never care about the slug rules — the trait is a hermetic seam
//! that keeps the renderer pure.

use std::path::{Path, PathBuf};

/// One-method trait used by the **scaffold-assert** code action to
/// look up the most recent recorded response for a step.
///
/// Implementations must be cheap to call (the LSP dispatcher invokes
/// them synchronously) and must never panic — return `None` for every
/// failure mode, including I/O errors, permission problems, invalid
/// JSON, or simply "no recording yet".
pub trait RecordedResponseSource: Send + Sync {
    /// Look up the last recorded response for `(file, test, step)`.
    ///
    /// `file` is the absolute filesystem path of the `.tarn.yaml`
    /// buffer the LSP is serving the code action for. `test` is the
    /// display name of the enclosing test group, or the sentinel
    /// string `"setup"` / `"teardown"` for top-level setup/teardown
    /// steps. `step` is the step's `name:` value.
    ///
    /// Returns `None` when nothing is on record or when the recording
    /// cannot be decoded as JSON. The code action falls back to "not
    /// offered" in that case.
    fn read(&self, file: &Path, test: &str, step: &str) -> Option<serde_json::Value>;
}

/// Default on-disk implementation of [`RecordedResponseSource`].
///
/// Checks, in order:
///   1. `<workspace-root>/.tarn/fixtures/...` (NAZ-252 layout) via
///      [`tarn::report::fixture_writer::read_latest_response_value`].
///      The step index is resolved by parsing `file` — when that
///      fails the lookup gracefully falls through to the sidecar.
///   2. `<file>.last-run/<test-slug>/<step-slug>.response.json`
///      (legacy L3.3 sidecar) — still consulted so existing
///      fixtures keep working after the layout change.
///
/// Every failure mode folds to `None` so LSP callers always see the
/// same "nothing recorded yet" signal regardless of the root cause.
pub struct DiskResponseSource;

impl RecordedResponseSource for DiskResponseSource {
    fn read(&self, file: &Path, test: &str, step: &str) -> Option<serde_json::Value> {
        // NAZ-252 layout. The workspace root is the closest ancestor
        // that holds a `.tarn/` directory — if none, fall back to
        // the file's own parent. This keeps the reader usable from
        // tests that drive it against an absolute tempdir.
        if let Some(root) = workspace_root_for(file) {
            if let Some(index) = resolve_step_index(file, test, step) {
                if let Some(value) = tarn::report::fixture_writer::read_latest_response_value(
                    &root, file, test, index,
                ) {
                    return Some(value);
                }
            }
        }

        let path = sidecar_path(file, test, step);
        let bytes = std::fs::read(&path).ok()?;
        serde_json::from_slice::<serde_json::Value>(&bytes).ok()
    }
}

/// Walk ancestors of `file` looking for a `.tarn` directory, which
/// we treat as the workspace root marker. Falls back to the file's
/// parent so callers that point the reader at a tempdir without a
/// `.tarn` subfolder still get useful behaviour.
///
/// Exposed at `pub(crate)` so sibling modules (`crate::fixtures`,
/// `crate::explain_failure`) reuse the same resolution heuristic —
/// changing it in one place should be enough to change it
/// everywhere.
pub(crate) fn workspace_root_for(file: &Path) -> Option<std::path::PathBuf> {
    let mut current = file.parent()?;
    loop {
        if current.join(".tarn").is_dir() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

/// Map `(test, step-name)` into the numeric step index used by the
/// fixture layout. Returns `None` when the file cannot be parsed or
/// the step is not found in the named test / setup / teardown /
/// flat-steps scope.
fn resolve_step_index(file: &Path, test: &str, step: &str) -> Option<usize> {
    let source = std::fs::read_to_string(file).ok()?;
    let parsed = tarn::parser::parse_str(&source, file).ok()?;
    let steps = match test {
        "setup" => &parsed.setup,
        "teardown" => &parsed.teardown,
        "<flat>" => &parsed.steps,
        name => parsed
            .tests
            .get(name)
            .map(|g| &g.steps)
            .unwrap_or(&parsed.steps),
    };
    steps.iter().position(|s| s.name == step)
}

/// Compute the sidecar path for a given (file, test, step) triple.
///
/// Public so tests (and future CLI-side writers) can anchor against
/// the same layout the reader expects. Relative paths are supported
/// for test fixtures — the function does not canonicalise.
pub fn sidecar_path(file: &Path, test: &str, step: &str) -> PathBuf {
    let mut buf = file.as_os_str().to_owned();
    buf.push(".last-run");
    let mut p = PathBuf::from(buf);
    p.push(slug(test));
    p.push(format!("{}.response.json", slug(step)));
    p
}

/// URL-safe slug derived from a step or test display name. Lower-
/// cased, whitespace-to-hyphen, everything outside `[a-z0-9_-]`
/// stripped. An empty / all-garbage name collapses to `_`.
pub fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_hyphen = false;
    for c in name.chars() {
        let lower = c.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() || lower == '_' {
            out.push(lower);
            prev_hyphen = false;
        } else if (lower == '-' || lower.is_whitespace()) && !prev_hyphen && !out.is_empty() {
            out.push('-');
            prev_hyphen = true;
        }
        // Everything else (punctuation, symbols) is silently dropped.
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        return "_".to_owned();
    }
    out
}

/// In-memory mock used only by tests. Always returns the pre-baked
/// JSON value regardless of arguments, which is exactly the seam we
/// need to keep the scaffold-assert renderer hermetic.
///
/// Guarded behind a public constructor so integration tests in
/// `tests/code_actions_test.rs` can instantiate it across the crate
/// boundary while production code has no reason to reach for it.
pub struct InMemoryResponseSource {
    value: Option<serde_json::Value>,
}

impl InMemoryResponseSource {
    /// Wrap a pre-baked value that every `read` call returns.
    pub fn new(value: serde_json::Value) -> Self {
        Self { value: Some(value) }
    }

    /// Wrap `None` — every `read` call reports "no recording", the
    /// same degradation path the disk reader uses for a missing
    /// sidecar file. Used by unit tests for the "reader present but
    /// empty" branch of the scaffold-assert renderer.
    pub fn empty() -> Self {
        Self { value: None }
    }
}

impl RecordedResponseSource for InMemoryResponseSource {
    fn read(&self, _file: &Path, _test: &str, _step: &str) -> Option<serde_json::Value> {
        self.value.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn slug_lowercases_and_hyphenates_whitespace() {
        assert_eq!(slug("Create User"), "create-user");
        assert_eq!(slug("  leading and trailing  "), "leading-and-trailing");
    }

    #[test]
    fn slug_drops_punctuation_and_collapses_hyphens() {
        assert_eq!(slug("POST /users/:id"), "post-usersid");
        assert_eq!(slug("a--b"), "a-b");
    }

    #[test]
    fn slug_empty_name_collapses_to_underscore() {
        assert_eq!(slug(""), "_");
        assert_eq!(slug("!!!"), "_");
    }

    #[test]
    fn sidecar_path_appends_last_run_suffix() {
        let p = sidecar_path(
            Path::new("/tmp/tests/users.tarn.yaml"),
            "create_user",
            "POST /users",
        );
        let s = p.to_string_lossy();
        assert!(s.ends_with("users.tarn.yaml.last-run/create_user/post-users.response.json"));
    }

    #[test]
    fn disk_response_source_round_trip_reads_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("case.tarn.yaml");
        std::fs::write(&file, "name: fixture\n").unwrap();
        let dir = tmp
            .path()
            .join("case.tarn.yaml.last-run")
            .join("main")
            .join("");
        std::fs::create_dir_all(&dir).unwrap();
        let sidecar = tmp
            .path()
            .join("case.tarn.yaml.last-run")
            .join("main")
            .join("step-a.response.json");
        let mut f = std::fs::File::create(&sidecar).unwrap();
        f.write_all(br#"{"id":1,"name":"x"}"#).unwrap();

        let src = DiskResponseSource;
        let got = src.read(&file, "main", "step a").expect("value");
        assert_eq!(got["id"], serde_json::json!(1));
        assert_eq!(got["name"], serde_json::json!("x"));
    }

    #[test]
    fn disk_response_source_missing_file_returns_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("missing.tarn.yaml");
        let src = DiskResponseSource;
        assert!(src.read(&file, "main", "step").is_none());
    }

    #[test]
    fn in_memory_response_source_returns_preset_value() {
        let value = serde_json::json!({"x": 1});
        let src = InMemoryResponseSource::new(value.clone());
        let got = src
            .read(Path::new("/tmp/x.tarn.yaml"), "t", "s")
            .expect("value");
        assert_eq!(got, value);
    }

    #[test]
    fn in_memory_response_source_empty_returns_none() {
        let src = InMemoryResponseSource::empty();
        assert!(src.read(Path::new("/tmp/x.tarn.yaml"), "t", "s").is_none());
    }
}
