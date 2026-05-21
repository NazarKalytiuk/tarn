use crate::error::{AppError, AppResult};
use std::path::PathBuf;

/// Resolves the path to the `tarn` binary.
///
/// Lookup order (first match wins):
/// 1. `TARN_BIN` environment variable (absolute path).
/// 2. The first `tarn` executable found on `PATH`.
/// 3. The workspace `target/release/tarn`, relative to `CARGO_MANIFEST_DIR`.
/// 4. The workspace `target/debug/tarn`, relative to `CARGO_MANIFEST_DIR`.
///
/// In Phase 4 this is replaced by Tauri's bundled sidecar resolution
/// (`tauri.conf.json > bundle > externalBin`), but for development the
/// workspace target tree is the canonical source.
pub fn resolve_tarn() -> AppResult<PathBuf> {
    if let Ok(env_path) = std::env::var("TARN_BIN") {
        let path = PathBuf::from(env_path);
        if path.is_file() {
            return Ok(path);
        }
    }

    if let Ok(path) = which("tarn") {
        return Ok(path);
    }

    let workspace_root = workspace_root();
    for profile in ["release", "debug"] {
        let candidate = workspace_root
            .join("target")
            .join(profile)
            .join(binary_name("tarn"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(AppError::BinaryNotFound)
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points at tarn-desktop/src-tauri/. The workspace
    // root is two levels up.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .unwrap_or(manifest_dir)
}

fn binary_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// Cross-platform `which` without pulling in the `which` crate. Walks
/// `PATH` directly, checking each entry for an executable file with the
/// expected name.
fn which(stem: &str) -> AppResult<PathBuf> {
    let name = binary_name(stem);
    let path_env =
        std::env::var_os("PATH").ok_or_else(|| AppError::Other("PATH not set".into()))?;
    for dir in std::env::split_paths(&path_env) {
        let candidate = dir.join(&name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(AppError::BinaryNotFound)
}
