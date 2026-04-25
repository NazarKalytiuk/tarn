use std::fs;
use zed_extension_api::{
    self as zed, serde_json, settings::LspSettings, Architecture, Command, DownloadedFileType,
    Extension, GithubReleaseOptions, LanguageServerId, LanguageServerInstallationStatus, Os,
    Result, Worktree,
};

const GITHUB_REPO: &str = "NazarKalytiuk/tarn";
const SERVER_BINARY: &str = "tarn-lsp";

struct TarnExtension {
    cached_binary_path: Option<String>,
}

impl TarnExtension {
    fn resolve_binary(
        &mut self,
        server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<String> {
        let settings = LspSettings::for_worktree(server_id.as_ref(), worktree).ok();
        if let Some(path) = settings
            .as_ref()
            .and_then(|s| s.binary.as_ref())
            .and_then(|b| b.path.clone())
        {
            return Ok(path);
        }

        if let Some(path) = worktree.which(SERVER_BINARY) {
            return Ok(path);
        }

        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).is_ok_and(|stat| stat.is_file()) {
                return Ok(path.clone());
            }
        }

        self.download_from_github(server_id)
    }

    fn download_from_github(&mut self, server_id: &LanguageServerId) -> Result<String> {
        zed::set_language_server_installation_status(
            server_id,
            &LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let release = zed::latest_github_release(
            GITHUB_REPO,
            GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let (platform, arch) = zed::current_platform();
        let (asset_name, archive_type) = asset_for(platform, arch)?;

        let asset = release
            .assets
            .iter()
            .find(|a| a.name == asset_name)
            .ok_or_else(|| {
                format!(
                    "tarn-lsp: no release asset named '{asset_name}' in {}",
                    release.version
                )
            })?;

        let version_dir = format!("tarn-lsp-{}", release.version);
        let binary_name = match platform {
            Os::Windows => "tarn-lsp.exe",
            _ => "tarn-lsp",
        };
        let binary_path = format!("{version_dir}/{binary_name}");

        if !fs::metadata(&binary_path).is_ok_and(|stat| stat.is_file()) {
            zed::set_language_server_installation_status(
                server_id,
                &LanguageServerInstallationStatus::Downloading,
            );

            zed::download_file(&asset.download_url, &version_dir, archive_type)
                .map_err(|e| format!("tarn-lsp: failed to download {}: {e}", asset.download_url))?;

            zed::make_file_executable(&binary_path).ok();

            if let Ok(entries) = fs::read_dir(".") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let keep = name
                        .to_str()
                        .is_some_and(|n| n == version_dir || !n.starts_with("tarn-lsp-"));
                    if !keep {
                        fs::remove_dir_all(entry.path()).ok();
                    }
                }
            }
        }

        self.cached_binary_path = Some(binary_path.clone());
        Ok(binary_path)
    }
}

impl Extension for TarnExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Command> {
        let settings = LspSettings::for_worktree(server_id.as_ref(), worktree).ok();
        let args = settings
            .as_ref()
            .and_then(|s| s.binary.as_ref())
            .and_then(|b| b.arguments.clone())
            .unwrap_or_default();

        Ok(Command {
            command: self.resolve_binary(server_id, worktree)?,
            args,
            env: Default::default(),
        })
    }

    fn language_server_workspace_configuration(
        &mut self,
        server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<serde_json::Value>> {
        let settings = LspSettings::for_worktree(server_id.as_ref(), worktree)
            .ok()
            .and_then(|s| s.settings.clone());

        Ok(settings.map(|s| serde_json::json!({ "tarn": s })))
    }
}

fn asset_for(platform: Os, arch: Architecture) -> Result<(String, DownloadedFileType)> {
    let (os_slug, arch_slug, ext, archive) = match (platform, arch) {
        (Os::Linux, Architecture::X8664) => {
            ("linux", "amd64", "tar.gz", DownloadedFileType::GzipTar)
        }
        (Os::Linux, Architecture::Aarch64) => {
            ("linux", "arm64", "tar.gz", DownloadedFileType::GzipTar)
        }
        (Os::Mac, Architecture::X8664) => {
            ("darwin", "amd64", "tar.gz", DownloadedFileType::GzipTar)
        }
        (Os::Mac, Architecture::Aarch64) => {
            ("darwin", "arm64", "tar.gz", DownloadedFileType::GzipTar)
        }
        (Os::Windows, Architecture::X8664) => ("windows", "amd64", "zip", DownloadedFileType::Zip),
        (os, arch) => {
            return Err(format!(
                "tarn-lsp: unsupported platform {os:?}/{arch:?} — install `tarn-lsp` manually (cargo install tarn-lsp) and set lsp.tarn-lsp.binary.path"
            ));
        }
    };

    Ok((format!("hive-{os_slug}-{arch_slug}.{ext}"), archive))
}

zed::register_extension!(TarnExtension);
