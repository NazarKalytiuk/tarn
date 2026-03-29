use crate::assert;
use crate::assert::types::{
    AssertionResult, FailureCategory, FileResult, RequestInfo, ResponseInfo, StepResult, TestResult,
};
use crate::capture;
use crate::cookie::CookieJar;
use crate::error::TarnError;
use crate::http;
use crate::interpolation::{self, Context};
use crate::model::{Assertion, PollConfig, Step, TestFile};
use crate::scripting;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

/// Options controlling how tests are run.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Print full request/response for every step
    pub verbose: bool,
    /// Show interpolated requests without sending
    pub dry_run: bool,
}

/// Check if a file or test matches tag filter (AND logic: all tags must be present).
pub fn matches_tags(item_tags: &[String], filter_tags: &[String]) -> bool {
    if filter_tags.is_empty() {
        return true;
    }
    filter_tags.iter().all(|ft| item_tags.contains(ft))
}

/// Parse tag filter string (comma-separated) into a list.
pub fn parse_tag_filter(tag_str: &str) -> Vec<String> {
    tag_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Run a single test file and return results.
pub fn run_file(
    test_file: &TestFile,
    file_path: &str,
    env: &HashMap<String, String>,
    tag_filter: &[String],
    opts: &RunOptions,
) -> Result<FileResult, TarnError> {
    let start = Instant::now();

    // Check if file-level tags match filter
    if !tag_filter.is_empty()
        && !test_file.steps.is_empty()
        && !matches_tags(&test_file.tags, tag_filter)
    {
        // Simple format files: check file-level tags
        return Ok(FileResult {
            file: file_path.to_string(),
            name: test_file.name.clone(),
            passed: true,
            duration_ms: 0,
            setup_results: vec![],
            test_results: vec![],
            teardown_results: vec![],
        });
    }

    // Build interpolation context with resolved env
    let mut captures: HashMap<String, serde_json::Value> = HashMap::new();

    // Cookie jar: enabled by default, disabled with `cookies: "off"`
    let cookies_enabled = test_file
        .cookies
        .as_deref()
        .map(|c| c != "off")
        .unwrap_or(true);
    let mut cookie_jar = if cookies_enabled {
        Some(CookieJar::new())
    } else {
        None
    };

    // Resolve base directory for file paths (e.g., multipart file references)
    let base_dir = Path::new(file_path)
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    // Run setup steps
    let setup_results = run_steps(
        &test_file.setup,
        env,
        &mut captures,
        test_file,
        opts,
        cookie_jar.as_mut(),
        &base_dir,
    )?;
    let setup_failed = setup_results.iter().any(|s| !s.passed);

    let mut test_results = Vec::new();

    if !setup_failed {
        if !test_file.steps.is_empty() {
            // Simple format: flat steps
            let mut step_captures = captures.clone();
            let step_results = run_steps(
                &test_file.steps,
                env,
                &mut step_captures,
                test_file,
                opts,
                cookie_jar.as_mut(),
                &base_dir,
            )?;
            let passed = step_results.iter().all(|s| s.passed);
            let duration_ms = step_results.iter().map(|s| s.duration_ms).sum();
            test_results.push(TestResult {
                name: test_file.name.clone(),
                description: test_file.description.clone(),
                passed,
                duration_ms,
                step_results,
            });
        }

        // Full format: named test groups (with tag filtering)
        for (name, test_group) in &test_file.tests {
            // Skip test groups that don't match tag filter
            // Check both file-level and test-level tags
            if !tag_filter.is_empty() {
                let combined_tags: Vec<String> = test_file
                    .tags
                    .iter()
                    .chain(test_group.tags.iter())
                    .cloned()
                    .collect();
                if !matches_tags(&combined_tags, tag_filter) {
                    continue;
                }
            }

            let mut test_captures = captures.clone();
            let step_results = run_steps(
                &test_group.steps,
                env,
                &mut test_captures,
                test_file,
                opts,
                cookie_jar.as_mut(),
                &base_dir,
            )?;
            let passed = step_results.iter().all(|s| s.passed);
            let duration_ms = step_results.iter().map(|s| s.duration_ms).sum();
            test_results.push(TestResult {
                name: name.clone(),
                description: test_group.description.clone(),
                passed,
                duration_ms,
                step_results,
            });
        }
    }

    // Run teardown steps (always, even on failure)
    let teardown_results = run_steps(
        &test_file.teardown,
        env,
        &mut captures,
        test_file,
        opts,
        cookie_jar.as_mut(),
        &base_dir,
    )?;

    let all_passed = !setup_failed
        && test_results.iter().all(|t| t.passed)
        && teardown_results.iter().all(|s| s.passed);

    Ok(FileResult {
        file: file_path.to_string(),
        name: test_file.name.clone(),
        passed: all_passed,
        duration_ms: start.elapsed().as_millis() as u64,
        setup_results,
        test_results,
        teardown_results,
    })
}

/// Run a sequence of steps, accumulating captures.
fn run_steps(
    steps: &[Step],
    env: &HashMap<String, String>,
    captures: &mut HashMap<String, serde_json::Value>,
    test_file: &TestFile,
    opts: &RunOptions,
    cookie_jar: Option<&mut CookieJar>,
    base_dir: &Path,
) -> Result<Vec<StepResult>, TarnError> {
    let mut results = Vec::new();

    // We need to reborrow cookie_jar on each iteration
    let jar_cell = std::cell::RefCell::new(cookie_jar);

    for step in steps {
        let result = run_step(
            step,
            env,
            captures,
            test_file,
            opts,
            jar_cell.borrow_mut().as_deref_mut(),
            base_dir,
        )?;
        results.push(result);
    }

    Ok(results)
}

/// Parse a delay spec like "500ms" or "2s" into milliseconds.
fn parse_delay(spec: &str) -> Option<u64> {
    let spec = spec.trim();
    if let Some(ms) = spec.strip_suffix("ms") {
        ms.trim().parse().ok()
    } else if let Some(s) = spec.strip_suffix('s') {
        s.trim().parse::<u64>().ok().map(|v| v * 1000)
    } else {
        spec.parse().ok()
    }
}

/// Prepare interpolated request parts from a step.
fn prepare_request(
    step: &Step,
    env: &HashMap<String, String>,
    captures: &HashMap<String, serde_json::Value>,
    test_file: &TestFile,
    cookie_jar: Option<&CookieJar>,
) -> (
    String,
    HashMap<String, String>,
    Option<serde_json::Value>,
    Option<u64>,
    Context,
) {
    let ctx = Context {
        env: env.clone(),
        captures: captures.clone(),
    };

    let url = interpolation::interpolate(&step.request.url, &ctx);

    let mut merged_headers = test_file
        .defaults
        .as_ref()
        .map(|d| d.headers.clone())
        .unwrap_or_default();
    for (k, v) in &step.request.headers {
        merged_headers.insert(k.clone(), v.clone());
    }

    // Inject cookies from jar (if not already set by the user)
    if let Some(jar) = cookie_jar {
        if !merged_headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("cookie"))
        {
            if let Some(cookie_header) = jar.cookie_header() {
                merged_headers.insert("Cookie".to_string(), cookie_header);
            }
        }
    }

    // GraphQL: build body from graphql block and auto-set Content-Type
    let body = if let Some(ref gql) = step.request.graphql {
        let mut gql_body = serde_json::json!({
            "query": interpolation::interpolate(&gql.query, &ctx),
        });
        if let Some(ref vars) = gql.variables {
            gql_body["variables"] = interpolation::interpolate_json(vars, &ctx);
        }
        if let Some(ref op) = gql.operation_name {
            gql_body["operationName"] =
                serde_json::Value::String(interpolation::interpolate(op, &ctx));
        }
        if !merged_headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("content-type"))
        {
            merged_headers.insert("Content-Type".to_string(), "application/json".to_string());
        }
        Some(gql_body)
    } else {
        step.request
            .body
            .as_ref()
            .map(|b| interpolation::interpolate_json(b, &ctx))
    };

    let headers = interpolation::interpolate_headers(&merged_headers, &ctx);
    let timeout = step
        .timeout
        .or_else(|| test_file.defaults.as_ref().and_then(|d| d.timeout));

    (url, headers, body, timeout, ctx)
}

/// Run a single step: interpolate, execute HTTP request, run assertions, extract captures.
/// Supports retries, polling, GraphQL, multipart, Lua scripts, step-level timeout, delay, verbose, dry-run.
/// Capture failures are handled gracefully — the step is marked as failed instead of aborting the run.
fn run_step(
    step: &Step,
    env: &HashMap<String, String>,
    captures: &mut HashMap<String, serde_json::Value>,
    test_file: &TestFile,
    opts: &RunOptions,
    mut cookie_jar: Option<&mut CookieJar>,
    base_dir: &Path,
) -> Result<StepResult, TarnError> {
    // Apply delay: step-level > defaults > none
    let delay_spec = step
        .delay
        .as_ref()
        .or_else(|| test_file.defaults.as_ref().and_then(|d| d.delay.as_ref()));
    if let Some(delay_spec) = delay_spec {
        if let Some(delay_ms) = parse_delay(delay_spec) {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }
    }

    // Dispatch to poll mode if configured
    if let Some(ref poll) = step.poll {
        return run_step_poll(step, poll, env, captures, test_file, opts, cookie_jar, base_dir);
    }

    let (url, headers, body, timeout, ctx) =
        prepare_request(step, env, captures, test_file, cookie_jar.as_deref());

    // Verbose: print request details
    if opts.verbose {
        eprintln!(
            "  --> {} {} (timeout: {})",
            step.request.method,
            url,
            timeout.map(|t| format!("{}ms", t)).unwrap_or("none".into())
        );
    }

    // Dry-run: show what would be sent, return a pass result
    if opts.dry_run {
        eprintln!("  [dry-run] {} {} {}", step.name, step.request.method, url);
        return Ok(StepResult {
            name: step.name.clone(),
            passed: true,
            duration_ms: 0,
            assertion_results: vec![],
            request_info: Some(RequestInfo {
                method: step.request.method.clone(),
                url,
                headers,
                body,
            }),
            response_info: None,
            error_category: None,
        });
    }

    // Resolve retries: step-level > defaults > 0
    let max_retries = step
        .retries
        .or_else(|| test_file.defaults.as_ref().and_then(|d| d.retries))
        .unwrap_or(0);

    // Execute with retries
    let mut last_result = None;
    for attempt in 0..=max_retries {
        // Choose between multipart and regular request
        let response = if let Some(ref multipart) = step.request.multipart {
            http::execute_multipart_request(
                &step.request.method,
                &url,
                &headers,
                multipart,
                timeout,
                base_dir,
            )?
        } else {
            http::execute_request(&step.request.method, &url, &headers, body.as_ref(), timeout)?
        };

        // Capture cookies from response
        if let Some(ref mut jar) = cookie_jar.as_deref_mut() {
            jar.capture_from_response(&response.raw_headers);
        }

        if opts.verbose {
            eprintln!("  <-- {} ({}ms)", response.status, response.duration_ms);
            if max_retries > 0 && attempt > 0 {
                eprintln!("      (retry {}/{})", attempt, max_retries);
            }
        }

        let assertion_results = if let Some(ref assertion) = step.assertions {
            let interpolated = interpolate_assertion(assertion, &ctx);
            assert::run_assertions(
                &interpolated,
                response.status,
                &response.headers,
                &response.body,
                response.duration_ms,
            )
        } else {
            vec![]
        };

        let passed = assertion_results.iter().all(|a| a.passed);

        if passed {
            // Extract captures on success — graceful failure (P0 fix)
            let capture_result = if !step.capture.is_empty() {
                match capture::extract_captures(&response.body, &response.headers, &step.capture) {
                    Ok(new_captures) => {
                        captures.extend(new_captures);
                        None
                    }
                    Err(e) => Some(e),
                }
            } else {
                None
            };

            // If capture failed, mark step as failed instead of aborting
            if let Some(capture_err) = capture_result {
                let mut all_assertions = assertion_results;
                all_assertions.push(AssertionResult::fail(
                    "capture",
                    "successful extraction",
                    "extraction failed",
                    format!("{}", capture_err),
                ));
                return Ok(StepResult {
                    name: step.name.clone(),
                    passed: false,
                    duration_ms: response.duration_ms,
                    assertion_results: all_assertions,
                    request_info: Some(RequestInfo {
                        method: step.request.method.clone(),
                        url,
                        headers,
                        body,
                    }),
                    response_info: Some(ResponseInfo {
                        status: response.status,
                        headers: response.headers,
                        body: Some(response.body),
                    }),
                    error_category: Some(FailureCategory::CaptureError),
                });
            }

            // Run Lua script if present
            let (all_assertions, all_passed) =
                run_script_if_present(step, &response, captures, assertion_results)?;

            return Ok(StepResult {
                name: step.name.clone(),
                passed: all_passed,
                duration_ms: response.duration_ms,
                assertion_results: all_assertions,
                request_info: None,
                response_info: None,
                error_category: None,
            });
        }

        last_result = Some((response, assertion_results, body.clone()));

        if attempt < max_retries {
            std::thread::sleep(std::time::Duration::from_millis(100 * (attempt as u64 + 1)));
        }
    }

    // All attempts failed
    let (response, assertion_results, body) = last_result.unwrap();

    Ok(StepResult {
        name: step.name.clone(),
        passed: false,
        duration_ms: response.duration_ms,
        assertion_results,
        request_info: Some(RequestInfo {
            method: step.request.method.clone(),
            url,
            headers,
            body,
        }),
        response_info: Some(ResponseInfo {
            status: response.status,
            headers: response.headers,
            body: Some(response.body),
        }),
        error_category: Some(FailureCategory::AssertionFailed),
    })
}

/// Execute a step with polling: re-execute until `poll.until` assertions pass.
#[allow(clippy::too_many_arguments)]
fn run_step_poll(
    step: &Step,
    poll: &PollConfig,
    env: &HashMap<String, String>,
    captures: &mut HashMap<String, serde_json::Value>,
    test_file: &TestFile,
    opts: &RunOptions,
    mut cookie_jar: Option<&mut CookieJar>,
    base_dir: &Path,
) -> Result<StepResult, TarnError> {
    let interval_ms = parse_delay(&poll.interval).unwrap_or(1000);

    for attempt in 0..poll.max_attempts {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        }

        let (url, headers, body, timeout, ctx) =
            prepare_request(step, env, captures, test_file, cookie_jar.as_deref());

        if opts.verbose {
            eprintln!(
                "  [poll {}/{}] {} {}",
                attempt + 1,
                poll.max_attempts,
                step.request.method,
                url
            );
        }

        // Choose between multipart and regular request
        let response = if let Some(ref multipart) = step.request.multipart {
            http::execute_multipart_request(
                &step.request.method,
                &url,
                &headers,
                multipart,
                timeout,
                base_dir,
            )?
        } else {
            http::execute_request(&step.request.method, &url, &headers, body.as_ref(), timeout)?
        };

        // Capture cookies
        if let Some(ref mut jar) = cookie_jar.as_deref_mut() {
            jar.capture_from_response(&response.raw_headers);
        }

        // Check poll.until condition
        let until_interpolated = interpolate_assertion(&poll.until, &ctx);
        let until_results = assert::run_assertions(
            &until_interpolated,
            response.status,
            &response.headers,
            &response.body,
            response.duration_ms,
        );
        let until_passed = until_results.iter().all(|a| a.passed);

        if until_passed {
            // Condition met — run the step's own assertions
            let assertion_results = if let Some(ref assertion) = step.assertions {
                let interpolated = interpolate_assertion(assertion, &ctx);
                assert::run_assertions(
                    &interpolated,
                    response.status,
                    &response.headers,
                    &response.body,
                    response.duration_ms,
                )
            } else {
                vec![]
            };

            let passed = assertion_results.iter().all(|a| a.passed);

            // Extract captures — graceful failure
            if passed && !step.capture.is_empty() {
                match capture::extract_captures(&response.body, &response.headers, &step.capture) {
                    Ok(new_captures) => {
                        captures.extend(new_captures);
                    }
                    Err(e) => {
                        let mut all_assertions = assertion_results;
                        all_assertions.push(AssertionResult::fail(
                            "capture",
                            "successful extraction",
                            "extraction failed",
                            format!("{}", e),
                        ));
                        return Ok(StepResult {
                            name: step.name.clone(),
                            passed: false,
                            duration_ms: response.duration_ms,
                            assertion_results: all_assertions,
                            request_info: None,
                            response_info: Some(ResponseInfo {
                                status: response.status,
                                headers: response.headers,
                                body: Some(response.body),
                            }),
                            error_category: Some(FailureCategory::CaptureError),
                        });
                    }
                }
            }

            // Run Lua script if present
            let (all_assertions, all_passed) =
                run_script_if_present(step, &response, captures, assertion_results)?;

            return Ok(StepResult {
                name: step.name.clone(),
                passed: all_passed,
                duration_ms: response.duration_ms,
                assertion_results: all_assertions,
                request_info: None,
                response_info: None,
                error_category: None,
            });
        }
    }

    // Polling timed out
    Ok(StepResult {
        name: step.name.clone(),
        passed: false,
        duration_ms: 0,
        assertion_results: vec![AssertionResult::fail(
            "poll",
            "condition met",
            format!("not met after {} attempts", poll.max_attempts),
            format!(
                "Polling timed out after {} attempts (interval: {})",
                poll.max_attempts, poll.interval
            ),
        )],
        request_info: None,
        response_info: None,
        error_category: Some(FailureCategory::Timeout),
    })
}

/// Run Lua script after HTTP step if `script:` field is present.
/// Returns combined assertion results and overall pass/fail.
fn run_script_if_present(
    step: &Step,
    response: &http::HttpResponse,
    captures: &mut HashMap<String, serde_json::Value>,
    mut assertion_results: Vec<AssertionResult>,
) -> Result<(Vec<AssertionResult>, bool), TarnError> {
    if let Some(ref script) = step.script {
        let script_result = scripting::run_script(script, response, captures, &step.name)?;
        captures.extend(script_result.captures);
        assertion_results.extend(script_result.assertion_results);
    }
    let passed = assertion_results.iter().all(|a| a.passed);
    Ok((assertion_results, passed))
}

/// Interpolate template expressions within assertion values.
fn interpolate_assertion(assertion: &Assertion, ctx: &Context) -> Assertion {
    let mut result = assertion.clone();

    // Interpolate header assertion values
    if let Some(ref headers) = assertion.headers {
        result.headers = Some(
            headers
                .iter()
                .map(|(k, v)| (k.clone(), interpolation::interpolate(v, ctx)))
                .collect(),
        );
    }

    // Interpolate body assertion values (YAML values containing {{ }})
    if let Some(ref body) = assertion.body {
        let interpolated: indexmap::IndexMap<String, serde_yaml::Value> = body
            .iter()
            .map(|(k, v)| (k.clone(), interpolate_yaml_value(v, ctx)))
            .collect();
        result.body = Some(interpolated);
    }

    // Interpolate duration spec
    if let Some(ref duration) = assertion.duration {
        result.duration = Some(interpolation::interpolate(duration, ctx));
    }

    result
}

/// Interpolate `{{ }}` templates within a serde_yaml::Value.
fn interpolate_yaml_value(value: &serde_yaml::Value, ctx: &Context) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::String(s) => {
            serde_yaml::Value::String(interpolation::interpolate(s, ctx))
        }
        serde_yaml::Value::Mapping(map) => {
            let new_map: serde_yaml::Mapping = map
                .iter()
                .map(|(k, v)| (k.clone(), interpolate_yaml_value(v, ctx)))
                .collect();
            serde_yaml::Value::Mapping(new_map)
        }
        serde_yaml::Value::Sequence(seq) => serde_yaml::Value::Sequence(
            seq.iter().map(|v| interpolate_yaml_value(v, ctx)).collect(),
        ),
        other => other.clone(),
    }
}

/// Discover test files in a directory matching *.tarn.yaml pattern.
pub fn discover_test_files(dir: &Path) -> Result<Vec<String>, TarnError> {
    let pattern = format!("{}/**/*.tarn.yaml", dir.display());
    let mut files: Vec<String> = glob::glob(&pattern)
        .map_err(|e| TarnError::Config(format!("Invalid glob pattern: {}", e)))?
        .filter_map(|entry| entry.ok())
        .map(|path| path.display().to_string())
        .collect();
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_test_files_in_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        std::fs::write(dir.path().join("a.tarn.yaml"), "").unwrap();
        std::fs::write(sub.join("b.tarn.yaml"), "").unwrap();
        std::fs::write(dir.path().join("not_a_test.yaml"), "").unwrap();

        let files = discover_test_files(dir.path()).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files[0].ends_with("a.tarn.yaml"));
        assert!(files[1].ends_with("b.tarn.yaml"));
    }

    #[test]
    fn discover_test_files_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let files = discover_test_files(dir.path()).unwrap();
        assert!(files.is_empty());
    }

    // --- Tag filtering ---

    #[test]
    fn matches_tags_empty_filter_matches_all() {
        assert!(matches_tags(&["a".into(), "b".into()], &[]));
        assert!(matches_tags(&[], &[]));
    }

    #[test]
    fn matches_tags_single_tag() {
        assert!(matches_tags(
            &["smoke".into(), "crud".into()],
            &["smoke".into()]
        ));
    }

    #[test]
    fn matches_tags_and_logic() {
        assert!(matches_tags(
            &["smoke".into(), "crud".into(), "users".into()],
            &["smoke".into(), "crud".into()]
        ));
    }

    #[test]
    fn matches_tags_missing_tag() {
        assert!(!matches_tags(&["smoke".into()], &["crud".into()]));
    }

    #[test]
    fn matches_tags_partial_match_fails() {
        assert!(!matches_tags(
            &["smoke".into()],
            &["smoke".into(), "crud".into()]
        ));
    }

    #[test]
    fn parse_tag_filter_single() {
        assert_eq!(parse_tag_filter("smoke"), vec!["smoke"]);
    }

    #[test]
    fn parse_tag_filter_multiple() {
        assert_eq!(parse_tag_filter("crud,users"), vec!["crud", "users"]);
    }

    #[test]
    fn parse_tag_filter_with_spaces() {
        assert_eq!(
            parse_tag_filter("crud , users , smoke"),
            vec!["crud", "users", "smoke"]
        );
    }

    #[test]
    fn parse_tag_filter_empty() {
        let result = parse_tag_filter("");
        assert!(result.is_empty());
    }

    // --- Delay parsing ---

    #[test]
    fn parse_delay_milliseconds() {
        assert_eq!(parse_delay("500ms"), Some(500));
    }

    #[test]
    fn parse_delay_seconds() {
        assert_eq!(parse_delay("2s"), Some(2000));
    }

    #[test]
    fn parse_delay_plain_number() {
        assert_eq!(parse_delay("100"), Some(100));
    }

    #[test]
    fn parse_delay_with_whitespace() {
        assert_eq!(parse_delay("  300ms  "), Some(300));
    }

    #[test]
    fn parse_delay_invalid() {
        assert_eq!(parse_delay("abc"), None);
    }

    // --- Model deserializes new fields ---

    #[test]
    fn step_with_retries_and_timeout() {
        let yaml = r#"
name: Retry test
steps:
  - name: Flaky endpoint
    request:
      method: GET
      url: "http://localhost:3000/flaky"
    retries: 3
    timeout: 2000
    delay: "500ms"
    assert:
      status: 200
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let step = &tf.steps[0];
        assert_eq!(step.retries, Some(3));
        assert_eq!(step.timeout, Some(2000));
        assert_eq!(step.delay, Some("500ms".to_string()));
    }

    #[test]
    fn defaults_with_retries() {
        let yaml = r#"
name: Default retries
defaults:
  retries: 2
  timeout: 5000
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let defaults = tf.defaults.unwrap();
        assert_eq!(defaults.retries, Some(2));
        assert_eq!(defaults.timeout, Some(5000));
    }

    #[test]
    fn step_without_new_fields_defaults_to_none() {
        let yaml = r#"
name: Basic
steps:
  - name: simple
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let step = &tf.steps[0];
        assert_eq!(step.retries, None);
        assert_eq!(step.timeout, None);
        assert_eq!(step.delay, None);
    }
}
