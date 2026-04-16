use crate::error::TarnError;
use crate::model::{AuthConfig, Defaults, HttpTransportConfig, RedactionConfig};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Project-level request defaults from tarn.config.yaml.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ProjectDefaults {
    #[serde(default)]
    pub headers: HashMap<String, String>,
    pub auth: Option<AuthConfig>,
    pub timeout: Option<u64>,
    #[serde(alias = "connect-timeout")]
    pub connect_timeout: Option<u64>,
    #[serde(alias = "follow-redirects")]
    pub follow_redirects: Option<bool>,
    #[serde(alias = "max-redirs")]
    pub max_redirs: Option<u32>,
    pub retries: Option<u32>,
    pub delay: Option<String>,
}

/// Named project environment profile.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct NamedEnvironmentConfig {
    pub env_file: Option<String>,
    #[serde(default)]
    pub vars: HashMap<String, String>,
}

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

    /// Project-level request defaults merged into file-level defaults.
    #[serde(default)]
    pub defaults: Option<ProjectDefaults>,

    /// Project-level default redaction policy.
    #[serde(default)]
    pub redaction: Option<RedactionConfig>,

    /// Named environment profiles for `--env`.
    #[serde(default)]
    pub environments: HashMap<String, NamedEnvironmentConfig>,

    /// Explicit proxy URL for HTTP and HTTPS requests.
    #[serde(default)]
    pub proxy: Option<String>,

    /// Hosts that should bypass the configured proxy.
    #[serde(default, alias = "no-proxy")]
    pub no_proxy: Option<String>,

    /// Additional PEM CA bundle to trust.
    #[serde(default)]
    pub cacert: Option<String>,

    /// Client certificate PEM file.
    #[serde(default)]
    pub cert: Option<String>,

    /// Client private key PEM file.
    #[serde(default)]
    pub key: Option<String>,

    /// Disable TLS certificate and hostname verification.
    #[serde(default)]
    pub insecure: bool,

    /// Abort the remaining steps inside each test the moment one step
    /// fails. Keeps reports short in integration suites where the first
    /// failure (auth break, schema mismatch, missing capture) makes
    /// every subsequent step a cascade echo rather than new information.
    #[serde(default, alias = "fail-fast-within-test")]
    pub fail_fast_within_test: bool,
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
            defaults: None,
            redaction: None,
            environments: HashMap::new(),
            proxy: None,
            no_proxy: None,
            cacert: None,
            cert: None,
            key: None,
            insecure: false,
            fail_fast_within_test: false,
        }
    }
}

impl TarnConfig {
    pub fn normalized(mut self) -> Self {
        let defaults = self.defaults.get_or_insert_with(ProjectDefaults::default);
        if defaults.timeout.is_none() {
            defaults.timeout = Some(self.timeout);
        }
        if defaults.retries.is_none() {
            defaults.retries = Some(self.retries);
        }
        self
    }

    pub fn request_defaults(&self) -> Defaults {
        let defaults = self.defaults.clone().unwrap_or_else(|| ProjectDefaults {
            timeout: Some(self.timeout),
            retries: Some(self.retries),
            ..ProjectDefaults::default()
        });

        Defaults {
            headers: defaults.headers,
            auth: defaults.auth,
            timeout: defaults.timeout,
            connect_timeout: defaults.connect_timeout,
            follow_redirects: defaults.follow_redirects,
            max_redirs: defaults.max_redirs,
            retries: defaults.retries,
            delay: defaults.delay,
        }
    }

    pub fn http_transport(&self) -> HttpTransportConfig {
        HttpTransportConfig {
            proxy: self.proxy.clone(),
            no_proxy: self.no_proxy.clone(),
            cacert: self.cacert.clone(),
            cert: self.cert.clone(),
            key: self.key.clone(),
            insecure: self.insecure,
            http_version: None,
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

    Ok(config.normalized())
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
        assert!(config.defaults.is_none());
        assert!(config.redaction.is_none());
        assert!(config.environments.is_empty());
        assert_eq!(config.proxy, None);
        assert_eq!(config.no_proxy, None);
        assert_eq!(config.cacert, None);
        assert_eq!(config.cert, None);
        assert_eq!(config.key, None);
        assert!(!config.insecure);
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
            "test_dir: my_tests\nenv_file: custom.env.yaml\ntimeout: 5000\nretries: 2\nparallel: true\ndefaults:\n  connect_timeout: 1500\n  follow_redirects: false\n  max_redirs: 2\n  delay: 50ms\nredaction:\n  headers: [authorization]\nenvironments:\n  staging:\n    env_file: env/staging.yaml\n    vars:\n      base_url: https://staging.example.com\nproxy: http://127.0.0.1:8080\nno-proxy: localhost,127.0.0.1\ncacert: certs/ca.pem\ncert: certs/client.pem\nkey: certs/client-key.pem\ninsecure: true"
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.test_dir, "my_tests");
        assert_eq!(config.env_file, "custom.env.yaml");
        assert_eq!(config.timeout, 5000);
        assert_eq!(config.retries, 2);
        assert!(config.parallel);
        let defaults = config.defaults.as_ref().unwrap();
        assert_eq!(defaults.timeout, Some(5000));
        assert_eq!(defaults.retries, Some(2));
        assert_eq!(defaults.connect_timeout, Some(1500));
        assert_eq!(defaults.follow_redirects, Some(false));
        assert_eq!(defaults.max_redirs, Some(2));
        assert_eq!(defaults.delay.as_deref(), Some("50ms"));
        assert_eq!(
            config.redaction.as_ref().unwrap().headers,
            vec!["authorization"]
        );
        assert_eq!(
            config.environments["staging"].env_file.as_deref(),
            Some("env/staging.yaml")
        );
        assert_eq!(config.proxy.as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(config.no_proxy.as_deref(), Some("localhost,127.0.0.1"));
        assert_eq!(config.cacert.as_deref(), Some("certs/ca.pem"));
        assert_eq!(config.cert.as_deref(), Some("certs/client.pem"));
        assert_eq!(config.key.as_deref(), Some("certs/client-key.pem"));
        assert!(config.insecure);
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
        assert_eq!(config.request_defaults().timeout, Some(3000));
        assert_eq!(config.proxy, None); // default
        assert!(!config.insecure); // default
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
    fn http_transport_maps_proxy_fields() {
        let config = TarnConfig {
            test_dir: "tests".into(),
            env_file: "tarn.env.yaml".into(),
            timeout: 10000,
            retries: 0,
            parallel: false,
            defaults: None,
            redaction: None,
            environments: HashMap::new(),
            proxy: Some("http://127.0.0.1:8080".into()),
            no_proxy: Some("localhost".into()),
            cacert: Some("certs/ca.pem".into()),
            cert: Some("certs/client.pem".into()),
            key: Some("certs/client-key.pem".into()),
            insecure: true,
            fail_fast_within_test: false,
        };

        let transport = config.http_transport();
        assert_eq!(transport.proxy.as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(transport.no_proxy.as_deref(), Some("localhost"));
        assert_eq!(transport.cacert.as_deref(), Some("certs/ca.pem"));
        assert_eq!(transport.cert.as_deref(), Some("certs/client.pem"));
        assert_eq!(transport.key.as_deref(), Some("certs/client-key.pem"));
        assert!(transport.insecure);
    }

    #[test]
    fn request_defaults_merge_project_policy() {
        let config = TarnConfig {
            test_dir: "tests".into(),
            env_file: "tarn.env.yaml".into(),
            timeout: 4000,
            retries: 2,
            parallel: false,
            defaults: Some(ProjectDefaults {
                connect_timeout: Some(250),
                ..ProjectDefaults::default()
            }),
            redaction: None,
            environments: HashMap::new(),
            proxy: None,
            no_proxy: None,
            cacert: None,
            cert: None,
            key: None,
            insecure: false,
            fail_fast_within_test: false,
        }
        .normalized();

        let defaults = config.request_defaults();
        assert_eq!(defaults.timeout, Some(4000));
        assert_eq!(defaults.retries, Some(2));
        assert_eq!(defaults.connect_timeout, Some(250));
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
