use crate::error::TarnError;
use crate::model::TestFile;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Parse a .tarn.yaml file into a TestFile struct.
pub fn parse_file(path: &Path) -> Result<TestFile, TarnError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| TarnError::Parse(format!("Failed to read {}: {}", path.display(), e)))?;
    parse_str(&content, path)
}

/// Parse YAML content string into a TestFile struct.
pub fn parse_str(content: &str, path: &Path) -> Result<TestFile, TarnError> {
    // Check if includes need resolution (fast path: skip if no include directives)
    if content.contains("include:") {
        // Two-pass: resolve includes first at the YAML value level
        let mut raw_value: serde_yaml::Value = serde_yaml::from_str(content)
            .map_err(|e| TarnError::Parse(enhance_parse_error(content, &e, path)))?;

        let base_dir = path.parent().unwrap_or(Path::new("."));
        let mut visited = HashSet::new();
        if let Ok(canonical) = std::fs::canonicalize(path) {
            visited.insert(canonical);
        } else {
            visited.insert(path.to_path_buf());
        }
        resolve_includes(&mut raw_value, base_dir, &mut visited)?;

        let test_file: TestFile = serde_yaml::from_value(raw_value)
            .map_err(|e| TarnError::Parse(format!("{}: {}", path.display(), e)))?;
        validate_test_file(&test_file, path)?;
        Ok(test_file)
    } else {
        // Fast path: direct deserialization (no includes)
        let test_file: TestFile = serde_yaml::from_str(content)
            .map_err(|e| TarnError::Parse(enhance_parse_error(content, &e, path)))?;
        validate_test_file(&test_file, path)?;
        Ok(test_file)
    }
}

/// Resolve `include:` directives in step arrays at the YAML value level.
/// Replaces `{ include: "./path.tarn.yaml" }` entries with the steps from the included file.
fn resolve_includes(
    value: &mut serde_yaml::Value,
    base_dir: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), TarnError> {
    // Process setup, teardown, steps arrays
    for key in &["setup", "teardown", "steps"] {
        if let Some(steps) = value.get_mut(*key) {
            resolve_step_includes(steps, base_dir, visited)?;
        }
    }

    // Process tests.*.steps arrays
    if let Some(serde_yaml::Value::Mapping(tests)) = value.get_mut("tests") {
        let keys: Vec<serde_yaml::Value> = tests.keys().cloned().collect();
        for key in keys {
            if let Some(test_group) = tests.get_mut(&key) {
                if let Some(steps) = test_group.get_mut("steps") {
                    resolve_step_includes(steps, base_dir, visited)?;
                }
            }
        }
    }

    Ok(())
}

/// Resolve include directives within a step array (YAML sequence).
fn resolve_step_includes(
    steps: &mut serde_yaml::Value,
    base_dir: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), TarnError> {
    if let serde_yaml::Value::Sequence(seq) = steps {
        let mut new_seq = Vec::new();

        for item in seq.iter() {
            if let Some(include_path) = item
                .as_mapping()
                .and_then(|m| m.get(serde_yaml::Value::String("include".to_string())))
                .and_then(|v| v.as_str())
            {
                // This is an include directive
                let full_path = base_dir.join(include_path);
                let canonical = std::fs::canonicalize(&full_path).map_err(|e| {
                    TarnError::Parse(format!(
                        "Include file not found: {} (resolved to {}): {}",
                        include_path,
                        full_path.display(),
                        e
                    ))
                })?;

                if !visited.insert(canonical.clone()) {
                    return Err(TarnError::Parse(format!(
                        "Circular include detected: {}",
                        canonical.display()
                    )));
                }

                let content = std::fs::read_to_string(&canonical).map_err(|e| {
                    TarnError::Parse(format!(
                        "Failed to read include file {}: {}",
                        canonical.display(),
                        e
                    ))
                })?;

                let mut included: serde_yaml::Value =
                    serde_yaml::from_str(&content).map_err(|e| {
                        TarnError::Parse(format!(
                            "Failed to parse include file {}: {}",
                            canonical.display(),
                            e
                        ))
                    })?;

                // Recursively resolve includes in the included file
                let included_base = canonical.parent().unwrap_or(Path::new("."));
                resolve_includes(&mut included, included_base, visited)?;

                // Extract steps from the included file: setup + steps
                if let Some(serde_yaml::Value::Sequence(setup_steps)) = included.get("setup") {
                    new_seq.extend(setup_steps.iter().cloned());
                }
                if let Some(serde_yaml::Value::Sequence(main_steps)) = included.get("steps") {
                    new_seq.extend(main_steps.iter().cloned());
                }

                // Remove from visited to allow re-inclusion in sibling arrays
                visited.remove(&canonical);
            } else {
                // Regular step — keep as-is
                new_seq.push(item.clone());
            }
        }

        *seq = new_seq;
    }

    Ok(())
}

/// Produce enhanced parse error messages with suggestions.
fn enhance_parse_error(raw: &str, err: &serde_yaml::Error, path: &Path) -> String {
    let location = err
        .location()
        .map(|loc| format!("{}:{}:{}", path.display(), loc.line(), loc.column()))
        .unwrap_or_else(|| path.display().to_string());

    let base_msg = format!("{}: {}", location, err);

    // Check for tabs in YAML
    if raw.contains('\t') {
        let tab_line = raw
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains('\t'));
        if let Some((line_num, _)) = tab_line {
            return format!(
                "{}\n  hint: YAML does not allow tabs for indentation. Found tab at line {}. Use spaces instead.",
                base_msg,
                line_num + 1
            );
        }
    }

    let err_str = err.to_string();

    // Check for body assertion format mistake (list instead of map)
    if err_str.contains("invalid type: sequence, expected a map") {
        return format!(
            "{}\n  hint: Body assertions use map format, not a list.\n  Use:\n    body:\n      \"$.field\": \"value\"\n  Not:\n    body:\n      - path: \"$.field\"\n        eq: \"value\"",
            base_msg
        );
    }

    // Check for common field name typos
    if err_str.contains("unknown field") {
        let typo_suggestions = [
            ("step", "steps"),
            ("test", "tests"),
            ("assertion", "assert"),
            ("asserts", "assert"),
            ("header", "headers"),
            ("tag", "tags"),
            ("default", "defaults"),
            ("environment", "env"),
            ("teardowns", "teardown"),
            ("setups", "setup"),
        ];
        for (typo, correct) in typo_suggestions {
            if err_str.contains(typo) {
                return format!(
                    "{}\n  hint: Did you mean `{}` instead of `{}`?",
                    base_msg, correct, typo
                );
            }
        }
    }

    base_msg
}

/// Validate semantic constraints on a parsed TestFile.
fn validate_test_file(tf: &TestFile, path: &Path) -> Result<(), TarnError> {
    // Must have either steps or tests (or both for setup+tests pattern)
    if tf.steps.is_empty() && tf.tests.is_empty() {
        return Err(TarnError::Parse(format!(
            "{}: Test file must have either 'steps' or 'tests'",
            path.display()
        )));
    }

    // Validate each step has a non-empty name
    let all_steps = tf
        .setup
        .iter()
        .chain(tf.teardown.iter())
        .chain(tf.steps.iter())
        .chain(tf.tests.values().flat_map(|t| t.steps.iter()));

    for step in all_steps {
        if step.name.trim().is_empty() {
            return Err(TarnError::Parse(format!(
                "{}: Step name cannot be empty",
                path.display()
            )));
        }
        if step.request.method.trim().is_empty() {
            return Err(TarnError::Parse(format!(
                "{}: Step '{}' has empty HTTP method",
                path.display(),
                step.name
            )));
        }
        if step.request.url.trim().is_empty() {
            return Err(TarnError::Parse(format!(
                "{}: Step '{}' has empty URL",
                path.display(),
                step.name
            )));
        }
        // Reject steps with both poll and retries
        if step.poll.is_some() && step.retries.is_some() && step.retries.unwrap() > 0 {
            return Err(TarnError::Validation(format!(
                "{}: Step '{}' cannot have both 'poll' and 'retries'",
                path.display(),
                step.name
            )));
        }
        // Reject GraphQL with non-POST method
        if step.request.graphql.is_some() && step.request.method.to_uppercase() != "POST" {
            return Err(TarnError::Validation(format!(
                "{}: Step '{}' has 'graphql' but method is '{}' (must be POST)",
                path.display(),
                step.name,
                step.request.method
            )));
        }
        // Reject steps with both body and graphql
        if step.request.body.is_some() && step.request.graphql.is_some() {
            return Err(TarnError::Validation(format!(
                "{}: Step '{}' cannot have both 'body' and 'graphql' on request",
                path.display(),
                step.name
            )));
        }
        // Reject steps with both body and multipart
        if step.request.body.is_some() && step.request.multipart.is_some() {
            return Err(TarnError::Validation(format!(
                "{}: Step '{}' cannot have both 'body' and 'multipart' on request",
                path.display(),
                step.name
            )));
        }
        // Reject multipart with graphql
        if step.request.multipart.is_some() && step.request.graphql.is_some() {
            return Err(TarnError::Validation(format!(
                "{}: Step '{}' cannot have both 'multipart' and 'graphql' on request",
                path.display(),
                step.name
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn parse_yaml(yaml: &str) -> Result<TestFile, TarnError> {
        parse_str(yaml, Path::new("test.tarn.yaml"))
    }

    #[test]
    fn parse_minimal_yaml() {
        let tf = parse_yaml(
            r#"
name: Health check
steps:
  - name: GET /health
    request:
      method: GET
      url: "http://localhost:3000/health"
    assert:
      status: 200
"#,
        )
        .unwrap();
        assert_eq!(tf.name, "Health check");
        assert_eq!(tf.steps.len(), 1);
    }

    #[test]
    fn parse_file_from_disk() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            r#"
name: Disk test
steps:
  - name: Check
    request:
      method: GET
      url: "http://localhost:3000"
    assert:
      status: 200
"#
        )
        .unwrap();

        let tf = parse_file(file.path()).unwrap();
        assert_eq!(tf.name, "Disk test");
    }

    #[test]
    fn error_on_missing_file() {
        let result = parse_file(Path::new("/nonexistent/test.tarn.yaml"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, TarnError::Parse(_)));
        assert!(err.to_string().contains("Failed to read"));
    }

    #[test]
    fn error_on_invalid_yaml() {
        let result = parse_yaml("not: [valid: yaml: content");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("test.tarn.yaml"),
            "Expected file path in error, got: {}",
            msg
        );
    }

    #[test]
    fn error_on_missing_name_field() {
        let result = parse_yaml(
            r#"
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn error_on_no_steps_or_tests() {
        let result = parse_yaml(
            r#"
name: Empty test
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must have either 'steps' or 'tests'"));
    }

    #[test]
    fn error_on_empty_step_name() {
        let result = parse_yaml(
            r#"
name: Bad step
steps:
  - name: ""
    request:
      method: GET
      url: "http://localhost:3000"
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Step name cannot be empty"));
    }

    #[test]
    fn error_on_empty_method() {
        let result = parse_yaml(
            r#"
name: Bad method
steps:
  - name: test
    request:
      method: ""
      url: "http://localhost:3000"
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("empty HTTP method"));
    }

    #[test]
    fn error_on_empty_url() {
        let result = parse_yaml(
            r#"
name: Bad url
steps:
  - name: test
    request:
      method: GET
      url: ""
"#,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty URL"));
    }

    #[test]
    fn parse_file_with_tests_map() {
        let tf = parse_yaml(
            r#"
name: Test map
tests:
  login:
    description: "Login test"
    steps:
      - name: Login
        request:
          method: POST
          url: "http://localhost:3000/login"
"#,
        )
        .unwrap();
        assert_eq!(tf.tests.len(), 1);
        assert!(tf.tests.contains_key("login"));
    }

    #[test]
    fn validates_setup_and_teardown_steps() {
        let result = parse_yaml(
            r#"
name: Bad setup
setup:
  - name: ""
    request:
      method: GET
      url: "http://localhost:3000"
steps:
  - name: OK step
    request:
      method: GET
      url: "http://localhost:3000"
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Step name cannot be empty"));
    }

    #[test]
    fn schema_validates_all_examples() {
        let schema_str =
            std::fs::read_to_string("../schemas/v1/testfile.json").expect("Schema file not found");
        let schema_value: serde_json::Value =
            serde_json::from_str(&schema_str).expect("Schema is not valid JSON");
        let schema = jsonschema::validator_for(&schema_value).expect("Invalid JSON Schema");

        let examples = glob::glob("../examples/*.tarn.yaml")
            .expect("Failed to glob examples")
            .chain(
                glob::glob("../examples/**/*.tarn.yaml").expect("Failed to glob nested examples"),
            );

        let mut count = 0;
        for entry in examples {
            let path = entry.expect("Failed to read glob entry");
            let yaml_str = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("Failed to read {}", path.display()));
            let yaml_value: serde_json::Value = serde_yaml::from_str(&yaml_str)
                .unwrap_or_else(|_| panic!("Failed to parse YAML {}", path.display()));

            if let Err(err) = schema.validate(&yaml_value) {
                panic!(
                    "Schema validation failed for {}:\n  - {}",
                    path.display(),
                    err
                );
            }
            count += 1;
        }
        assert!(
            count >= 5,
            "Expected at least 5 example files, found {count}"
        );
    }

    #[test]
    fn parse_error_includes_location() {
        let result = parse_yaml("name: test\nsteps: not_a_list\n");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        // Should contain file:line:col format
        assert!(
            msg.contains("test.tarn.yaml:"),
            "Expected location in error, got: {}",
            msg
        );
    }

    #[test]
    fn parse_error_detects_tabs() {
        let result = parse_yaml("name: test\nsteps:\n\t- name: step\n\t  request:\n\t    method: GET\n\t    url: http://x\n");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("tab"),
            "Expected tab hint in error, got: {}",
            msg
        );
    }

    #[test]
    fn parse_error_body_assertion_list_format_hint() {
        let result = parse_yaml(
            r#"
name: Bad body
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
    assert:
      status: 200
      body:
        - path: "$.name"
          eq: "Alice"
"#,
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("map format"),
            "Expected body format hint, got: {}",
            msg
        );
    }

    #[test]
    fn validates_body_multipart_conflict() {
        let result = parse_yaml(
            r#"
name: Conflict
steps:
  - name: test
    request:
      method: POST
      url: "http://localhost:3000"
      body:
        name: "test"
      multipart:
        fields:
          - name: "title"
            value: "test"
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("cannot have both 'body' and 'multipart'"));
    }

    // --- Include tests ---

    #[test]
    fn resolve_includes_from_disk() {
        let dir = tempfile::tempdir().unwrap();

        // Create shared setup file
        let shared_content = r#"
name: Shared auth
steps:
  - name: Login
    request:
      method: POST
      url: "http://localhost:3000/login"
"#;
        std::fs::write(dir.path().join("shared.tarn.yaml"), shared_content).unwrap();

        // Create main test file with include
        let main_content = r#"
name: Main test
setup:
  - include: ./shared.tarn.yaml
steps:
  - name: Check
    request:
      method: GET
      url: "http://localhost:3000/check"
"#;
        let main_path = dir.path().join("main.tarn.yaml");
        std::fs::write(&main_path, main_content).unwrap();

        let tf = parse_file(&main_path).unwrap();
        assert_eq!(tf.name, "Main test");
        // The include should have resolved — setup should contain the Login step
        assert_eq!(tf.setup.len(), 1);
        assert_eq!(tf.setup[0].name, "Login");
    }

    #[test]
    fn resolve_includes_circular_detection() {
        let dir = tempfile::tempdir().unwrap();

        // a.tarn.yaml includes b.tarn.yaml
        let a_content = r#"
name: A
setup:
  - include: ./b.tarn.yaml
steps:
  - name: step
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        std::fs::write(dir.path().join("a.tarn.yaml"), a_content).unwrap();

        // b.tarn.yaml includes a.tarn.yaml (circular!)
        let b_content = r#"
name: B
steps:
  - include: ./a.tarn.yaml
"#;
        std::fs::write(dir.path().join("b.tarn.yaml"), b_content).unwrap();

        let result = parse_file(&dir.path().join("a.tarn.yaml"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Circular include"));
    }

    #[test]
    fn resolve_includes_missing_file() {
        let dir = tempfile::tempdir().unwrap();

        let content = r#"
name: Missing include
setup:
  - include: ./nonexistent.tarn.yaml
steps:
  - name: step
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let path = dir.path().join("test.tarn.yaml");
        std::fs::write(&path, content).unwrap();

        let result = parse_file(&path);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Include file not found"));
    }
}
