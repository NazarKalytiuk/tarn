use crate::assert::body::ASSERTION_OPERATORS;
use crate::error::TarnError;
use crate::model::{CaptureSpec, Location, Step, TestFile};
use crate::parser_locations::{self, FileLocations, StepLocations};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

const TOP_LEVEL_FIELDS: &[(&str, &str)] = &[
    ("version", "version"),
    ("name", "name"),
    ("description", "description"),
    ("tags", "tags"),
    ("env", "env"),
    ("redaction", "redaction"),
    ("redact", "redaction"),
    ("defaults", "defaults"),
    ("setup", "setup"),
    ("teardown", "teardown"),
    ("tests", "tests"),
    ("steps", "steps"),
    ("cookies", "cookies"),
];
const REDACTION_FIELDS: &[(&str, &str)] = &[
    ("headers", "headers"),
    ("replacement", "replacement"),
    ("env", "env"),
    ("captures", "captures"),
];
const DEFAULT_FIELDS: &[(&str, &str)] = &[
    ("headers", "headers"),
    ("auth", "auth"),
    ("timeout", "timeout"),
    ("connect_timeout", "connect_timeout"),
    ("connect-timeout", "connect_timeout"),
    ("follow_redirects", "follow_redirects"),
    ("follow-redirects", "follow_redirects"),
    ("max_redirs", "max_redirs"),
    ("max-redirs", "max_redirs"),
    ("retries", "retries"),
    ("delay", "delay"),
];
const TEST_GROUP_FIELDS: &[(&str, &str)] = &[
    ("description", "description"),
    ("tags", "tags"),
    ("steps", "steps"),
];
const STEP_FIELDS: &[(&str, &str)] = &[
    ("name", "name"),
    ("description", "description"),
    ("request", "request"),
    ("capture", "capture"),
    ("assert", "assert"),
    ("if", "if"),
    ("unless", "unless"),
    ("retries", "retries"),
    ("timeout", "timeout"),
    ("connect_timeout", "connect_timeout"),
    ("connect-timeout", "connect_timeout"),
    ("follow_redirects", "follow_redirects"),
    ("follow-redirects", "follow_redirects"),
    ("max_redirs", "max_redirs"),
    ("max-redirs", "max_redirs"),
    ("delay", "delay"),
    ("poll", "poll"),
    ("script", "script"),
    ("cookies", "cookies"),
];
const INCLUDE_FIELDS: &[(&str, &str)] = &[
    ("include", "include"),
    ("with", "with"),
    ("override", "override"),
    ("overrides", "override"),
];
const REQUEST_FIELDS: &[(&str, &str)] = &[
    ("method", "method"),
    ("url", "url"),
    ("headers", "headers"),
    ("auth", "auth"),
    ("body", "body"),
    ("form", "form"),
    ("graphql", "graphql"),
    ("multipart", "multipart"),
];
const GRAPHQL_FIELDS: &[(&str, &str)] = &[
    ("query", "query"),
    ("variables", "variables"),
    ("operation_name", "operation_name"),
];
const AUTH_FIELDS: &[(&str, &str)] = &[("bearer", "bearer"), ("basic", "basic")];
const BASIC_AUTH_FIELDS: &[(&str, &str)] = &[("username", "username"), ("password", "password")];
const MULTIPART_FIELDS: &[(&str, &str)] = &[("fields", "fields"), ("files", "files")];
const FORM_FIELD_FIELDS: &[(&str, &str)] = &[("name", "name"), ("value", "value")];
const FILE_FIELD_FIELDS: &[(&str, &str)] = &[
    ("name", "name"),
    ("path", "path"),
    ("content_type", "content_type"),
    ("filename", "filename"),
];
const POLL_FIELDS: &[(&str, &str)] = &[
    ("until", "until"),
    ("interval", "interval"),
    ("max_attempts", "max_attempts"),
];
const ASSERT_FIELDS: &[(&str, &str)] = &[
    ("status", "status"),
    ("duration", "duration"),
    ("redirect", "redirect"),
    ("headers", "headers"),
    ("body", "body"),
];
const REDIRECT_ASSERT_FIELDS: &[(&str, &str)] = &[("url", "url"), ("count", "count")];
const STATUS_ASSERT_FIELDS: &[(&str, &str)] = &[
    ("in", "in"),
    ("gte", "gte"),
    ("gt", "gt"),
    ("lte", "lte"),
    ("lt", "lt"),
];
// Extending this list? Also extend `CAPTURE_EXT_FIELDS` above so
// capture-side validation stays in lockstep with assertion-side.
const CAPTURE_EXT_FIELDS: &[(&str, &str)] = &[
    ("header", "header"),
    ("cookie", "cookie"),
    ("jsonpath", "jsonpath"),
    ("body", "body"),
    ("status", "status"),
    ("url", "url"),
    ("regex", "regex"),
    ("where", "where"),
    ("optional", "optional"),
    ("default", "default"),
    ("when", "when"),
];
/// Valid keys inside a `capture.when:` gate. Kept narrow so a typo
/// like `statuus: 201` fails validation with a fuzzy hint instead of
/// silently letting every response pass the gate.
const CAPTURE_WHEN_FIELDS: &[(&str, &str)] = &[("status", "status")];

/// Parse a .tarn.yaml file into a TestFile struct.
pub fn parse_file(path: &Path) -> Result<TestFile, TarnError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| TarnError::Parse(format!("Failed to read {}: {}", path.display(), e)))?;
    parse_str(&content, path)
}

/// Collect transitive include dependencies for a Tarn file without expanding it.
pub fn include_dependencies(path: &Path) -> Result<Vec<PathBuf>, TarnError> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| TarnError::Parse(format!("Failed to read {}: {}", path.display(), e)))?;
    let mut dependencies = BTreeSet::new();
    let mut visiting = HashSet::from([canonical.clone()]);
    collect_include_dependencies(&canonical, &mut dependencies, &mut visiting)?;
    Ok(dependencies.into_iter().collect())
}

/// Parse YAML content string into a TestFile struct.
pub fn parse_str(content: &str, path: &Path) -> Result<TestFile, TarnError> {
    let mut raw_value: serde_yaml::Value = serde_yaml::from_str(content)
        .map_err(|e| TarnError::Parse(enhance_parse_error(content, &e, path)))?;

    // Extract source locations from the ORIGINAL content (before
    // include expansion), keyed by the display path we will put on
    // every `Location.file` for this file. If a file pulls in includes
    // we fall back to leaving locations as `None` for the included
    // steps — the `location` field on the JSON report is optional and
    // include expansion is best-effort in this first iteration
    // (T55 / NAZ-260). The common case (no includes) gets full
    // coverage.
    let location_file = location_file_display(path);
    let pre_include_locations = parser_locations::extract(content, &location_file);

    if content.contains("include:") {
        let base_dir = path.parent().unwrap_or(Path::new("."));
        let mut visited = HashSet::new();
        if let Ok(canonical) = std::fs::canonicalize(path) {
            visited.insert(canonical);
        } else {
            visited.insert(path.to_path_buf());
        }
        resolve_includes(&mut raw_value, base_dir, &mut visited)?;
    }

    validate_yaml_shape(&raw_value, path)?;

    let mut test_file: TestFile = serde_yaml::from_value(raw_value)
        .map_err(|e| TarnError::Parse(format!("{}: {}", path.display(), e)))?;
    validate_test_file(&test_file, path)?;

    if let Some(locations) = pre_include_locations {
        attach_locations(&mut test_file, &locations);
    }

    Ok(test_file)
}

/// Resolve the path we surface on `Location.file` for a freshly
/// parsed test file. We canonicalize when possible so downstream
/// consumers (VS Code, CI) can anchor results unambiguously, but we
/// fall back to the original display path when canonicalization fails
/// (e.g. when the file only lives in memory during tests).
fn location_file_display(path: &Path) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

/// Copy step locations gathered from the pre-include YAML text onto
/// each parsed `Step`. Sections whose length no longer matches the
/// deserialized `TestFile` (because an `include:` directive expanded
/// into additional steps) are skipped; the `location` field on each
/// `StepResult` is documented as optional for exactly this reason.
fn attach_locations(test_file: &mut TestFile, locations: &FileLocations) {
    attach_section_locations(&mut test_file.setup, &locations.setup);
    attach_section_locations(&mut test_file.teardown, &locations.teardown);
    attach_section_locations(&mut test_file.steps, &locations.flat_steps);
    for (name, group) in test_file.tests.iter_mut() {
        if let Some(group_locations) = locations.tests.get(name) {
            attach_section_locations(&mut group.steps, group_locations);
        }
    }
}

fn attach_section_locations(steps: &mut [Step], locations: &[StepLocations]) {
    if steps.len() != locations.len() {
        // Include expansion (or some other transformation) changed
        // the step count — leave locations as None rather than
        // guessing at a misalignment.
        return;
    }
    for (step, loc) in steps.iter_mut().zip(locations.iter()) {
        if let Some(name_location) = &loc.name {
            step.location = Some(name_location.clone());
        }
        if !loc.assertions.is_empty() {
            step.assertion_locations = loc.assertions.clone();
        }
    }
}

/// Resolve a `Location` for any assertion whose label matches one in
/// the step's `assertion_locations` map. Used by the runner to stamp
/// every `AssertionResult` with its source range.
pub(crate) fn assertion_location(step: &Step, label: &str) -> Option<Location> {
    step.assertion_locations.get(label).cloned()
}

/// Format Tarn YAML into a normalized, canonical layout without expanding include directives.
pub fn format_str(content: &str, path: &Path) -> Result<String, TarnError> {
    let mut raw_value: serde_yaml::Value = serde_yaml::from_str(content)
        .map_err(|e| TarnError::Parse(enhance_parse_error(content, &e, path)))?;
    validate_yaml_shape(&raw_value, path)?;
    validate_formattable_test_file(content, &raw_value, path)?;
    normalize_root_value(&mut raw_value);

    let mut formatted = serde_yaml::to_string(&raw_value).map_err(|e| {
        TarnError::Parse(format!("{}: Failed to render YAML: {}", path.display(), e))
    })?;
    if let Some(rest) = formatted.strip_prefix("---\n") {
        formatted = rest.to_string();
    }
    if let Some(schema_directive) = extract_schema_directive(content) {
        formatted = format!("{schema_directive}\n{formatted}");
    }
    Ok(formatted)
}

fn validate_formattable_test_file(
    content: &str,
    raw_value: &serde_yaml::Value,
    path: &Path,
) -> Result<(), TarnError> {
    let mut validation_value = raw_value.clone();
    if content.contains("include:") {
        let base_dir = path.parent().unwrap_or(Path::new("."));
        let mut visited = HashSet::new();
        if let Ok(canonical) = std::fs::canonicalize(path) {
            visited.insert(canonical);
        } else {
            visited.insert(path.to_path_buf());
        }
        resolve_includes(&mut validation_value, base_dir, &mut visited)?;
    }

    let test_file: TestFile = serde_yaml::from_value(validation_value)
        .map_err(|e| TarnError::Parse(format!("{}: {}", path.display(), e)))?;
    validate_test_file(&test_file, path)
}

fn extract_schema_directive(content: &str) -> Option<&str> {
    content.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with("# yaml-language-server: $schema=") {
            Some(trimmed)
        } else {
            None
        }
    })
}

fn normalize_root_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(map) = value {
        normalize_known_mapping_in_place(map, TOP_LEVEL_FIELDS, |key, value| match key {
            "defaults" => normalize_defaults_value(value),
            "redaction" => normalize_redaction_value(value),
            "setup" | "teardown" | "steps" => normalize_steps_value(value),
            "tests" => normalize_tests_value(value),
            _ => {}
        });
    }
}

fn normalize_defaults_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(map) = value {
        normalize_known_mapping_in_place(map, DEFAULT_FIELDS, |key, value| {
            if key == "auth" {
                normalize_auth_value(value);
            }
        });
    }
}

fn normalize_redaction_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(map) = value {
        normalize_known_mapping_in_place(map, REDACTION_FIELDS, |_key, _value| {});
    }
}

fn normalize_tests_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(tests) = value {
        for (_, group_value) in tests.iter_mut() {
            normalize_test_group_value(group_value);
        }
    }
}

fn normalize_test_group_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(map) = value {
        normalize_known_mapping_in_place(map, TEST_GROUP_FIELDS, |key, value| {
            if key == "steps" {
                normalize_steps_value(value);
            }
        });
    }
}

fn normalize_steps_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Sequence(steps) = value {
        for step_value in steps {
            normalize_step_value(step_value);
        }
    }
}

fn normalize_step_value(value: &mut serde_yaml::Value) {
    let Some(map) = value.as_mapping_mut() else {
        return;
    };
    if map.contains_key(serde_yaml::Value::String("include".into())) {
        normalize_known_mapping_in_place(map, INCLUDE_FIELDS, |key, value| {
            if key == "override" {
                normalize_include_override_value(value);
            }
        });
        return;
    }
    normalize_known_mapping_in_place(map, STEP_FIELDS, |key, value| match key {
        "request" => normalize_request_value(value),
        "capture" => normalize_capture_map_value(value),
        "assert" => normalize_assertion_value(value),
        "poll" => normalize_poll_value(value),
        _ => {}
    });
}

fn normalize_include_override_value(value: &mut serde_yaml::Value) {
    let Some(map) = value.as_mapping_mut() else {
        return;
    };
    normalize_known_mapping_in_place(map, STEP_FIELDS, |key, value| match key {
        "request" => normalize_request_value(value),
        "capture" => normalize_capture_map_value(value),
        "assert" => normalize_assertion_value(value),
        "poll" => normalize_poll_value(value),
        _ => {}
    });
}

fn normalize_request_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(map) = value {
        normalize_known_mapping_in_place(map, REQUEST_FIELDS, |key, value| match key {
            "auth" => normalize_auth_value(value),
            "graphql" => normalize_graphql_value(value),
            "multipart" => normalize_multipart_value(value),
            _ => {}
        });
    }
}

fn normalize_auth_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(map) = value {
        normalize_known_mapping_in_place(map, AUTH_FIELDS, |key, value| {
            if key == "basic" {
                if let serde_yaml::Value::Mapping(map) = value {
                    normalize_known_mapping_in_place(map, BASIC_AUTH_FIELDS, |_key, _value| {});
                }
            }
        });
    }
}

fn normalize_graphql_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(map) = value {
        normalize_known_mapping_in_place(map, GRAPHQL_FIELDS, |_key, _value| {});
    }
}

fn normalize_multipart_value(value: &mut serde_yaml::Value) {
    let Some(map) = value.as_mapping_mut() else {
        return;
    };
    normalize_known_mapping_in_place(map, MULTIPART_FIELDS, |key, value| match key {
        "fields" => normalize_mapping_sequence(value, FORM_FIELD_FIELDS),
        "files" => normalize_mapping_sequence(value, FILE_FIELD_FIELDS),
        _ => {}
    });
}

fn normalize_mapping_sequence(value: &mut serde_yaml::Value, fields: &[(&str, &str)]) {
    if let serde_yaml::Value::Sequence(items) = value {
        for item in items {
            if let serde_yaml::Value::Mapping(map) = item {
                normalize_known_mapping_in_place(map, fields, |_key, _value| {});
            }
        }
    }
}

fn normalize_capture_map_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(map) = value {
        for (_, capture_value) in map.iter_mut() {
            if let serde_yaml::Value::Mapping(capture_map) = capture_value {
                normalize_known_mapping_in_place(
                    capture_map,
                    CAPTURE_EXT_FIELDS,
                    |_key, _value| {},
                );
            }
        }
    }
}

fn normalize_poll_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(map) = value {
        normalize_known_mapping_in_place(map, POLL_FIELDS, |key, value| {
            if key == "until" {
                normalize_assertion_value(value);
            }
        });
    }
}

fn normalize_assertion_value(value: &mut serde_yaml::Value) {
    let Some(map) = value.as_mapping_mut() else {
        return;
    };
    normalize_known_mapping_in_place(map, ASSERT_FIELDS, |key, value| match key {
        "redirect" => normalize_redirect_assertion_value(value),
        "body" => normalize_body_assertions_value(value),
        _ => {}
    });
}

fn normalize_redirect_assertion_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(map) = value {
        normalize_known_mapping_in_place(map, REDIRECT_ASSERT_FIELDS, |_key, _value| {});
    }
}

fn normalize_body_assertions_value(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(assertions) = value {
        for (_, body_assertion) in assertions.iter_mut() {
            if let serde_yaml::Value::Mapping(map) = body_assertion {
                normalize_assertion_operator_map(map);
            }
        }
    }
}

fn normalize_assertion_operator_map(map: &mut serde_yaml::Mapping) {
    let allowed: Vec<(&str, &str)> = ASSERTION_OPERATORS.iter().map(|op| (*op, *op)).collect();
    normalize_known_mapping_in_place(map, &allowed, |_key, _value| {});
}

fn normalize_known_mapping_in_place<F>(
    map: &mut serde_yaml::Mapping,
    fields: &[(&str, &str)],
    mut normalize_value: F,
) where
    F: FnMut(&str, &mut serde_yaml::Value),
{
    let original = std::mem::take(map);
    let mut ordered = serde_yaml::Mapping::new();
    let mut leftovers = Vec::new();

    for (key, mut value) in original {
        let Some(key_name) = yaml_key_as_string(&key).map(|s| s.to_string()) else {
            leftovers.push((key, value));
            continue;
        };

        let canonical = canonical_key(&key_name, fields).unwrap_or(key_name.as_str());
        normalize_value(canonical, &mut value);

        if canonical_key(&key_name, fields).is_some() {
            ordered.insert(serde_yaml::Value::String(canonical.to_string()), value);
        } else {
            leftovers.push((serde_yaml::Value::String(key_name), value));
        }
    }

    let mut normalized = serde_yaml::Mapping::new();
    for (_, canonical) in fields {
        let key = serde_yaml::Value::String((*canonical).to_string());
        if let Some(value) = ordered.remove(&key) {
            normalized.insert(key, value);
        }
    }
    for (key, value) in leftovers {
        normalized.insert(key, value);
    }
    *map = normalized;
}

fn canonical_key<'a>(key: &str, fields: &'a [(&str, &str)]) -> Option<&'a str> {
    fields.iter().find_map(|(alias, canonical)| {
        if *alias == key {
            Some(*canonical)
        } else {
            None
        }
    })
}

fn validate_yaml_shape(value: &serde_yaml::Value, path: &Path) -> Result<(), TarnError> {
    let root = as_mapping(value, path, "root")?;
    validate_mapping_keys(root, TOP_LEVEL_FIELDS, "root", path)?;

    if let Some(value) = mapping_value(root, "redaction").or_else(|| mapping_value(root, "redact"))
    {
        validate_known_mapping(value, REDACTION_FIELDS, "root.redaction", path)?;
    }
    if let Some(value) = mapping_value(root, "defaults") {
        validate_known_mapping(value, DEFAULT_FIELDS, "root.defaults", path)?;
        if let Some(auth) = value
            .as_mapping()
            .and_then(|map| mapping_value(map, "auth"))
        {
            validate_auth(auth, "root.defaults.auth", path)?;
        }
    }
    if let Some(value) = mapping_value(root, "setup") {
        validate_steps_sequence(value, "root.setup", path)?;
    }
    if let Some(value) = mapping_value(root, "teardown") {
        validate_steps_sequence(value, "root.teardown", path)?;
    }
    if let Some(value) = mapping_value(root, "steps") {
        validate_steps_sequence(value, "root.steps", path)?;
    }
    if let Some(value) = mapping_value(root, "tests") {
        let tests = as_mapping(value, path, "root.tests")?;
        for (name, test_group) in tests {
            let test_name = yaml_key_as_string(name).ok_or_else(|| {
                TarnError::Validation(format!(
                    "{}: Test group name at root.tests must be a string",
                    path.display()
                ))
            })?;
            validate_test_group(test_group, &format!("root.tests.{test_name}"), path)?;
        }
    }

    Ok(())
}

fn validate_test_group(
    value: &serde_yaml::Value,
    context: &str,
    path: &Path,
) -> Result<(), TarnError> {
    let group = as_mapping(value, path, context)?;
    validate_mapping_keys(group, TEST_GROUP_FIELDS, context, path)?;
    if let Some(steps) = mapping_value(group, "steps") {
        validate_steps_sequence(steps, &format!("{context}.steps"), path)?;
    }
    Ok(())
}

fn validate_steps_sequence(
    value: &serde_yaml::Value,
    context: &str,
    path: &Path,
) -> Result<(), TarnError> {
    let steps = as_sequence(value, path, context)?;
    for (index, step) in steps.iter().enumerate() {
        validate_step(step, &format!("{context}[{index}]"), path)?;
    }
    Ok(())
}

fn validate_step(value: &serde_yaml::Value, context: &str, path: &Path) -> Result<(), TarnError> {
    let step = as_mapping(value, path, context)?;

    if mapping_value(step, "include").is_some() {
        validate_mapping_keys(step, INCLUDE_FIELDS, context, path)?;
        return Ok(());
    }

    validate_mapping_keys(step, STEP_FIELDS, context, path)?;

    if let Some(request) = mapping_value(step, "request") {
        validate_request(request, &format!("{context}.request"), path)?;
    }
    if let Some(capture) = mapping_value(step, "capture") {
        validate_capture_map(capture, &format!("{context}.capture"), path)?;
    }
    if let Some(assertion) = mapping_value(step, "assert") {
        validate_assertion(assertion, &format!("{context}.assert"), path)?;
    }
    if let Some(poll) = mapping_value(step, "poll") {
        validate_poll(poll, &format!("{context}.poll"), path)?;
    }

    Ok(())
}

fn validate_request(
    value: &serde_yaml::Value,
    context: &str,
    path: &Path,
) -> Result<(), TarnError> {
    let request = as_mapping(value, path, context)?;
    validate_mapping_keys(request, REQUEST_FIELDS, context, path)?;

    if let Some(auth) = mapping_value(request, "auth") {
        validate_auth(auth, &format!("{context}.auth"), path)?;
    }

    if let Some(graphql) = mapping_value(request, "graphql") {
        validate_known_mapping(graphql, GRAPHQL_FIELDS, &format!("{context}.graphql"), path)?;
    }
    if let Some(multipart) = mapping_value(request, "multipart") {
        validate_multipart(multipart, &format!("{context}.multipart"), path)?;
    }

    Ok(())
}

fn validate_auth(value: &serde_yaml::Value, context: &str, path: &Path) -> Result<(), TarnError> {
    let auth = as_mapping(value, path, context)?;
    validate_mapping_keys(auth, AUTH_FIELDS, context, path)?;

    if let Some(basic) = mapping_value(auth, "basic") {
        validate_known_mapping(basic, BASIC_AUTH_FIELDS, &format!("{context}.basic"), path)?;
    }

    let source_count = usize::from(mapping_value(auth, "bearer").is_some())
        + usize::from(mapping_value(auth, "basic").is_some());
    if source_count != 1 {
        return Err(TarnError::Validation(format!(
            "{}: {} must specify exactly one auth helper: bearer or basic",
            path.display(),
            context
        )));
    }

    Ok(())
}

fn validate_multipart(
    value: &serde_yaml::Value,
    context: &str,
    path: &Path,
) -> Result<(), TarnError> {
    let multipart = as_mapping(value, path, context)?;
    validate_mapping_keys(multipart, MULTIPART_FIELDS, context, path)?;

    if let Some(fields) = mapping_value(multipart, "fields") {
        for (index, field) in as_sequence(fields, path, &format!("{context}.fields"))?
            .iter()
            .enumerate()
        {
            validate_known_mapping(
                field,
                FORM_FIELD_FIELDS,
                &format!("{context}.fields[{index}]"),
                path,
            )?;
        }
    }

    if let Some(files) = mapping_value(multipart, "files") {
        for (index, file) in as_sequence(files, path, &format!("{context}.files"))?
            .iter()
            .enumerate()
        {
            validate_known_mapping(
                file,
                FILE_FIELD_FIELDS,
                &format!("{context}.files[{index}]"),
                path,
            )?;
        }
    }

    Ok(())
}

fn validate_capture_map(
    value: &serde_yaml::Value,
    context: &str,
    path: &Path,
) -> Result<(), TarnError> {
    let captures = as_mapping(value, path, context)?;
    for (capture_name, capture_spec) in captures {
        let capture_name = yaml_key_as_string(capture_name).unwrap_or("<capture>");
        if capture_spec.is_mapping() {
            let cap_context = format!("{context}.{capture_name}");
            validate_known_mapping(capture_spec, CAPTURE_EXT_FIELDS, &cap_context, path)?;

            if let Some(when) = mapping_value(capture_spec.as_mapping().unwrap(), "when") {
                validate_known_mapping(
                    when,
                    CAPTURE_WHEN_FIELDS,
                    &format!("{cap_context}.when"),
                    path,
                )?;
                if let Some(status) = mapping_value(when.as_mapping().unwrap(), "status") {
                    if status.is_mapping() {
                        validate_known_mapping(
                            status,
                            STATUS_ASSERT_FIELDS,
                            &format!("{cap_context}.when.status"),
                            path,
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_poll(value: &serde_yaml::Value, context: &str, path: &Path) -> Result<(), TarnError> {
    let poll = as_mapping(value, path, context)?;
    validate_mapping_keys(poll, POLL_FIELDS, context, path)?;
    if let Some(until) = mapping_value(poll, "until") {
        validate_assertion(until, &format!("{context}.until"), path)?;
    }
    Ok(())
}

fn validate_assertion(
    value: &serde_yaml::Value,
    context: &str,
    path: &Path,
) -> Result<(), TarnError> {
    let assertion = as_mapping(value, path, context)?;
    validate_mapping_keys(assertion, ASSERT_FIELDS, context, path)?;

    if let Some(status) = mapping_value(assertion, "status") {
        if status.is_mapping() {
            validate_known_mapping(
                status,
                STATUS_ASSERT_FIELDS,
                &format!("{context}.status"),
                path,
            )?;
        }
    }
    if let Some(redirect) = mapping_value(assertion, "redirect") {
        validate_known_mapping(
            redirect,
            REDIRECT_ASSERT_FIELDS,
            &format!("{context}.redirect"),
            path,
        )?;
    }
    if let Some(body) = mapping_value(assertion, "body") {
        validate_body_assertions(body, &format!("{context}.body"), path)?;
    }

    Ok(())
}

fn validate_body_assertions(
    value: &serde_yaml::Value,
    context: &str,
    path: &Path,
) -> Result<(), TarnError> {
    if value.is_sequence() {
        return Err(TarnError::Validation(format!(
            "{}: Body assertions use map format, not a list.\n  Use:\n    body:\n      \"$.field\": \"value\"\n  Not:\n    body:\n      - path: \"$.field\"\n        eq: \"value\"",
            path.display()
        )));
    }

    let body = as_mapping(value, path, context)?;
    let operator_fields: Vec<(&str, &str)> = crate::assert::body::ASSERTION_OPERATORS
        .iter()
        .map(|operator| (*operator, *operator))
        .collect();

    for (jsonpath, assertion) in body {
        let Some(jsonpath) = yaml_key_as_string(jsonpath) else {
            continue;
        };
        let Some(map) = assertion.as_mapping() else {
            continue;
        };

        let is_operator_map = jsonpath != "$"
            || map.keys().any(|key| {
                yaml_key_as_string(key)
                    .map(|key| crate::assert::body::ASSERTION_OPERATORS.contains(&key))
                    .unwrap_or(false)
            });

        if is_operator_map {
            validate_mapping_keys(
                map,
                &operator_fields,
                &format!("{context}.{jsonpath}"),
                path,
            )?;
        }
    }

    Ok(())
}

fn validate_known_mapping(
    value: &serde_yaml::Value,
    allowed: &[(&str, &str)],
    context: &str,
    path: &Path,
) -> Result<(), TarnError> {
    let map = as_mapping(value, path, context)?;
    validate_mapping_keys(map, allowed, context, path)
}

fn validate_mapping_keys(
    map: &serde_yaml::Mapping,
    allowed: &[(&str, &str)],
    context: &str,
    path: &Path,
) -> Result<(), TarnError> {
    for key in map.keys() {
        let Some(key) = yaml_key_as_string(key) else {
            continue;
        };
        if allowed.iter().any(|(accepted, _)| accepted == &key) {
            continue;
        }
        return Err(unknown_field_error(path, context, key, allowed));
    }
    Ok(())
}

fn as_mapping<'a>(
    value: &'a serde_yaml::Value,
    path: &Path,
    context: &str,
) -> Result<&'a serde_yaml::Mapping, TarnError> {
    value.as_mapping().ok_or_else(|| {
        TarnError::Validation(format!(
            "{}: Expected {} to be a YAML mapping/object",
            path.display(),
            context
        ))
    })
}

fn as_sequence<'a>(
    value: &'a serde_yaml::Value,
    path: &Path,
    context: &str,
) -> Result<&'a Vec<serde_yaml::Value>, TarnError> {
    value.as_sequence().ok_or_else(|| {
        TarnError::Validation(format!(
            "{}: Expected {} to be a YAML list/sequence",
            path.display(),
            context
        ))
    })
}

fn mapping_value<'a>(map: &'a serde_yaml::Mapping, key: &str) -> Option<&'a serde_yaml::Value> {
    map.get(serde_yaml::Value::String(key.to_string()))
}

fn yaml_key_as_string(value: &serde_yaml::Value) -> Option<&str> {
    match value {
        serde_yaml::Value::String(s) => Some(s.as_str()),
        _ => None,
    }
}

fn unknown_field_error(
    path: &Path,
    context: &str,
    unknown: &str,
    allowed: &[(&str, &str)],
) -> TarnError {
    let suggestion = nearest_suggestion(unknown, allowed);
    let mut message = format!(
        "{}: Unknown field '{}' at {}",
        path.display(),
        unknown,
        context
    );
    if let Some(suggestion) = suggestion {
        message.push_str(&format!(". Did you mean '{}'?", suggestion));
    }
    TarnError::Validation(message)
}

fn nearest_suggestion<'a>(unknown: &str, allowed: &'a [(&str, &'a str)]) -> Option<&'a str> {
    let unknown = normalize_key(unknown);
    let (distance, suggestion) = allowed
        .iter()
        .map(|(candidate, display)| (levenshtein(&unknown, &normalize_key(candidate)), *display))
        .min_by_key(|(distance, display)| (*distance, *display))?;

    let threshold = match unknown.len() {
        0..=4 => 1,
        5..=8 => 2,
        _ => 3,
    };
    (distance <= threshold).then_some(suggestion)
}

fn normalize_key(key: &str) -> String {
    key.to_ascii_lowercase().replace('-', "_")
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();

    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];

    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b.len()]
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
                let include_map = item.as_mapping().expect("include directive is a mapping");
                let include_params =
                    mapping_value(include_map, "with").and_then(|value| value.as_mapping());
                let include_override = mapping_value(include_map, "override")
                    .or_else(|| mapping_value(include_map, "overrides"));
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
                apply_include_params(&mut included, include_params);

                // Extract steps from the included file: setup + steps
                if let Some(serde_yaml::Value::Sequence(setup_steps)) = included.get("setup") {
                    new_seq.extend(setup_steps.iter().cloned().map(|mut step| {
                        if let Some(override_value) = include_override {
                            merge_yaml(&mut step, override_value);
                        }
                        step
                    }));
                }
                if let Some(serde_yaml::Value::Sequence(main_steps)) = included.get("steps") {
                    new_seq.extend(main_steps.iter().cloned().map(|mut step| {
                        if let Some(override_value) = include_override {
                            merge_yaml(&mut step, override_value);
                        }
                        step
                    }));
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

fn collect_include_dependencies(
    path: &Path,
    dependencies: &mut BTreeSet<PathBuf>,
    visiting: &mut HashSet<PathBuf>,
) -> Result<(), TarnError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| TarnError::Parse(format!("Failed to read {}: {}", path.display(), e)))?;
    let value: serde_yaml::Value = serde_yaml::from_str(&content)
        .map_err(|e| TarnError::Parse(enhance_parse_error(&content, &e, path)))?;
    let base_dir = path.parent().unwrap_or(Path::new("."));
    collect_include_dependencies_from_value(&value, base_dir, dependencies, visiting)
}

fn collect_include_dependencies_from_value(
    value: &serde_yaml::Value,
    base_dir: &Path,
    dependencies: &mut BTreeSet<PathBuf>,
    visiting: &mut HashSet<PathBuf>,
) -> Result<(), TarnError> {
    for key in ["setup", "teardown", "steps"] {
        if let Some(steps) = value.get(key) {
            collect_include_dependencies_from_steps(steps, base_dir, dependencies, visiting)?;
        }
    }

    if let Some(serde_yaml::Value::Mapping(tests)) = value.get("tests") {
        for (_, group) in tests {
            if let Some(steps) = group.get("steps") {
                collect_include_dependencies_from_steps(steps, base_dir, dependencies, visiting)?;
            }
        }
    }

    Ok(())
}

fn collect_include_dependencies_from_steps(
    steps: &serde_yaml::Value,
    base_dir: &Path,
    dependencies: &mut BTreeSet<PathBuf>,
    visiting: &mut HashSet<PathBuf>,
) -> Result<(), TarnError> {
    let Some(sequence) = steps.as_sequence() else {
        return Ok(());
    };

    for item in sequence {
        let Some(include_path) = item
            .as_mapping()
            .and_then(|map| map.get(serde_yaml::Value::String("include".into())))
            .and_then(|value| value.as_str())
        else {
            continue;
        };

        let canonical = std::fs::canonicalize(base_dir.join(include_path)).map_err(|e| {
            TarnError::Parse(format!(
                "Include file not found: {} (resolved to {}): {}",
                include_path,
                base_dir.join(include_path).display(),
                e
            ))
        })?;
        dependencies.insert(canonical.clone());
        if visiting.insert(canonical.clone()) {
            collect_include_dependencies(&canonical, dependencies, visiting)?;
            visiting.remove(&canonical);
        }
    }

    Ok(())
}

fn apply_include_params(value: &mut serde_yaml::Value, params: Option<&serde_yaml::Mapping>) {
    let Some(params) = params else {
        return;
    };

    match value {
        serde_yaml::Value::String(text) => {
            if let Some(replaced) = replace_param_placeholders(text, params) {
                *value = replaced;
            }
        }
        serde_yaml::Value::Sequence(items) => {
            for item in items {
                apply_include_params(item, Some(params));
            }
        }
        serde_yaml::Value::Mapping(map) => {
            for (_, item) in map.iter_mut() {
                apply_include_params(item, Some(params));
            }
        }
        _ => {}
    }
}

fn replace_param_placeholders(
    text: &str,
    params: &serde_yaml::Mapping,
) -> Option<serde_yaml::Value> {
    for (key, value) in params {
        let Some(name) = yaml_key_as_string(key) else {
            continue;
        };
        if param_tokens(name).iter().any(|token| token == text) {
            return Some(value.clone());
        }
    }

    let mut replaced = text.to_string();
    let mut changed = false;
    for (key, value) in params {
        let Some(name) = yaml_key_as_string(key) else {
            continue;
        };
        let Some(string_value) = yaml_scalar_to_string(value) else {
            continue;
        };
        for token in param_tokens(name) {
            if replaced.contains(&token) {
                replaced = replaced.replace(&token, &string_value);
                changed = true;
            }
        }
    }

    changed.then_some(serde_yaml::Value::String(replaced))
}

fn param_tokens(name: &str) -> [String; 3] {
    [
        format!("{{{{ params.{name} }}}}"),
        format!("{{{{ param.{name} }}}}"),
        format!("{{{{ include.{name} }}}}"),
    ]
}

fn yaml_scalar_to_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(value) => Some(value.clone()),
        serde_yaml::Value::Bool(value) => Some(value.to_string()),
        serde_yaml::Value::Number(value) => Some(value.to_string()),
        serde_yaml::Value::Null => Some(String::new()),
        _ => None,
    }
}

fn merge_yaml(target: &mut serde_yaml::Value, patch: &serde_yaml::Value) {
    match (target, patch) {
        (serde_yaml::Value::Mapping(target_map), serde_yaml::Value::Mapping(patch_map)) => {
            for (key, value) in patch_map {
                if let Some(existing) = target_map.get_mut(key) {
                    merge_yaml(existing, value);
                } else {
                    target_map.insert(key.clone(), value.clone());
                }
            }
        }
        (target, patch) => *target = patch.clone(),
    }
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
        if step.request.graphql.is_some() && !step.request.method.eq_ignore_ascii_case("POST") {
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
        // Reject steps with both body and form
        if step.request.body.is_some() && step.request.form.is_some() {
            return Err(TarnError::Validation(format!(
                "{}: Step '{}' cannot have both 'body' and 'form' on request",
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
        // Reject form with graphql
        if step.request.form.is_some() && step.request.graphql.is_some() {
            return Err(TarnError::Validation(format!(
                "{}: Step '{}' cannot have both 'form' and 'graphql' on request",
                path.display(),
                step.name
            )));
        }
        // Reject form with multipart
        if step.request.form.is_some() && step.request.multipart.is_some() {
            return Err(TarnError::Validation(format!(
                "{}: Step '{}' cannot have both 'form' and 'multipart' on request",
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

        // Reject `if:` + `unless:` on the same step — their semantics are
        // exact inverses, so mixing them is always a typo / copy-paste
        // bug. Skipping both cases at once yields undefined behavior
        // worth failing loud for.
        if step.run_if.is_some() && step.unless.is_some() {
            return Err(TarnError::Validation(format!(
                "{}: Step '{}' cannot have both 'if' and 'unless' — use one (they are exact inverses)",
                path.display(),
                step.name
            )));
        }

        for (capture_name, spec) in &step.capture {
            let CaptureSpec::Extended(ext) = spec else {
                continue;
            };

            let source_count = usize::from(ext.header.is_some())
                + usize::from(ext.cookie.is_some())
                + usize::from(ext.jsonpath.is_some())
                + usize::from(ext.body.unwrap_or(false))
                + usize::from(ext.status.unwrap_or(false))
                + usize::from(ext.url.unwrap_or(false));

            if source_count != 1 {
                return Err(TarnError::Validation(format!(
                    "{}: Step '{}' capture '{}' must specify exactly one source: header, cookie, jsonpath, body, status, or url",
                    path.display(),
                    step.name,
                    capture_name
                )));
            }

            // `optional: false` while also providing `default:` or
            // `when:` is contradictory — both features exist to make a
            // missing value legal. A `false` here explicitly says
            // "fail on missing", which negates the other two fields
            // and is guaranteed to be a user mistake.
            if matches!(ext.optional, Some(false))
                && (ext.default.is_some() || ext.when.is_some())
            {
                return Err(TarnError::Validation(format!(
                    "{}: Step '{}' capture '{}' has 'optional: false' but also 'default:' or 'when:' — \
                     those fields imply optional capture; either drop 'optional: false' or remove 'default:'/'when:'",
                    path.display(),
                    step.name,
                    capture_name
                )));
            }
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

    fn format_yaml(yaml: &str) -> Result<String, TarnError> {
        format_str(yaml, Path::new("test.tarn.yaml"))
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
    fn format_normalizes_aliases_and_field_order() {
        let formatted = format_yaml(
            r#"
name: Format me
redact:
  replacement: "[redacted]"
  headers: [authorization]
defaults:
  retries: 1
  follow-redirects: true
steps:
  - request:
      url: "http://localhost:3000"
      method: GET
    name: Example
    assert:
      body:
        "$.email":
          not_empty: true
          type: string
          contains: "@"
      status: 200
"#,
        )
        .unwrap();

        assert!(formatted.contains("redaction:\n"));
        assert!(formatted.contains("follow_redirects: true\n"));
        assert!(formatted.contains(
            "name: Example\n  request:\n    method: GET\n    url: http://localhost:3000\n"
        ));

        let type_index = formatted.find("type: string").unwrap();
        let contains_index = formatted.find("contains: '@'").unwrap();
        let not_empty_index = formatted.find("not_empty: true").unwrap();
        assert!(type_index < contains_index);
        assert!(contains_index < not_empty_index);
    }

    #[test]
    fn format_preserves_schema_directive_and_include_directives() {
        let dir = tempfile::tempdir().unwrap();
        let include_path = dir.path().join("auth.tarn.yaml");
        std::fs::write(
            &include_path,
            "name: Auth\nsteps:\n  - name: Login\n    request:\n      method: POST\n      url: http://localhost:3000/auth\n",
        )
        .unwrap();
        let test_path = dir.path().join("suite.tarn.yaml");
        let formatted = format_str(
            r#"# yaml-language-server: $schema=https://raw.githubusercontent.com/NazarKalytiuk/hive/main/schemas/v1/testfile.json
name: With include
steps:
  - include: "./auth.tarn.yaml"
    with:
      tenant: acme
    override:
      request:
        headers:
          X-Tenant: acme
"#,
            &test_path,
        )
        .unwrap();

        assert!(formatted.starts_with(
            "# yaml-language-server: $schema=https://raw.githubusercontent.com/NazarKalytiuk/hive/main/schemas/v1/testfile.json\n"
        ));
        assert!(formatted.contains("- include: ./auth.tarn.yaml"));
        assert!(formatted.contains("with:\n    tenant: acme"));
        assert!(formatted.contains("override:\n    request:\n      headers:"));
        assert!(!formatted.contains("name: Login"));
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
            count >= 18,
            "Expected at least 18 example files, found {count}"
        );
    }

    #[test]
    fn tarn_parser_accepts_all_examples() {
        let examples = glob::glob("../examples/*.tarn.yaml")
            .expect("Failed to glob examples")
            .chain(
                glob::glob("../examples/**/*.tarn.yaml").expect("Failed to glob nested examples"),
            );

        let mut count = 0;
        for entry in examples {
            let path = entry.expect("Failed to read glob entry");
            parse_file(&path)
                .unwrap_or_else(|err| panic!("Failed to parse {}: {}", path.display(), err));
            count += 1;
        }

        assert!(
            count >= 18,
            "Expected at least 18 example files, found {count}"
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
    fn fuzzy_validation_suggests_top_level_field() {
        let result = parse_yaml(
            r#"
name: Typo
step:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
"#,
        );
        let message = result.unwrap_err().to_string();
        assert!(message.contains("Unknown field 'step'"));
        assert!(message.contains("Did you mean 'steps'"));
    }

    #[test]
    fn fuzzy_validation_suggests_nested_request_field() {
        let result = parse_yaml(
            r#"
name: Typo
steps:
  - name: test
    request:
      method: GET
      uri: "http://localhost:3000"
"#,
        );
        let message = result.unwrap_err().to_string();
        assert!(message.contains("root.steps[0].request"));
        assert!(message.contains("Did you mean 'url'"));
    }

    #[test]
    fn fuzzy_validation_suggests_body_operator() {
        let result = parse_yaml(
            r#"
name: Typo
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
    assert:
      body:
        "$.email":
          contians: "@example.com"
"#,
        );
        let message = result.unwrap_err().to_string();
        assert!(message.contains("root.steps[0].assert.body.$.email"));
        assert!(message.contains("Did you mean 'contains'"));
    }

    #[test]
    fn fuzzy_validation_suggests_capture_source_field() {
        let result = parse_yaml(
            r#"
name: Typo
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
    capture:
      token:
        json_path: "$.token"
"#,
        );
        let message = result.unwrap_err().to_string();
        assert!(message.contains("root.steps[0].capture.token"));
        assert!(message.contains("Did you mean 'jsonpath'"));
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

    #[test]
    fn validates_body_form_conflict() {
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
      form:
        email: "test@example.com"
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("cannot have both 'body' and 'form'"));
    }

    #[test]
    fn extended_capture_requires_exactly_one_source() {
        let result = parse_yaml(
            r#"
name: Invalid capture
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
    capture:
      bad_capture:
        header: "x-request-id"
        cookie: "session"
"#,
        );

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must specify exactly one source"));
    }

    #[test]
    fn graphql_accepts_lowercase_post_method() {
        let result = parse_yaml(
            r#"
name: GraphQL
steps:
  - name: query
    request:
      method: post
      url: "http://localhost:3000/graphql"
      graphql:
        query: "{ health }"
"#,
        );

        assert!(result.is_ok());
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
    fn resolve_includes_in_teardown() {
        let dir = tempfile::tempdir().unwrap();

        let shared_content = r#"
name: Shared cleanup
steps:
  - name: Cleanup DB
    request:
      method: POST
      url: "http://localhost:3000/cleanup"
"#;
        std::fs::write(dir.path().join("cleanup.tarn.yaml"), shared_content).unwrap();

        let main_content = r#"
name: Main test
teardown:
  - include: ./cleanup.tarn.yaml
steps:
  - name: Check
    request:
      method: GET
      url: "http://localhost:3000/check"
"#;
        let main_path = dir.path().join("main.tarn.yaml");
        std::fs::write(&main_path, main_content).unwrap();

        let tf = parse_file(&main_path).unwrap();
        assert_eq!(tf.teardown.len(), 1);
        assert_eq!(tf.teardown[0].name, "Cleanup DB");
    }

    #[test]
    fn resolve_includes_in_test_group_steps() {
        let dir = tempfile::tempdir().unwrap();

        let shared_content = r#"
name: Shared steps
steps:
  - name: Shared step A
    request:
      method: GET
      url: "http://localhost:3000/a"
"#;
        std::fs::write(dir.path().join("shared.tarn.yaml"), shared_content).unwrap();

        let main_content = r#"
name: Test groups with includes
tests:
  group_one:
    steps:
      - include: ./shared.tarn.yaml
      - name: Group-specific step
        request:
          method: GET
          url: "http://localhost:3000/b"
"#;
        let main_path = dir.path().join("main.tarn.yaml");
        std::fs::write(&main_path, main_content).unwrap();

        let tf = parse_file(&main_path).unwrap();
        let group = tf.tests.get("group_one").unwrap();
        assert_eq!(group.steps.len(), 2);
        assert_eq!(group.steps[0].name, "Shared step A");
        assert_eq!(group.steps[1].name, "Group-specific step");
    }

    #[test]
    fn resolve_includes_in_steps_array() {
        let dir = tempfile::tempdir().unwrap();

        let shared_content = r#"
name: Shared check
steps:
  - name: Health check
    request:
      method: GET
      url: "http://localhost:3000/health"
"#;
        std::fs::write(dir.path().join("health.tarn.yaml"), shared_content).unwrap();

        let main_content = r#"
name: Main with included steps
steps:
  - include: ./health.tarn.yaml
  - name: Custom step
    request:
      method: GET
      url: "http://localhost:3000/custom"
"#;
        let main_path = dir.path().join("main.tarn.yaml");
        std::fs::write(&main_path, main_content).unwrap();

        let tf = parse_file(&main_path).unwrap();
        assert_eq!(tf.steps.len(), 2);
        assert_eq!(tf.steps[0].name, "Health check");
        assert_eq!(tf.steps[1].name, "Custom step");
    }

    #[test]
    fn resolve_includes_extracts_setup_and_steps() {
        let dir = tempfile::tempdir().unwrap();

        // Included file has both setup and steps
        let shared_content = r#"
name: Shared auth
setup:
  - name: Pre-auth
    request:
      method: POST
      url: "http://localhost:3000/pre"
steps:
  - name: Login
    request:
      method: POST
      url: "http://localhost:3000/login"
"#;
        std::fs::write(dir.path().join("auth.tarn.yaml"), shared_content).unwrap();

        let main_content = r#"
name: Main test
setup:
  - include: ./auth.tarn.yaml
steps:
  - name: Check
    request:
      method: GET
      url: "http://localhost:3000/check"
"#;
        let main_path = dir.path().join("main.tarn.yaml");
        std::fs::write(&main_path, main_content).unwrap();

        let tf = parse_file(&main_path).unwrap();
        // Both setup and steps from the included file should be inlined
        assert_eq!(tf.setup.len(), 2);
        assert_eq!(tf.setup[0].name, "Pre-auth");
        assert_eq!(tf.setup[1].name, "Login");
    }

    #[test]
    fn resolve_includes_nested() {
        let dir = tempfile::tempdir().unwrap();

        // Level 2: deepest include
        let deep_content = r#"
name: Deep shared
steps:
  - name: Deep step
    request:
      method: GET
      url: "http://localhost:3000/deep"
"#;
        std::fs::write(dir.path().join("deep.tarn.yaml"), deep_content).unwrap();

        // Level 1: includes the deep file
        let mid_content = r#"
name: Mid shared
steps:
  - include: ./deep.tarn.yaml
  - name: Mid step
    request:
      method: GET
      url: "http://localhost:3000/mid"
"#;
        std::fs::write(dir.path().join("mid.tarn.yaml"), mid_content).unwrap();

        // Level 0: includes the mid file
        let main_content = r#"
name: Main test
setup:
  - include: ./mid.tarn.yaml
steps:
  - name: Main step
    request:
      method: GET
      url: "http://localhost:3000/main"
"#;
        let main_path = dir.path().join("main.tarn.yaml");
        std::fs::write(&main_path, main_content).unwrap();

        let tf = parse_file(&main_path).unwrap();
        // mid includes deep, so setup should have: Deep step, Mid step
        assert_eq!(tf.setup.len(), 2);
        assert_eq!(tf.setup[0].name, "Deep step");
        assert_eq!(tf.setup[1].name, "Mid step");
    }

    #[test]
    fn resolve_includes_apply_params_and_overrides() {
        let dir = tempfile::tempdir().unwrap();

        let shared_content = r#"
name: Shared
steps:
  - name: Tenant request
    request:
      method: GET
      url: "http://localhost:3000/{{ params.tenant }}/users/{{ params.user_id }}"
      headers:
        X-Base: "base"
"#;
        std::fs::write(dir.path().join("shared.tarn.yaml"), shared_content).unwrap();

        let main_content = r#"
name: Main
steps:
  - include: ./shared.tarn.yaml
    with:
      tenant: acme
      user_id: 42
    override:
      request:
        headers:
          X-Trace: "trace-1"
"#;
        let main_path = dir.path().join("main.tarn.yaml");
        std::fs::write(&main_path, main_content).unwrap();

        let tf = parse_file(&main_path).unwrap();
        assert_eq!(tf.steps.len(), 1);
        let step = &tf.steps[0];
        assert_eq!(step.request.url, "http://localhost:3000/acme/users/42");
        assert_eq!(
            step.request.headers.get("X-Base").map(String::as_str),
            Some("base")
        );
        assert_eq!(
            step.request.headers.get("X-Trace").map(String::as_str),
            Some("trace-1")
        );
    }

    #[test]
    fn resolve_includes_can_replace_exact_value_with_object_param() {
        let dir = tempfile::tempdir().unwrap();

        let shared_content = r#"
name: Shared
steps:
  - name: Create
    request:
      method: POST
      url: "http://localhost:3000/users"
      body: "{{ params.payload }}"
"#;
        std::fs::write(dir.path().join("shared.tarn.yaml"), shared_content).unwrap();

        let main_content = r#"
name: Main
steps:
  - include: ./shared.tarn.yaml
    with:
      payload:
        name: "Jane"
        role: "admin"
"#;
        let main_path = dir.path().join("main.tarn.yaml");
        std::fs::write(&main_path, main_content).unwrap();

        let tf = parse_file(&main_path).unwrap();
        assert_eq!(tf.steps.len(), 1);
        let body = tf.steps[0].request.body.as_ref().unwrap();
        assert_eq!(body["name"], "Jane");
        assert_eq!(body["role"], "admin");
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

    // --- NAZ-242: parser validation for new fields ---

    #[test]
    fn parser_rejects_if_plus_unless_on_same_step() {
        // Exact inverses — accepting both is always a user mistake
        // with undefined semantics. Validation must stop the run at
        // parse time rather than silently running or silently
        // skipping.
        let yaml = r#"
name: Conflict
steps:
  - name: confused
    if: "{{ capture.a }}"
    unless: "{{ capture.b }}"
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let err = parse_yaml(yaml).unwrap_err().to_string();
        assert!(
            err.contains("cannot have both 'if' and 'unless'"),
            "got: {err}"
        );
    }

    #[test]
    fn parser_rejects_optional_false_plus_default() {
        // `optional: false` contradicts `default:` — either drop one
        // or the user intent is ambiguous. Validation catches it.
        let yaml = r#"
name: Contradiction
steps:
  - name: step
    request:
      method: GET
      url: "http://localhost:3000"
    capture:
      x:
        jsonpath: "$.x"
        optional: false
        default: 0
"#;
        let err = parse_yaml(yaml).unwrap_err().to_string();
        assert!(
            err.contains("optional: false") && err.contains("default"),
            "got: {err}"
        );
    }

    #[test]
    fn parser_rejects_optional_false_plus_when() {
        // Same class of contradiction as optional+default.
        let yaml = r#"
name: Contradiction
steps:
  - name: step
    request:
      method: GET
      url: "http://localhost:3000"
    capture:
      x:
        jsonpath: "$.x"
        optional: false
        when:
          status: 200
"#;
        let err = parse_yaml(yaml).unwrap_err().to_string();
        assert!(
            err.contains("optional: false") && err.contains("when"),
            "got: {err}"
        );
    }

    #[test]
    fn parser_rejects_unknown_field_under_capture_when() {
        // Guard against typos silently making the gate pass on every
        // response. `statuus:` (typo) must fail validation.
        let yaml = r#"
name: Typo
steps:
  - name: step
    request:
      method: GET
      url: "http://localhost:3000"
    capture:
      x:
        jsonpath: "$.x"
        when:
          statuus: 200
"#;
        let err = parse_yaml(yaml).unwrap_err().to_string();
        assert!(
            err.contains("statuus") || err.contains("unknown"),
            "got: {err}"
        );
    }

    #[test]
    fn parser_accepts_where_inside_extended_capture() {
        // `where:` was previously missing from the parser's allowed
        // keys for extended captures (pre-existing gap surfaced by
        // NAZ-242). Locking in the fix so future field-list edits
        // don't regress array-filter captures.
        let yaml = r#"
name: Where
steps:
  - name: step
    request:
      method: GET
      url: "http://localhost:3000"
    capture:
      admins:
        jsonpath: "$.users"
        where:
          role: admin
"#;
        assert!(parse_yaml(yaml).is_ok());
    }

    #[test]
    fn parser_accepts_full_feature_combination() {
        // Whole-stack smoke test: optional + default + when on a
        // capture plus if: on a step must all parse together.
        let yaml = r#"
name: Combined
steps:
  - name: conditional
    if: "{{ capture.do_it }}"
    request:
      method: GET
      url: "http://localhost:3000"
    capture:
      id:
        jsonpath: "$.id"
        optional: true
        default: ""
        when:
          status:
            in: [200, 201]
"#;
        let tf = parse_yaml(yaml).expect("should parse");
        assert!(tf.steps[0].run_if.is_some());
        let spec = tf.steps[0].capture.get("id").unwrap();
        match spec {
            CaptureSpec::Extended(ext) => {
                assert_eq!(ext.optional, Some(true));
                assert!(ext.default.is_some());
                assert!(ext.when.is_some());
            }
            other => panic!("expected Extended, got {:?}", other),
        }
    }
}
