use crate::error::TarnError;
use crate::model::TestFile;
use std::path::Path;

/// Parse a .tarn.yaml file into a TestFile struct.
pub fn parse_file(path: &Path) -> Result<TestFile, TarnError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| TarnError::Parse(format!("Failed to read {}: {}", path.display(), e)))?;
    parse_str(&content, path)
}

/// Parse YAML content string into a TestFile struct.
pub fn parse_str(content: &str, path: &Path) -> Result<TestFile, TarnError> {
    let test_file: TestFile = serde_yaml::from_str(content)
        .map_err(|e| TarnError::Parse(enhance_parse_error(content, &e, path)))?;
    validate_test_file(&test_file, path)?;
    Ok(test_file)
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

    // Check for common field name typos
    let err_str = err.to_string();
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
}
