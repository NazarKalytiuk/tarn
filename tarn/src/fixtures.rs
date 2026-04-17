//! Per-step fixture store shared between the runner (writer) and
//! tarn-lsp (reader).
//!
//! The fixture store is a tree of JSON documents under
//! `<workspace-root>/.tarn/fixtures/` that records the most recent
//! request/response/captures for every step in every test file. The
//! runner writes one fixture per executed step; the LSP reads them
//! back through `tarn.getFixture`, `tarn.evaluateJsonpath`, and the
//! hover/code-action providers.
//!
//! ## Layout
//!
//! ```text
//! <root>/.tarn/fixtures/
//!   <file-path-hash>/
//!     <test-slug>/
//!       <step-index>-<nonce>.json   # rolling history, at most N
//!       latest-passed.json           # copy of the most recent passed run
//!       _index.json                  # ordered list of history filenames
//! ```
//!
//! * `<file-path-hash>` — first 16 hex characters of the SHA-256 of
//!   the test file's path **relative to the workspace root**. Keeps
//!   paths filesystem-safe regardless of how nested the test sits.
//! * `<test-slug>` — URL-safe slug of the test name
//!   (`slugify_name`). Setup / teardown / simple-format steps use the
//!   sentinels `setup` / `teardown` / `<flat>`.
//! * `<step-index>` — the 0-based index within the enclosing step
//!   list (setup, test, teardown, or flat).
//! * History files are stamped with the millisecond timestamp in the
//!   filename so a later run always sorts after an earlier one even
//!   when a user clock rewinds by less than a millisecond.
//! * `latest-passed.json` is the most recent *passing* fixture so the
//!   LSP can fall back to a known-good baseline when the most recent
//!   fixture is a failure the user is debugging.
//!
//! Both writer and reader import `fixture_dir`, `slugify_name`, and
//! `file_path_hash` from this module so the on-disk layout is
//! authoritative here and nowhere else.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Retention default used when the CLI does not pass
/// `--fixture-retention`. Chosen to match the ticket's "keep the last
/// five per step" rule; small enough that a typical CI run stays well
/// under a megabyte per step even for large JSON bodies.
pub const DEFAULT_RETENTION: usize = 5;

/// Sentinel test slug for top-level `setup:` steps that have no
/// enclosing test group. Mirrors the sidecar convention shipped with
/// NAZ-304 so the LSP reader and the new writer resolve to the same
/// directory.
pub const SETUP_TEST_SLUG: &str = "setup";

/// Sentinel test slug for top-level `teardown:` steps.
pub const TEARDOWN_TEST_SLUG: &str = "teardown";

/// Sentinel test slug for simple-format (flat) steps that live outside
/// any `tests:` mapping. Angle brackets make it visually distinct from
/// user-supplied test names and legally filesystem-safe.
pub const FLAT_TEST_SLUG: &str = "<flat>";

/// Filename used for the most recent passing fixture — a stable entry
/// the LSP can always name without consulting `_index.json`.
pub const LATEST_PASSED_FILENAME: &str = "latest-passed.json";

/// Filename used for the rolling-history index inside a step dir.
pub const INDEX_FILENAME: &str = "_index.json";

/// Return the first 16 hex characters of the SHA-256 of the file's
/// path **relative** to the workspace root.
///
/// If `file_path` is absolute we try to strip `root` first; if the
/// strip fails we hash the absolute path verbatim so two workspaces
/// that legitimately store the same relative filename under different
/// roots still get distinct fixture trees.
pub fn file_path_hash(root: &Path, file_path: &Path) -> String {
    let relative = file_path.strip_prefix(root).unwrap_or(file_path);
    // Canonicalise the separator so the hash is stable across
    // Windows/Unix. `Path::display()` already yields forward slashes
    // on Windows except when a component contains `\`, so we normalise
    // by iterating components.
    let mut canonical = String::new();
    for comp in relative.components() {
        if !canonical.is_empty() {
            canonical.push('/');
        }
        canonical.push_str(&comp.as_os_str().to_string_lossy());
    }
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

/// URL-safe slug derived from a user-supplied name (test or step).
///
/// Rules: ASCII-lowercase, runs of non-alphanumeric (other than `_`)
/// are collapsed to a single `-`, leading/trailing `-` stripped. The
/// empty and all-garbage cases collapse to `_`. We keep `_` because
/// upstream tests already rely on underscore-style names.
pub fn slugify_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_hyphen = false;
    for c in name.chars() {
        let lower = c.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() || lower == '_' {
            out.push(lower);
            prev_hyphen = false;
        } else if !prev_hyphen && !out.is_empty() {
            out.push('-');
            prev_hyphen = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        return "_".to_owned();
    }
    out
}

/// Directory that holds every fixture recorded for a single
/// `(file, test)` pair.
///
/// The caller is responsible for creating the directory before
/// writing into it; the reader returns `None` when the directory does
/// not exist.
pub fn test_fixture_dir(root: &Path, file_path: &Path, test: &str) -> PathBuf {
    root.join(".tarn")
        .join("fixtures")
        .join(file_path_hash(root, file_path))
        .join(slugify_name(test))
}

/// Directory that holds every fixture recorded for a single
/// `(file, test, step-index)` triple. All history rolls inside this
/// directory so retention pruning is a directory-local operation.
pub fn step_fixture_dir(root: &Path, file_path: &Path, test: &str, step_index: usize) -> PathBuf {
    test_fixture_dir(root, file_path, test).join(step_index.to_string())
}

/// Absolute path to the canonical "most recent passing" fixture for a
/// step. Read-only consumers (LSP, docs, replay) should try this
/// before walking the rolling history — it is the only stable name
/// that survives every retention cycle.
pub fn latest_passed_path(
    root: &Path,
    file_path: &Path,
    test: &str,
    step_index: usize,
) -> PathBuf {
    step_fixture_dir(root, file_path, test, step_index).join(LATEST_PASSED_FILENAME)
}

/// Absolute path to the rolling-history index file.
pub fn index_path(root: &Path, file_path: &Path, test: &str, step_index: usize) -> PathBuf {
    step_fixture_dir(root, file_path, test, step_index).join(INDEX_FILENAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_name_lowercases_and_hyphenates() {
        assert_eq!(slugify_name("Create User"), "create-user");
        assert_eq!(slugify_name("POST /users/:id"), "post-users-id");
    }

    #[test]
    fn slugify_name_keeps_underscores_and_digits() {
        assert_eq!(slugify_name("list_2"), "list_2");
    }

    #[test]
    fn slugify_name_empty_or_punctuation_collapses_to_underscore() {
        assert_eq!(slugify_name(""), "_");
        assert_eq!(slugify_name("!!!"), "_");
    }

    #[test]
    fn file_path_hash_is_deterministic_and_relative() {
        let root = Path::new("/tmp/workspace");
        let a = root.join("tests").join("users.tarn.yaml");
        let b = Path::new("/elsewhere/tests/users.tarn.yaml");
        let hash_a = file_path_hash(root, &a);
        let hash_b = file_path_hash(root, b);
        // Different roots must produce different hashes when one can't
        // be stripped — otherwise two unrelated suites clobber each
        // other's fixtures.
        assert_ne!(hash_a, hash_b);
        assert_eq!(hash_a.len(), 16);
    }

    #[test]
    fn file_path_hash_matches_relative_input() {
        let root = Path::new("/tmp/workspace");
        let absolute = root.join("tests").join("users.tarn.yaml");
        let relative = Path::new("tests/users.tarn.yaml");
        assert_eq!(
            file_path_hash(root, &absolute),
            file_path_hash(root, relative)
        );
    }

    #[test]
    fn step_fixture_dir_layers_under_dot_tarn() {
        let root = Path::new("/tmp/workspace");
        let file = root.join("tests/users.tarn.yaml");
        let dir = step_fixture_dir(root, &file, "create user", 2);
        let display = dir.to_string_lossy().into_owned();
        assert!(
            display.contains(".tarn/fixtures/"),
            "expected .tarn/fixtures/ path, got {display}"
        );
        assert!(display.ends_with("/create-user/2"));
    }

    #[test]
    fn latest_passed_path_points_inside_step_dir() {
        let root = Path::new("/tmp/workspace");
        let file = root.join("tests/users.tarn.yaml");
        let latest = latest_passed_path(root, &file, "create user", 0);
        assert!(latest
            .to_string_lossy()
            .ends_with("/create-user/0/latest-passed.json"));
    }
}
