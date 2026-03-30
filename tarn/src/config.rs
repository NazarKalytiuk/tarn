use crate::error::TarnError;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Project-level configuration from tarn.config.yaml.
#[derive(Debug, Deserialize, Clone)]
pub struct TarnConfig {
    /// Directory to scan for .tarn.yaml test files
    #[serde(default = "default_test_dir")]
    pub test_dir: String,

    /// Default env file path
    #[serde(default = "default_env_file")]
    pub env_file: String,

    /// Global default timeout in milliseconds
    #[serde(default = "default_timeout")]
    pub timeout: u64,

    /// Retry failed requests (0 = no retries)
    #[serde(default)]
    pub retries: u32,

    /// Run test files in parallel (future)
    #[serde(default)]
    pub parallel: bool,
}

fn default_test_dir() -> String {
    "tests".into()
}

fn default_env_file() -> String {
    "tarn.env.yaml".into()
}

fn default_timeout() -> u64 {
    10000
}

impl Default for TarnConfig {
    fn default() -> Self {
        Self {
            test_dir: default_test_dir(),
            env_file: default_env_file(),
            timeout: default_timeout(),
            retries: 0,
            parallel: false,
        }
    }
}

/// Load project config from tarn.config.yaml.
/// Returns default config if the file doesn't exist.
pub fn load_config(base_dir: &Path) -> Result<TarnConfig, TarnError> {
    let config_path = base_dir.join("tarn.config.yaml");
    if !config_path.exists() {
        return Ok(TarnConfig::default());
    }

    let content = std::fs::read_to_string(&config_path).map_err(|e| {
        TarnError::Config(format!("Failed to read {}: {}", config_path.display(), e))
    })?;

    let config: TarnConfig = serde_yaml::from_str(&content).map_err(|e| {
        TarnError::Config(format!("Failed to parse {}: {}", config_path.display(), e))
    })?;

    Ok(config)
}

/// Find the nearest project root by walking ancestors for tarn.config.yaml or default env files.
pub fn find_project_root(start_dir: &Path) -> Option<PathBuf> {
    for dir in start_dir.ancestors() {
        if dir.join("tarn.config.yaml").is_file()
            || dir.join("tarn.env.yaml").is_file()
            || dir.join("tarn.env.local.yaml").is_file()
        {
            return Some(dir.to_path_buf());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn default_config() {
        let config = TarnConfig::default();
        assert_eq!(config.test_dir, "tests");
        assert_eq!(config.env_file, "tarn.env.yaml");
        assert_eq!(config.timeout, 10000);
        assert_eq!(config.retries, 0);
        assert!(!config.parallel);
    }

    #[test]
    fn load_config_missing_file_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.test_dir, "tests");
        assert_eq!(config.timeout, 10000);
    }

    #[test]
    fn load_config_with_all_fields() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("tarn.config.yaml");
        let mut f = std::fs::File::create(&config_path).unwrap();
        write!(
            f,
            "test_dir: my_tests\nenv_file: custom.env.yaml\ntimeout: 5000\nretries: 2\nparallel: true"
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.test_dir, "my_tests");
        assert_eq!(config.env_file, "custom.env.yaml");
        assert_eq!(config.timeout, 5000);
        assert_eq!(config.retries, 2);
        assert!(config.parallel);
    }

    #[test]
    fn load_config_partial_uses_defaults() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("tarn.config.yaml");
        let mut f = std::fs::File::create(&config_path).unwrap();
        write!(f, "timeout: 3000").unwrap();

        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.test_dir, "tests"); // default
        assert_eq!(config.timeout, 3000); // overridden
        assert_eq!(config.retries, 0); // default
    }

    #[test]
    fn load_config_invalid_yaml() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("tarn.config.yaml");
        let mut f = std::fs::File::create(&config_path).unwrap();
        write!(f, "invalid: [yaml: content").unwrap();

        let result = load_config(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to parse"));
    }

    #[test]
    fn load_config_empty_file() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("tarn.config.yaml");
        // Empty YAML file deserializes to null which won't work with struct
        let mut f = std::fs::File::create(&config_path).unwrap();
        write!(f, "{{}}").unwrap();

        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.test_dir, "tests"); // all defaults
    }

    #[test]
    fn find_project_root_finds_nearest_ancestor_with_config() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("tests").join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join("tarn.config.yaml"), "test_dir: tests\n").unwrap();

        let root = find_project_root(&nested).unwrap();
        assert_eq!(root, dir.path());
    }

    #[test]
    fn find_project_root_returns_none_without_config() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("tests");
        std::fs::create_dir_all(&nested).unwrap();

        assert!(find_project_root(&nested).is_none());
    }

    #[test]
    fn find_project_root_finds_default_env_without_config() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("tests");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            dir.path().join("tarn.env.yaml"),
            "base_url: http://localhost\n",
        )
        .unwrap();

        let root = find_project_root(&nested).unwrap();
        assert_eq!(root, dir.path());
    }
}
