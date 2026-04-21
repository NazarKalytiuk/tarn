//! Immutable per-run artifact directories under
//! `<workspace-root>/.tarn/runs/<run_id>/`.
//!
//! Every Tarn run is assigned a stable `run_id` derived from its start
//! timestamp plus a short random suffix, and its artifacts (the full
//! JSON report, the condensed `state.json`, and any follow-up files
//! from sibling tickets) are written into that directory. The old
//! `.tarn/last-run.json` and `.tarn/state.json` paths stay as copies
//! of the most recent run, so tooling that already reads those paths
//! keeps working.
//!
//! The goal is to make debugging context durable: a second run does
//! not destroy the artifacts from the previous run, so users can
//! compare runs, open a prior run after the fact, or share a run id
//! with an agent.

use chrono::{DateTime, Utc};
use rand::RngCore;
use std::io;
use std::path::{Path, PathBuf};

/// Build a stable run identifier from the run's start timestamp and a
/// short random suffix. Format: `YYYYmmdd-HHMMSS-xxxxxx` where
/// `xxxxxx` is 6 lowercase hex characters.
///
/// The timestamp prefix keeps directory listings chronologically
/// sortable; the random suffix prevents collisions when two runs
/// start inside the same second (e.g. watch mode firing repeatedly).
pub fn generate_run_id(started_at: DateTime<Utc>) -> String {
    let mut rng = rand::rng();
    let mut bytes = [0u8; 3];
    rng.fill_bytes(&mut bytes);
    format!(
        "{}-{:02x}{:02x}{:02x}",
        started_at.format("%Y%m%d-%H%M%S"),
        bytes[0],
        bytes[1],
        bytes[2]
    )
}

/// Return `<workspace_root>/.tarn/runs/<run_id>/` without creating it.
pub fn run_directory(workspace_root: &Path, run_id: &str) -> PathBuf {
    workspace_root.join(".tarn").join("runs").join(run_id)
}

/// Create `<workspace_root>/.tarn/runs/<run_id>/` (and parents) and
/// return its absolute path.
pub fn ensure_run_directory(workspace_root: &Path, run_id: &str) -> io::Result<PathBuf> {
    let dir = run_directory(workspace_root, run_id);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Resolve a user-supplied run identifier alias to a concrete `run_id`
/// that matches a directory under `<workspace_root>/.tarn/runs/`.
///
/// Supported aliases:
/// - `last` / `latest` / `@latest` → the most recent archive
///   (lexically greatest `run_id`; the timestamp prefix keeps lexical
///   and chronological order aligned).
/// - `prev` → the archive immediately preceding the latest.
/// - Any other string → treated as a literal `run_id`; returned
///   verbatim only when the corresponding directory exists.
///
/// Returns `io::Error` with `NotFound` when the alias cannot be
/// satisfied (no archives on disk, alias mismatch, unknown id). The
/// caller is expected to map that to exit code 2.
pub fn resolve_run_id(workspace_root: &Path, alias: &str) -> io::Result<String> {
    let lower = alias.to_ascii_lowercase();
    match lower.as_str() {
        "last" | "latest" | "@latest" => nth_run_id_from_end(workspace_root, 0),
        "prev" | "previous" => nth_run_id_from_end(workspace_root, 1),
        _ => {
            let dir = run_directory(workspace_root, alias);
            if dir.is_dir() {
                Ok(alias.to_string())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "unknown run id '{}' (no directory at {})",
                        alias,
                        dir.display()
                    ),
                ))
            }
        }
    }
}

/// Return the N-th most recent `run_id` under `.tarn/runs/` (0 = latest).
/// Names are sorted lexicographically descending; since `run_id` carries
/// a `YYYYmmdd-HHMMSS-xxxxxx` prefix, lexical order equals chronological
/// order.
fn nth_run_id_from_end(workspace_root: &Path, offset: usize) -> io::Result<String> {
    let runs_dir = workspace_root.join(".tarn").join("runs");
    if !runs_dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no run archives at {}", runs_dir.display()),
        ));
    }
    let mut names: Vec<String> = std::fs::read_dir(&runs_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names.reverse();
    names.into_iter().nth(offset).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no run at offset {} under {} ({} runs available)",
                offset,
                runs_dir.display(),
                offset
            ),
        )
    })
}

/// Copy a freshly-written run artifact (e.g. `report.json`) to a
/// pointer path (e.g. `.tarn/last-run.json`) so legacy consumers that
/// read the pointer path keep working. The pointer is overwritten
/// atomically via write-then-rename so a reader never sees a
/// half-populated file.
pub fn copy_to_pointer(src: &Path, pointer: &Path) -> io::Result<()> {
    if !src.exists() {
        return Ok(());
    }
    if let Some(parent) = pointer.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = std::fs::read(src)?;
    let tmp = pointer.with_extension("tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, pointer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn generate_run_id_has_timestamp_and_suffix() {
        let started = Utc::now();
        let id = generate_run_id(started);
        // Format: YYYYmmdd-HHMMSS-xxxxxx -> 8+1+6+1+6 = 22 chars
        assert_eq!(id.len(), 22, "unexpected run_id format: {}", id);
        assert!(id.starts_with(&started.format("%Y%m%d-%H%M%S").to_string()));
    }

    #[test]
    fn generate_run_id_is_unique_under_same_second() {
        let started = Utc::now();
        let a = generate_run_id(started);
        let b = generate_run_id(started);
        assert_ne!(a, b);
    }

    #[test]
    fn run_directory_path_is_under_workspace_tarn_runs() {
        let path = run_directory(Path::new("/tmp/ws"), "20260101-000000-aabbcc");
        assert_eq!(
            path,
            PathBuf::from("/tmp/ws/.tarn/runs/20260101-000000-aabbcc")
        );
    }

    #[test]
    fn ensure_run_directory_creates_nested_path() {
        let tmp = TempDir::new().unwrap();
        let dir = ensure_run_directory(tmp.path(), "20260101-000000-abcdef").unwrap();
        assert!(dir.is_dir());
        assert!(dir.ends_with(".tarn/runs/20260101-000000-abcdef"));
    }

    #[test]
    fn copy_to_pointer_overwrites_existing_file_atomically() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("report.json");
        let pointer = tmp.path().join(".tarn").join("last-run.json");
        std::fs::write(&src, b"v2").unwrap();
        std::fs::create_dir_all(pointer.parent().unwrap()).unwrap();
        std::fs::write(&pointer, b"v1").unwrap();
        copy_to_pointer(&src, &pointer).unwrap();
        assert_eq!(std::fs::read(&pointer).unwrap(), b"v2");
        assert!(
            !pointer.with_extension("tmp").exists(),
            "tmp must be renamed away"
        );
    }

    #[test]
    fn resolve_run_id_last_returns_lexically_greatest_archive() {
        let tmp = TempDir::new().unwrap();
        for id in ["20260101-000000-aaaaaa", "20260102-000000-bbbbbb"] {
            ensure_run_directory(tmp.path(), id).unwrap();
        }
        let resolved = resolve_run_id(tmp.path(), "last").unwrap();
        assert_eq!(resolved, "20260102-000000-bbbbbb");
        assert_eq!(resolve_run_id(tmp.path(), "latest").unwrap(), resolved);
        assert_eq!(resolve_run_id(tmp.path(), "@latest").unwrap(), resolved);
    }

    #[test]
    fn resolve_run_id_prev_returns_one_before_latest() {
        let tmp = TempDir::new().unwrap();
        for id in [
            "20260101-000000-aaaaaa",
            "20260102-000000-bbbbbb",
            "20260103-000000-cccccc",
        ] {
            ensure_run_directory(tmp.path(), id).unwrap();
        }
        assert_eq!(
            resolve_run_id(tmp.path(), "prev").unwrap(),
            "20260102-000000-bbbbbb"
        );
    }

    #[test]
    fn resolve_run_id_bare_id_returns_verbatim_when_archive_exists() {
        let tmp = TempDir::new().unwrap();
        ensure_run_directory(tmp.path(), "20260101-000000-abcdef").unwrap();
        assert_eq!(
            resolve_run_id(tmp.path(), "20260101-000000-abcdef").unwrap(),
            "20260101-000000-abcdef"
        );
    }

    #[test]
    fn resolve_run_id_unknown_id_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        ensure_run_directory(tmp.path(), "20260101-000000-abcdef").unwrap();
        let err = resolve_run_id(tmp.path(), "nope").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn resolve_run_id_last_without_any_archives_errors() {
        let tmp = TempDir::new().unwrap();
        let err = resolve_run_id(tmp.path(), "last").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn resolve_run_id_prev_with_only_one_archive_errors() {
        let tmp = TempDir::new().unwrap();
        ensure_run_directory(tmp.path(), "20260101-000000-abcdef").unwrap();
        let err = resolve_run_id(tmp.path(), "prev").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn copy_to_pointer_noop_when_source_missing() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("missing.json");
        let pointer = tmp.path().join(".tarn").join("last-run.json");
        copy_to_pointer(&src, &pointer).unwrap();
        assert!(!pointer.exists());
    }
}
