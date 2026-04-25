//! Cross-platform path string helpers.

use std::path::Path;

/// Render `path` with forward-slash separators on every platform.
///
/// Use this for path strings that go into JSON or other machine-readable
/// artifacts, so the artifact bytes are identical between Linux/macOS
/// (which already use `/`) and Windows (where `Path::display()` would
/// emit `\`). Drive letters and UNC prefixes survive — only the separator
/// changes (`C:\foo\bar` → `C:/foo/bar`, `\\srv\sh\f` → `//srv/sh/f`).
pub fn to_forward_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn unix_path_passes_through_unchanged() {
        assert_eq!(
            to_forward_slash(&PathBuf::from("/tmp/.tarn/runs/abc/failures.json")),
            "/tmp/.tarn/runs/abc/failures.json"
        );
    }

    #[test]
    fn relative_path_uses_forward_slashes() {
        let raw = format!(".tarn{}runs{}abc{}failures.json", "\\", "\\", "\\");
        let p = PathBuf::from(&raw);
        assert_eq!(to_forward_slash(&p), ".tarn/runs/abc/failures.json");
    }

    #[test]
    fn windows_drive_path_keeps_drive_letter() {
        let p = PathBuf::from("C:\\Users\\runner\\.tarn\\last-run.json");
        assert_eq!(to_forward_slash(&p), "C:/Users/runner/.tarn/last-run.json");
    }
}
