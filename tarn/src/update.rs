use std::fs;
use std::path::PathBuf;

const REPO_OWNER: &str = "NazarKalytiuk";
const REPO_NAME: &str = "hive";

/// Information about an available update.
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub is_newer: bool,
    pub download_url: Option<String>,
}

/// Check GitHub releases for the latest version.
pub fn check_for_update() -> Result<UpdateInfo, String> {
    let current = env!("CARGO_PKG_VERSION");
    let target = get_target()?;

    let client = reqwest::blocking::Client::builder()
        .user_agent("tarn-updater")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        REPO_OWNER, REPO_NAME
    );
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("Failed to check for updates: {}", e))?;

    if resp.status().as_u16() == 404 {
        return Err("No releases found".to_string());
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub API returned status {}", resp.status()));
    }

    let release: serde_json::Value = resp
        .json()
        .map_err(|e| format!("Failed to parse release info: {}", e))?;

    let tag = release["tag_name"]
        .as_str()
        .ok_or("No tag_name in release")?;
    let latest_version = tag.strip_prefix('v').unwrap_or(tag).to_string();

    let is_newer = version_is_newer(current, &latest_version);

    // Find the download URL for our platform
    let asset_name = format!("tarn-{}.tar.gz", target);
    let download_url = release["assets"].as_array().and_then(|assets| {
        assets
            .iter()
            .find(|a| a["name"].as_str() == Some(asset_name.as_str()))
            .and_then(|a| a["browser_download_url"].as_str())
            .map(|s| s.to_string())
    });

    Ok(UpdateInfo {
        current_version: current.to_string(),
        latest_version,
        is_newer,
        download_url,
    })
}

/// Download and install the latest version, replacing the current binary.
pub fn perform_update(info: &UpdateInfo) -> Result<(), String> {
    let url = info
        .download_url
        .as_ref()
        .ok_or("No download URL available for your platform")?;

    let target = get_target()?;

    eprintln!("Downloading tarn v{}...", info.latest_version);
    let client = reqwest::blocking::Client::builder()
        .user_agent("tarn-updater")
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("Download failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Download returned status {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .map_err(|e| format!("Failed to read download: {}", e))?;

    eprintln!("Extracting...");

    // Extract tar.gz to temp directory
    let tmpdir = std::env::temp_dir().join("tarn-update");
    let _ = fs::remove_dir_all(&tmpdir);
    fs::create_dir_all(&tmpdir).map_err(|e| format!("Failed to create temp dir: {}", e))?;

    let decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(decoder);

    let expected_names = [format!("tarn-{}", target), "tarn".to_string()];
    let mut extracted_path: Option<PathBuf> = None;

    for entry in archive
        .entries()
        .map_err(|e| format!("Failed to read archive: {}", e))?
    {
        let mut entry = entry.map_err(|e| format!("Archive entry error: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| format!("Path error: {}", e))?
            .to_path_buf();

        let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");

        if expected_names.iter().any(|n| n == filename) {
            let dest = tmpdir.join("tarn");
            entry
                .unpack(&dest)
                .map_err(|e| format!("Failed to extract binary: {}", e))?;
            extracted_path = Some(dest);
            break;
        }
    }

    let new_binary =
        extracted_path.ok_or("Binary not found in archive. Is this the correct release?")?;

    // Set executable permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&new_binary)
            .map_err(|e| format!("Failed to read permissions: {}", e))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&new_binary, perms)
            .map_err(|e| format!("Failed to set permissions: {}", e))?;
    }

    // Replace current binary
    let current_exe = std::env::current_exe()
        .map_err(|e| format!("Cannot determine current executable: {}", e))?;

    eprintln!("Installing to {}...", current_exe.display());

    let backup = current_exe.with_extension("bak");
    fs::rename(&current_exe, &backup).map_err(|e| {
        format!(
            "Failed to backup current binary: {}. Try running with sudo.",
            e
        )
    })?;

    if let Err(e) = fs::rename(&new_binary, &current_exe) {
        // Restore backup on failure
        let _ = fs::rename(&backup, &current_exe);
        let _ = fs::remove_dir_all(&tmpdir);
        return Err(format!(
            "Failed to install new binary: {}. Try running with sudo.",
            e
        ));
    }

    // Cleanup
    let _ = fs::remove_file(&backup);
    let _ = fs::remove_dir_all(&tmpdir);

    Ok(())
}

/// Get the platform target string (e.g., "darwin-arm64", "linux-amd64").
fn get_target() -> Result<String, String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        other => return Err(format!("Unsupported OS: {}", other)),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => return Err(format!("Unsupported architecture: {}", other)),
    };
    Ok(format!("{}-{}", os, arch))
}

/// Compare two semver strings. Returns true if `latest` is newer than `current`.
fn version_is_newer(current: &str, latest: &str) -> bool {
    let parse = |v: &str| -> (u32, u32, u32) {
        let parts: Vec<u32> = v.split('.').filter_map(|p| p.parse().ok()).collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };
    parse(latest) > parse(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison_newer() {
        assert!(version_is_newer("0.1.0", "0.2.0"));
        assert!(version_is_newer("0.1.0", "1.0.0"));
        assert!(version_is_newer("1.2.3", "1.2.4"));
        assert!(version_is_newer("0.9.9", "1.0.0"));
    }

    #[test]
    fn version_comparison_same() {
        assert!(!version_is_newer("0.1.0", "0.1.0"));
        assert!(!version_is_newer("1.0.0", "1.0.0"));
    }

    #[test]
    fn version_comparison_older() {
        assert!(!version_is_newer("0.2.0", "0.1.0"));
        assert!(!version_is_newer("1.0.0", "0.9.0"));
    }

    #[test]
    fn target_string_format() {
        let target = get_target().unwrap();
        assert!(
            target.contains('-'),
            "Target should be os-arch format, got: {}",
            target
        );
        // Should be one of the known targets
        let valid = ["darwin-arm64", "darwin-amd64", "linux-arm64", "linux-amd64"];
        assert!(
            valid.contains(&target.as_str()),
            "Unknown target: {}",
            target
        );
    }
}
