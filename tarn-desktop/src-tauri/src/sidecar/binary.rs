use crate::error::{AppError, AppResult};
use std::path::PathBuf;

/// Resolves the path to the `tarn` binary.
///
/// Lookup order (first match wins):
/// 1. `TARN_BIN` environment variable (absolute path). Explicit user
///    override — always wins.
/// 2. **Adjacent to the current executable.** This is where Tauri's
///    `bundle.externalBin` lands a packaged sidecar: on macOS that's
///    `<App>.app/Contents/MacOS/tarn`, on Windows `<App>/tarn.exe`,
///    on Linux `<App>/tarn`. Checking this before `PATH` means a
///    user's globally-installed `tarn` cannot accidentally shadow
///    the version the app was built and tested with.
/// 3. The first `tarn` executable found on `PATH`. Convenient for
///    developer setups where Studio runs unbundled.
/// 4. The CLI workspace `target/release` or `target/debug` —
///    relative to `CARGO_MANIFEST_DIR`. Useful when running Studio
///    via `cargo tauri dev` from the CLI repo.
///
/// Phase 4 will likely replace step 4 with Tauri's
/// `app_handle.path().resolve(...)` so the resolver works without
/// any compile-time path assumptions, but the current design keeps
/// the function free of a Tauri AppHandle for testability.
pub fn resolve_tarn() -> AppResult<PathBuf> {
    if let Ok(env_path) = std::env::var("TARN_BIN") {
        let path = PathBuf::from(env_path);
        if path.is_file() {
            return Ok(path);
        }
    }

    if let Some(adjacent) = adjacent_to_current_exe() {
        if adjacent.is_file() {
            return Ok(adjacent);
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

fn adjacent_to_current_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join(binary_name("tarn")))
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points at tarn-desktop/src-tauri/. The CLI
    // workspace root we want to probe is two levels up from there.
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
