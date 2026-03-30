use crate::error::TarnError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Resolve environment variables with the priority chain:
/// CLI --var > shell env ${VAR} > tarn.env.local.yaml > tarn.env.{name}.yaml > tarn.env.yaml > inline env: block
pub fn resolve_env(
    inline_env: &HashMap<String, String>,
    env_name: Option<&str>,
    cli_vars: &[(String, String)],
    base_dir: &Path,
) -> Result<HashMap<String, String>, TarnError> {
    resolve_env_with_file(inline_env, env_name, cli_vars, base_dir, "tarn.env.yaml")
}

/// Resolve environment variables using a configurable env file name.
pub fn resolve_env_with_file(
    inline_env: &HashMap<String, String>,
    env_name: Option<&str>,
    cli_vars: &[(String, String)],
    base_dir: &Path,
    env_file_name: &str,
) -> Result<HashMap<String, String>, TarnError> {
    let mut env = HashMap::new();

    // Layer 1 (lowest): inline env: block from test file
    for (k, v) in inline_env {
        env.insert(k.clone(), v.clone());
    }

    // Layer 2: tarn.env.yaml (default env file)
    let default_env_file = base_dir.join(env_file_name);
    if default_env_file.exists() {
        let file_env = load_env_file(&default_env_file)?;
        for (k, v) in file_env {
            env.insert(k, v);
        }
    }

    // Layer 3: tarn.env.{name}.yaml (environment-specific)
    if let Some(name) = env_name {
        let named_env_file = base_dir.join(env_variant_filename(env_file_name, name));
        if named_env_file.exists() {
            let file_env = load_env_file(&named_env_file)?;
            for (k, v) in file_env {
                env.insert(k, v);
            }
        }
    }

    // Layer 4: tarn.env.local.yaml (gitignored, secrets)
    let local_env_file = base_dir.join(env_variant_filename(env_file_name, "local"));
    if local_env_file.exists() {
        let file_env = load_env_file(&local_env_file)?;
        for (k, v) in file_env {
            env.insert(k, v);
        }
    }

    // Layer 5: shell environment variables (resolve ${VAR} references)
    let resolved: HashMap<String, String> = env
        .into_iter()
        .map(|(k, v)| {
            let resolved_v = resolve_shell_vars(&v);
            (k, resolved_v)
        })
        .collect();
    env = resolved;

    // Layer 6 (highest): CLI --var overrides
    for (k, v) in cli_vars {
        env.insert(k.clone(), v.clone());
    }

    Ok(env)
}

fn env_variant_filename(env_file_name: &str, suffix: &str) -> PathBuf {
    let path = Path::new(env_file_name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(env_file_name);

    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => PathBuf::from(format!("{stem}.{suffix}.{ext}")),
        None => PathBuf::from(format!("{stem}.{suffix}")),
    }
}

/// Load an env file (YAML key-value pairs).
fn load_env_file(path: &Path) -> Result<HashMap<String, String>, TarnError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        TarnError::Config(format!("Failed to read env file {}: {}", path.display(), e))
    })?;

    let map: HashMap<String, serde_yaml::Value> = serde_yaml::from_str(&content).map_err(|e| {
        TarnError::Config(format!(
            "Failed to parse env file {}: {}",
            path.display(),
            e
        ))
    })?;

    Ok(map
        .into_iter()
        .map(|(k, v)| {
            let s = match v {
                serde_yaml::Value::String(s) => s,
                serde_yaml::Value::Number(n) => n.to_string(),
                serde_yaml::Value::Bool(b) => b.to_string(),
                serde_yaml::Value::Null => String::new(),
                other => format!("{:?}", other),
            };
            (k, s)
        })
        .collect())
}

/// Resolve ${VAR_NAME} references in a string using shell environment variables.
fn resolve_shell_vars(value: &str) -> String {
    let mut result = value.to_string();
    // Find all ${VAR} patterns
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let replacement = std::env::var(var_name).unwrap_or_default();
            result = format!(
                "{}{}{}",
                &result[..start],
                replacement,
                &result[start + end + 1..]
            );
        } else {
            break;
        }
    }
    result
}

/// Parse CLI --var arguments from "key=value" format.
pub fn parse_cli_vars(vars: &[String]) -> Result<Vec<(String, String)>, TarnError> {
    vars.iter()
        .map(|s| {
            let parts: Vec<&str> = s.splitn(2, '=').collect();
            if parts.len() != 2 {
                Err(TarnError::Config(format!(
                    "Invalid --var format '{}': expected key=value",
                    s
                )))
            } else {
                Ok((parts[0].to_string(), parts[1].to_string()))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_env_files(dir: &TempDir, files: &[(&str, &str)]) {
        for (name, content) in files {
            let path = dir.path().join(name);
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(content.as_bytes()).unwrap();
        }
    }

    // --- Priority chain ---

    #[test]
    fn inline_env_is_base_layer() {
        let dir = TempDir::new().unwrap();
        let mut inline = HashMap::new();
        inline.insert("base_url".into(), "http://localhost:3000".into());

        let env = resolve_env(&inline, None, &[], dir.path()).unwrap();
        assert_eq!(env.get("base_url").unwrap(), "http://localhost:3000");
    }

    #[test]
    fn env_file_overrides_inline() {
        let dir = TempDir::new().unwrap();
        setup_env_files(&dir, &[("tarn.env.yaml", "base_url: http://from-file")]);

        let mut inline = HashMap::new();
        inline.insert("base_url".into(), "http://inline".into());

        let env = resolve_env(&inline, None, &[], dir.path()).unwrap();
        assert_eq!(env.get("base_url").unwrap(), "http://from-file");
    }

    #[test]
    fn named_env_overrides_default() {
        let dir = TempDir::new().unwrap();
        setup_env_files(
            &dir,
            &[
                ("tarn.env.yaml", "base_url: http://default"),
                ("tarn.env.staging.yaml", "base_url: http://staging"),
            ],
        );

        let env = resolve_env(&HashMap::new(), Some("staging"), &[], dir.path()).unwrap();
        assert_eq!(env.get("base_url").unwrap(), "http://staging");
    }

    #[test]
    fn local_env_overrides_named() {
        let dir = TempDir::new().unwrap();
        setup_env_files(
            &dir,
            &[
                ("tarn.env.yaml", "base_url: http://default"),
                ("tarn.env.staging.yaml", "base_url: http://staging"),
                ("tarn.env.local.yaml", "base_url: http://local"),
            ],
        );

        let env = resolve_env(&HashMap::new(), Some("staging"), &[], dir.path()).unwrap();
        assert_eq!(env.get("base_url").unwrap(), "http://local");
    }

    #[test]
    fn cli_var_overrides_all() {
        let dir = TempDir::new().unwrap();
        setup_env_files(
            &dir,
            &[
                ("tarn.env.yaml", "base_url: http://default"),
                ("tarn.env.local.yaml", "base_url: http://local"),
            ],
        );

        let mut inline = HashMap::new();
        inline.insert("base_url".into(), "http://inline".into());

        let cli_vars = vec![("base_url".into(), "http://cli".into())];
        let env = resolve_env(&inline, None, &cli_vars, dir.path()).unwrap();
        assert_eq!(env.get("base_url").unwrap(), "http://cli");
    }

    // --- Shell variable resolution ---

    #[test]
    fn resolve_shell_variable() {
        std::env::set_var("HIVE_TEST_SECRET", "s3cret");
        let result = resolve_shell_vars("password is ${HIVE_TEST_SECRET}");
        assert_eq!(result, "password is s3cret");
        std::env::remove_var("HIVE_TEST_SECRET");
    }

    #[test]
    fn resolve_missing_shell_variable_becomes_empty() {
        let result = resolve_shell_vars("${HIVE_NONEXISTENT_VAR}");
        assert_eq!(result, "");
    }

    #[test]
    fn resolve_multiple_shell_variables() {
        std::env::set_var("HIVE_TEST_A", "alpha");
        std::env::set_var("HIVE_TEST_B", "beta");
        let result = resolve_shell_vars("${HIVE_TEST_A} and ${HIVE_TEST_B}");
        assert_eq!(result, "alpha and beta");
        std::env::remove_var("HIVE_TEST_A");
        std::env::remove_var("HIVE_TEST_B");
    }

    #[test]
    fn no_shell_vars_unchanged() {
        let result = resolve_shell_vars("no variables here");
        assert_eq!(result, "no variables here");
    }

    // --- Env file parsing ---

    #[test]
    fn load_env_file_with_various_types() {
        let dir = TempDir::new().unwrap();
        setup_env_files(
            &dir,
            &[(
                "test.yaml",
                "string_val: hello\nnumber_val: 42\nbool_val: true\nnull_val: null",
            )],
        );

        let env = load_env_file(&dir.path().join("test.yaml")).unwrap();
        assert_eq!(env.get("string_val").unwrap(), "hello");
        assert_eq!(env.get("number_val").unwrap(), "42");
        assert_eq!(env.get("bool_val").unwrap(), "true");
        assert_eq!(env.get("null_val").unwrap(), "");
    }

    #[test]
    fn load_env_file_missing() {
        let result = load_env_file(Path::new("/nonexistent/file.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn load_env_file_invalid_yaml() {
        let dir = TempDir::new().unwrap();
        setup_env_files(&dir, &[("bad.yaml", "not: [valid: yaml")]);
        let result = load_env_file(&dir.path().join("bad.yaml"));
        assert!(result.is_err());
    }

    // --- CLI var parsing ---

    #[test]
    fn parse_cli_vars_valid() {
        let vars = vec!["key=value".into(), "another=with=equals".into()];
        let result = parse_cli_vars(&vars).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], ("key".into(), "value".into()));
        assert_eq!(result[1], ("another".into(), "with=equals".into()));
    }

    #[test]
    fn parse_cli_vars_invalid() {
        let vars = vec!["no_equals_sign".into()];
        let result = parse_cli_vars(&vars);
        assert!(result.is_err());
    }

    #[test]
    fn parse_cli_vars_empty() {
        let vars: Vec<String> = vec![];
        let result = parse_cli_vars(&vars).unwrap();
        assert!(result.is_empty());
    }

    // --- Merging ---

    #[test]
    fn variables_from_different_layers_merge() {
        let dir = TempDir::new().unwrap();
        setup_env_files(
            &dir,
            &[("tarn.env.yaml", "file_only: from_file\nbase_url: from_file")],
        );

        let mut inline = HashMap::new();
        inline.insert("inline_only".into(), "from_inline".into());
        inline.insert("base_url".into(), "from_inline".into());

        let env = resolve_env(&inline, None, &[], dir.path()).unwrap();
        assert_eq!(env.get("inline_only").unwrap(), "from_inline");
        assert_eq!(env.get("file_only").unwrap(), "from_file");
        assert_eq!(env.get("base_url").unwrap(), "from_file"); // file overrides inline
    }

    #[test]
    fn missing_named_env_file_is_ok() {
        let dir = TempDir::new().unwrap();
        // No tarn.env.staging.yaml exists
        let env = resolve_env(&HashMap::new(), Some("staging"), &[], dir.path()).unwrap();
        assert!(env.is_empty());
    }

    #[test]
    fn custom_env_file_supports_named_and_local_variants() {
        let dir = TempDir::new().unwrap();
        setup_env_files(
            &dir,
            &[
                ("custom.env.yaml", "base_url: http://default"),
                ("custom.env.staging.yaml", "base_url: http://staging"),
                ("custom.env.local.yaml", "token: secret"),
            ],
        );

        let env = resolve_env_with_file(
            &HashMap::new(),
            Some("staging"),
            &[],
            dir.path(),
            "custom.env.yaml",
        )
        .unwrap();

        assert_eq!(env.get("base_url").unwrap(), "http://staging");
        assert_eq!(env.get("token").unwrap(), "secret");
    }

    #[test]
    fn env_variant_filename_inserts_suffix_before_extension() {
        assert_eq!(
            env_variant_filename("tarn.env.yaml", "local"),
            PathBuf::from("tarn.env.local.yaml")
        );
        assert_eq!(
            env_variant_filename("custom.env.yaml", "staging"),
            PathBuf::from("custom.env.staging.yaml")
        );
    }
}
