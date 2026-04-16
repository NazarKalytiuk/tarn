use crate::assert;
use crate::assert::types::{
    AssertionResult, FailureCategory, FileResult, RequestInfo, ResponseInfo, StepResult, TestResult,
};
use crate::capture;
use crate::cookie::CookieJar;
use crate::error::TarnError;
use crate::http;
use crate::interpolation::{self, Context};
use crate::model::{
    Assertion, AuthConfig, CookieMode, HttpTransportConfig, PollConfig, RedactionConfig, Step,
    StepCookies, TestFile,
};
use crate::parser;
use crate::report::progress::{ProgressReporter, ReportContext};
use crate::scripting;
use crate::selector::{self, Selector};
use base64::Engine;
use indexmap::IndexMap;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Options controlling how tests are run.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Print full request/response for every step
    pub verbose: bool,
    /// Show interpolated requests without sending
    pub dry_run: bool,
    /// Runtime HTTP transport settings.
    pub http: HttpTransportConfig,
    /// CLI override that forces per-test cookie jar isolation regardless
    /// of the file's declared `cookies:` mode. Used by the
    /// `--cookie-jar-per-test` flag and the VS Code extension's subset runs.
    pub cookie_jar_per_test: bool,
    /// When true, any step failure inside a test marks every later
    /// step in the same test as `SkippedDueToFailFast` instead of
    /// running it. Keeps reports short when the first failure already
    /// tells the whole story (auth break, schema mismatch, etc.).
    pub fail_fast_within_test: bool,
}

/// Name of the default cookie jar used when no `cookies: <name>` is set on a step.
const DEFAULT_JAR_NAME: &str = "default";

/// Resolve the effective cookie mode for a file run. CLI override wins over
/// the file's declared mode. `Off` always wins over per-test because "off"
/// means no jar activity at all, and there is nothing to reset.
fn effective_cookie_mode(declared: Option<CookieMode>, cli_per_test: bool) -> CookieMode {
    let base = declared.unwrap_or_default();
    if base == CookieMode::Off {
        return CookieMode::Off;
    }
    if cli_per_test {
        return CookieMode::PerTest;
    }
    base
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
    let mut cookie_jars = HashMap::new();
    run_file_with_cookie_jars(
        test_file,
        file_path,
        env,
        tag_filter,
        &[],
        opts,
        &mut cookie_jars,
        None,
    )
}

/// Run a single test file with an externally managed cookie jar set.
///
/// When `selectors` is non-empty, only tests and steps matching at least
/// one selector run. Setup and teardown always run for a file that has
/// any matching work, so captures and cleanup behave consistently.
#[allow(clippy::too_many_arguments)]
pub fn run_file_with_cookie_jars(
    test_file: &TestFile,
    file_path: &str,
    env: &HashMap<String, String>,
    tag_filter: &[String],
    selectors: &[Selector],
    opts: &RunOptions,
    cookie_jars: &mut HashMap<String, CookieJar>,
    progress: Option<&(dyn ProgressReporter + Send + Sync)>,
) -> Result<FileResult, TarnError> {
    let start = Instant::now();
    let client = http::HttpClient::new(&opts.http)?;
    let redaction = test_file.redaction.clone().unwrap_or_default();
    let mut redacted_values = collect_redacted_env_values(env, &redaction);

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
            redaction,
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![],
            teardown_results: vec![],
        });
    }

    // Skip the whole file when selectors exclude it.
    if !selector::any_matches_file(selectors, file_path) {
        return Ok(FileResult {
            file: file_path.to_string(),
            name: test_file.name.clone(),
            passed: true,
            duration_ms: 0,
            redaction,
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![],
            teardown_results: vec![],
        });
    }

    // Build interpolation context with resolved env
    let mut captures: HashMap<String, serde_json::Value> = HashMap::new();

    // Cookie jars: enabled by default, disabled with `cookies: "off"`.
    // `per-test` (file-level) or `--cookie-jar-per-test` (CLI override) clear
    // the default jar between named tests; setup/teardown still share it.
    let cookie_mode = effective_cookie_mode(test_file.cookies, opts.cookie_jar_per_test);
    let cookies_enabled = cookie_mode != CookieMode::Off;

    // Resolve base directory for file paths (e.g., multipart file references)
    let base_dir = Path::new(file_path)
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    if let Some(p) = progress {
        p.file_started(file_path, &test_file.name);
    }

    // Run setup steps
    let setup_results = run_steps(
        &test_file.setup,
        env,
        &mut captures,
        test_file,
        &redaction,
        &mut redacted_values,
        &client,
        opts,
        cookies_enabled,
        cookie_jars,
        &base_dir,
    )?;
    let setup_failed = setup_results.iter().any(|s| !s.passed);

    if let Some(p) = progress {
        let snapshot: Vec<String> = redacted_values.iter().cloned().collect();
        let ctx = ReportContext {
            redaction: &redaction,
            redacted_values: &snapshot,
        };
        p.setup_finished(&setup_results, &ctx);
    }

    let mut test_results = Vec::new();

    if !setup_failed {
        if !test_file.steps.is_empty()
            && selector::any_matches_test(selectors, file_path, &test_file.name)
        {
            // Simple format: flat steps (treated as a single "default" test
            // whose name is the file's own name).
            let selected_steps =
                filter_steps(&test_file.steps, selectors, file_path, &test_file.name);
            let mut step_captures = captures.clone();
            let step_results = run_steps(
                &selected_steps,
                env,
                &mut step_captures,
                test_file,
                &redaction,
                &mut redacted_values,
                &client,
                opts,
                cookies_enabled,
                cookie_jars,
                &base_dir,
            )?;
            let passed = step_results.iter().all(|s| s.passed);
            let duration_ms = step_results.iter().map(|s| s.duration_ms).sum();
            let test_result = TestResult {
                name: test_file.name.clone(),
                description: test_file.description.clone(),
                passed,
                duration_ms,
                step_results,
                captures: step_captures.clone(),
            };
            if let Some(p) = progress {
                let snapshot: Vec<String> = redacted_values.iter().cloned().collect();
                let ctx = ReportContext {
                    redaction: &redaction,
                    redacted_values: &snapshot,
                };
                p.test_finished(&test_result, &ctx);
            }
            test_results.push(test_result);
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

            // Skip tests that no selector matches.
            if !selector::any_matches_test(selectors, file_path, name) {
                continue;
            }

            // Per-test cookie isolation: wipe the default jar before each
            // named test runs so session state never leaks between tests.
            // Named jars (multi-user scenarios) are intentionally preserved.
            if cookie_mode == CookieMode::PerTest {
                cookie_jars.remove(DEFAULT_JAR_NAME);
            }

            let selected_steps = filter_steps(&test_group.steps, selectors, file_path, name);
            let mut test_captures = captures.clone();
            let step_results = run_steps(
                &selected_steps,
                env,
                &mut test_captures,
                test_file,
                &redaction,
                &mut redacted_values,
                &client,
                opts,
                cookies_enabled,
                cookie_jars,
                &base_dir,
            )?;
            let passed = step_results.iter().all(|s| s.passed);
            let duration_ms = step_results.iter().map(|s| s.duration_ms).sum();
            let test_result = TestResult {
                name: name.clone(),
                description: test_group.description.clone(),
                passed,
                duration_ms,
                step_results,
                captures: test_captures.clone(),
            };
            if let Some(p) = progress {
                let snapshot: Vec<String> = redacted_values.iter().cloned().collect();
                let ctx = ReportContext {
                    redaction: &redaction,
                    redacted_values: &snapshot,
                };
                p.test_finished(&test_result, &ctx);
            }
            test_results.push(test_result);
        }
    }

    // Run teardown steps (always, even on failure)
    let teardown_results = run_steps(
        &test_file.teardown,
        env,
        &mut captures,
        test_file,
        &redaction,
        &mut redacted_values,
        &client,
        opts,
        cookies_enabled,
        cookie_jars,
        &base_dir,
    )?;

    if let Some(p) = progress {
        let snapshot: Vec<String> = redacted_values.iter().cloned().collect();
        let ctx = ReportContext {
            redaction: &redaction,
            redacted_values: &snapshot,
        };
        p.teardown_finished(&teardown_results, &ctx);
    }

    let all_passed = !setup_failed
        && test_results.iter().all(|t| t.passed)
        && teardown_results.iter().all(|s| s.passed);

    let file_result = FileResult {
        file: file_path.to_string(),
        name: test_file.name.clone(),
        passed: all_passed,
        duration_ms: start.elapsed().as_millis() as u64,
        redaction,
        redacted_values: redacted_values.into_iter().collect(),
        setup_results,
        test_results,
        teardown_results,
    };

    if let Some(p) = progress {
        p.file_finished(&file_result);
    }

    Ok(file_result)
}

/// Return the subset of steps matching the active selectors. Without
/// selectors (or without step-level constraints for this test) the full
/// input is returned. Step selection drops prior steps, so tests that
/// rely on chained captures should use a test-level selector instead of
/// a step-level selector.
fn filter_steps(
    steps: &[Step],
    selectors: &[Selector],
    file_path: &str,
    test_name: &str,
) -> Vec<Step> {
    if selectors.is_empty() || !selector::has_step_level_filter(selectors, file_path, test_name) {
        return steps.to_vec();
    }
    steps
        .iter()
        .enumerate()
        .filter(|(index, step)| {
            selector::any_matches_step(selectors, file_path, test_name, *index, &step.name)
        })
        .map(|(_, step)| step.clone())
        .collect()
}

/// Run a sequence of steps, accumulating captures.
#[allow(clippy::too_many_arguments)]
fn run_steps(
    steps: &[Step],
    env: &HashMap<String, String>,
    captures: &mut HashMap<String, serde_json::Value>,
    test_file: &TestFile,
    redaction: &RedactionConfig,
    redacted_values: &mut BTreeSet<String>,
    client: &http::HttpClient,
    opts: &RunOptions,
    cookies_enabled: bool,
    cookie_jars: &mut HashMap<String, CookieJar>,
    base_dir: &Path,
) -> Result<Vec<StepResult>, TarnError> {
    let mut results = Vec::new();
    // Track which capture names this scope failed to produce so that
    // downstream steps referencing them can be classified as cascade
    // fallout (`SkippedDueToFailedCapture`) instead of re-failing the
    // run with `UnresolvedTemplate`. Keyed on capture name (not
    // per-step) because callers can reference a single failed capture
    // in many later steps; one failure name suppresses them all.
    let mut failed_captures: BTreeSet<String> = BTreeSet::new();
    let mut any_step_failed = false;

    for step in steps {
        // `fail_fast_within_test`: once any step in this scope has
        // failed, short-circuit the remaining ones so reports stop at
        // the root cause.
        if opts.fail_fast_within_test && any_step_failed {
            results.push(fail_fast_skipped_step(step));
            continue;
        }

        // If this step's request interpolates a capture that the test
        // already failed to produce, skip the HTTP round-trip and
        // record the cascade explicitly. This keeps the output short
        // and machine-classifiable instead of flooding it with
        // unresolved-template duplicates.
        let cascade_refs = step_references_failed_captures(step, &failed_captures);
        if !cascade_refs.is_empty() {
            results.push(skipped_due_to_failed_capture(step, &cascade_refs));
            any_step_failed = true;
            // Propagate: anything this step would have captured is
            // also unavailable downstream.
            for name in step.capture.keys() {
                failed_captures.insert(name.clone());
            }
            continue;
        }

        let result = run_step(
            step,
            env,
            captures,
            test_file,
            redaction,
            redacted_values,
            client,
            opts,
            cookies_enabled,
            cookie_jars,
            base_dir,
        )?;

        if !result.passed {
            any_step_failed = true;
            // Any declared capture whose value never made it into the
            // shared `captures` map is a failed capture for cascade
            // detection purposes. We use `captures_set` (the authority
            // on what this step did produce) rather than the
            // `error_category`, so a partial-failure path that captured
            // some values and failed assertions on others still blocks
            // downstream references to the missing names.
            for name in step.capture.keys() {
                if !result.captures_set.iter().any(|k| k == name) {
                    failed_captures.insert(name.clone());
                }
            }
        }

        results.push(result);
    }

    Ok(results)
}

/// Collect the `capture.<name>` references that `step`'s request makes
/// and that appear in `failed_captures`. Returns the set of referenced
/// names (sorted, deduped) so the caller can render a diagnostic like
/// `skipped: prior capture 'user_id' failed`.
fn step_references_failed_captures(step: &Step, failed_captures: &BTreeSet<String>) -> Vec<String> {
    if failed_captures.is_empty() {
        return Vec::new();
    }

    let mut refs: BTreeSet<String> = BTreeSet::new();
    fn collect(out: &mut BTreeSet<String>, s: &str) {
        for expr in interpolation::find_unresolved(s) {
            if let Some(name) = expr.strip_prefix("capture.") {
                out.insert(name.trim().to_string());
            }
        }
    }
    fn collect_json(out: &mut BTreeSet<String>, value: &serde_json::Value) {
        for expr in interpolation::find_unresolved_in_json(value) {
            if let Some(name) = expr.strip_prefix("capture.") {
                out.insert(name.trim().to_string());
            }
        }
    }

    collect(&mut refs, &step.request.url);
    for v in step.request.headers.values() {
        collect(&mut refs, v);
    }
    if let Some(ref body) = step.request.body {
        collect_json(&mut refs, body);
    }
    if let Some(ref form) = step.request.form {
        for v in form.values() {
            collect(&mut refs, v);
        }
    }
    if let Some(ref auth) = step.request.auth {
        if let Some(ref bearer) = auth.bearer {
            collect(&mut refs, bearer);
        }
        if let Some(ref basic) = auth.basic {
            collect(&mut refs, &basic.username);
            collect(&mut refs, &basic.password);
        }
    }
    if let Some(ref graphql) = step.request.graphql {
        collect(&mut refs, &graphql.query);
        if let Some(ref vars) = graphql.variables {
            collect_json(&mut refs, vars);
        }
    }

    refs.retain(|name| failed_captures.contains(name));
    refs.into_iter().collect()
}

fn skipped_due_to_failed_capture(step: &Step, failed_refs: &[String]) -> StepResult {
    let message = format!(
        "Skipped: step references capture(s) that failed earlier in this test: {}. \
         Fix the root-cause step first — this cascade failure is a direct consequence.",
        failed_refs.join(", ")
    );
    StepResult {
        name: step.name.clone(),
        passed: false,
        duration_ms: 0,
        assertion_results: vec![AssertionResult::fail(
            "cascade",
            "prior captures available".to_string(),
            format!("missing: {}", failed_refs.join(", ")),
            message,
        )],
        request_info: None,
        response_info: None,
        error_category: Some(FailureCategory::SkippedDueToFailedCapture),
        response_status: None,
        response_summary: None,
        captures_set: vec![],
        location: step.location.clone(),
    }
}

fn fail_fast_skipped_step(step: &Step) -> StepResult {
    StepResult {
        name: step.name.clone(),
        passed: false,
        duration_ms: 0,
        assertion_results: vec![AssertionResult::fail(
            "fail_fast",
            "earlier steps passing".to_string(),
            "earlier step failed".to_string(),
            "Skipped: `fail_fast_within_test` aborted the remaining steps after an earlier failure.".to_string(),
        )],
        request_info: None,
        response_info: None,
        error_category: Some(FailureCategory::SkippedDueToFailFast),
        response_status: None,
        response_summary: None,
        captures_set: vec![],
        location: step.location.clone(),
    }
}

/// Stamp each `AssertionResult` with its source `Location` (if the
/// parser recorded one for that operator key on this step). The lookup
/// is keyed on the same label the assertion modules emit — `"status"`,
/// `"duration"`, `"redirect.url"`, `"header <Name>"`, `"body <path>"`
/// — so there is no risk of mis-attribution across operators.
fn stamp_assertion_locations(step: &Step, results: Vec<AssertionResult>) -> Vec<AssertionResult> {
    if step.assertion_locations.is_empty() {
        return results;
    }
    results
        .into_iter()
        .map(|result| {
            let location = parser::assertion_location(step, &result.assertion);
            result.with_location(location)
        })
        .collect()
}

fn runtime_failure_step(
    step: &Step,
    duration_ms: u64,
    request_info: RequestInfo,
    error: TarnError,
) -> StepResult {
    let error_category = match &error {
        TarnError::Http(message) => Some(http_failure_category(message)),
        TarnError::Capture(_) => Some(FailureCategory::CaptureError),
        TarnError::Parse(_)
        | TarnError::Config(_)
        | TarnError::Interpolation(_)
        | TarnError::Validation(_) => Some(FailureCategory::ParseError),
        TarnError::Io(_) => Some(FailureCategory::ConnectionError),
        TarnError::Script(_) => None,
    };

    StepResult {
        name: step.name.clone(),
        passed: false,
        duration_ms,
        assertion_results: vec![AssertionResult::fail(
            "runtime",
            "step completed successfully",
            "runtime error",
            error.to_string(),
        )],
        request_info: Some(request_info),
        response_info: None,
        error_category,
        response_status: None,
        response_summary: None,
        captures_set: vec![],
        location: step.location.clone(),
    }
}

fn http_failure_category(message: &str) -> FailureCategory {
    if message.to_ascii_lowercase().contains("timed out") {
        FailureCategory::Timeout
    } else {
        FailureCategory::ConnectionError
    }
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

fn format_transport(transport: http::RequestTransportOptions) -> String {
    match (transport.timeout_ms, transport.connect_timeout_ms) {
        (None, None) => "none".into(),
        (Some(total), None) => format!("{}ms", total),
        (None, Some(connect)) => format!("connect={}ms", connect),
        (Some(total), Some(connect)) => format!("{}ms, connect={}ms", total, connect),
    }
}

/// Resolve which cookie jar a step should use.
/// Returns None if cookies are disabled for this step, or the jar name.
fn resolve_jar_name(step: &Step) -> Option<String> {
    match &step.cookies {
        None => Some(DEFAULT_JAR_NAME.to_string()),
        Some(StepCookies::Enabled(true)) => Some(DEFAULT_JAR_NAME.to_string()),
        Some(StepCookies::Enabled(false)) => None,
        Some(StepCookies::Named(name)) => Some(name.clone()),
    }
}

fn collect_redacted_env_values(
    env: &HashMap<String, String>,
    redaction: &RedactionConfig,
) -> BTreeSet<String> {
    let mut values = BTreeSet::new();
    for name in &redaction.env_vars {
        if let Some(value) = env.get(name) {
            if !value.is_empty() {
                values.insert(value.clone());
            }
        }
    }
    values
}

fn record_redacted_capture_candidates(
    response: &http::HttpResponse,
    capture_map: &HashMap<String, crate::model::CaptureSpec>,
    redaction: &RedactionConfig,
    redacted_values: &mut BTreeSet<String>,
    ctx: &Context,
) {
    for name in &redaction.captures {
        let Some(spec) = capture_map.get(name) else {
            continue;
        };
        let view = capture::ResponseView {
            status: response.status,
            url: &response.url,
            body: &response.body,
            headers: &response.headers,
            raw_headers: &response.raw_headers,
        };
        if let Ok(value) = capture::extract_capture(&view, name, spec, ctx) {
            insert_redacted_value(&capture::value_to_string(&value), redacted_values);
        }
    }
}

fn record_redacted_named_values(
    values: &HashMap<String, serde_json::Value>,
    redaction: &RedactionConfig,
    redacted_values: &mut BTreeSet<String>,
) {
    for name in &redaction.captures {
        if let Some(value) = values.get(name) {
            insert_redacted_value(&capture::value_to_string(value), redacted_values);
        }
    }
}

fn insert_redacted_value(value: &str, redacted_values: &mut BTreeSet<String>) {
    if !value.is_empty() {
        redacted_values.insert(value.to_string());
    }
}

struct PreparedRequest {
    url: String,
    headers: HashMap<String, String>,
    body: Option<serde_json::Value>,
    form: Option<IndexMap<String, String>>,
    transport: http::RequestTransportOptions,
    ctx: Context,
}

fn resolve_multipart_for_report(
    multipart: &crate::model::MultipartBody,
    base_dir: &Path,
) -> crate::model::MultipartBody {
    let mut resolved = multipart.clone();
    for file in &mut resolved.files {
        file.path = base_dir.join(&file.path).display().to_string();
    }
    resolved
}

fn build_request_info(step: &Step, request: &PreparedRequest, base_dir: &Path) -> RequestInfo {
    let multipart = step
        .request
        .multipart
        .as_ref()
        .map(|multipart| resolve_multipart_for_report(multipart, base_dir));
    let mut headers = request.headers.clone();
    if multipart.is_some() {
        headers.retain(|key, _| !key.eq_ignore_ascii_case("content-type"));
    }

    RequestInfo {
        method: step.request.method.clone(),
        url: request.url.clone(),
        headers,
        body: request.body.clone(),
        multipart,
    }
}

fn form_to_report_body(form: &IndexMap<String, String>) -> serde_json::Value {
    let body: serde_json::Map<String, serde_json::Value> = form
        .iter()
        .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
        .collect();
    serde_json::Value::Object(body)
}

fn effective_auth<'a>(step: &'a Step, test_file: &'a TestFile) -> Option<&'a AuthConfig> {
    step.request.auth.as_ref().or_else(|| {
        test_file
            .defaults
            .as_ref()
            .and_then(|defaults| defaults.auth.as_ref())
    })
}

fn apply_auth_header(
    headers: &mut HashMap<String, String>,
    auth: Option<&AuthConfig>,
    ctx: &Context,
) {
    if headers
        .keys()
        .any(|key| key.eq_ignore_ascii_case("authorization"))
    {
        return;
    }

    let Some(auth) = auth else {
        return;
    };

    if let Some(token) = auth.bearer.as_ref() {
        headers.insert(
            "Authorization".into(),
            format!("Bearer {}", interpolation::interpolate(token, ctx)),
        );
    } else if let Some(basic) = auth.basic.as_ref() {
        let username = interpolation::interpolate(&basic.username, ctx);
        let password = interpolation::interpolate(&basic.password, ctx);
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
        headers.insert("Authorization".into(), format!("Basic {encoded}"));
    }
}

/// Prepare interpolated request parts from a step.
fn prepare_request(
    step: &Step,
    env: &HashMap<String, String>,
    captures: &HashMap<String, serde_json::Value>,
    test_file: &TestFile,
    cookie_jar: Option<&CookieJar>,
) -> PreparedRequest {
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
    apply_auth_header(&mut merged_headers, effective_auth(step, test_file), &ctx);

    // Inject cookies from jar (if not already set by the user)
    if let Some(jar) = cookie_jar {
        if !merged_headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("cookie"))
        {
            if let Some(cookie_header) = jar.cookie_header(&url) {
                merged_headers.insert("Cookie".to_string(), cookie_header);
            }
        }
    }

    // GraphQL: build body from graphql block and auto-set Content-Type
    let (body, form) = if let Some(ref gql) = step.request.graphql {
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
        (Some(gql_body), None)
    } else if let Some(ref form) = step.request.form {
        // Ensure the content type is form-urlencoded. Override any non-form
        // content type (e.g. application/json from defaults), but preserve
        // form-urlencoded variants (e.g. with charset param).
        let has_form_ct = merged_headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("content-type")
                && v.starts_with("application/x-www-form-urlencoded")
        });
        if !has_form_ct {
            merged_headers.retain(|k, _| !k.eq_ignore_ascii_case("content-type"));
            merged_headers.insert(
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            );
        }
        let form = interpolation::interpolate_string_map(form, &ctx);
        (Some(form_to_report_body(&form)), Some(form))
    } else {
        (
            step.request
                .body
                .as_ref()
                .map(|b| interpolation::interpolate_json(b, &ctx)),
            None,
        )
    };

    let headers = interpolation::interpolate_headers(&merged_headers, &ctx);
    let transport = http::RequestTransportOptions {
        timeout_ms: step
            .timeout
            .or_else(|| test_file.defaults.as_ref().and_then(|d| d.timeout)),
        connect_timeout_ms: step
            .connect_timeout
            .or_else(|| test_file.defaults.as_ref().and_then(|d| d.connect_timeout)),
        follow_redirects: step
            .follow_redirects
            .or_else(|| test_file.defaults.as_ref().and_then(|d| d.follow_redirects)),
        max_redirs: step
            .max_redirs
            .or_else(|| test_file.defaults.as_ref().and_then(|d| d.max_redirs)),
    };

    PreparedRequest {
        url,
        headers,
        body,
        form,
        transport,
        ctx,
    }
}

fn execute_prepared_request(
    client: &http::HttpClient,
    step: &Step,
    request: &PreparedRequest,
    base_dir: &Path,
) -> Result<http::HttpResponse, TarnError> {
    if let Some(ref multipart) = step.request.multipart {
        http::execute_multipart_request(
            client,
            &step.request.method,
            &request.url,
            &request.headers,
            multipart,
            request.transport,
            base_dir,
        )
    } else if let Some(ref form) = request.form {
        http::execute_form_request(
            client,
            &step.request.method,
            &request.url,
            &request.headers,
            form,
            request.transport,
        )
    } else {
        http::execute_request(
            client,
            &step.request.method,
            &request.url,
            &request.headers,
            request.body.as_ref(),
            request.transport,
        )
    }
}

/// Run a single step: interpolate, execute HTTP request, run assertions, extract captures.
/// Supports retries, polling, GraphQL, multipart, Lua scripts, step-level timeout, delay, verbose, dry-run.
/// Capture failures are handled gracefully — the step is marked as failed instead of aborting the run.
#[allow(clippy::too_many_arguments)]
fn run_step(
    step: &Step,
    env: &HashMap<String, String>,
    captures: &mut HashMap<String, serde_json::Value>,
    test_file: &TestFile,
    redaction: &RedactionConfig,
    redacted_values: &mut BTreeSet<String>,
    client: &http::HttpClient,
    opts: &RunOptions,
    cookies_enabled: bool,
    cookie_jars: &mut HashMap<String, CookieJar>,
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

    // Resolve which cookie jar to use (None = disabled)
    let jar_name = if cookies_enabled {
        resolve_jar_name(step)
    } else {
        None
    };

    // Dispatch to poll mode if configured
    if let Some(ref poll) = step.poll {
        return run_step_poll(
            step,
            poll,
            env,
            captures,
            test_file,
            redaction,
            redacted_values,
            client,
            opts,
            cookies_enabled,
            cookie_jars,
            base_dir,
        );
    }

    let request = prepare_request(
        step,
        env,
        captures,
        test_file,
        jar_name
            .as_ref()
            .and_then(|name| cookie_jars.get(name.as_str())),
    );
    let request_info = build_request_info(step, &request, base_dir);

    // Check for unresolved template expressions (e.g. failed captures, missing env vars)
    let mut unresolved = interpolation::find_unresolved(&request.url);
    for v in request.headers.values() {
        unresolved.extend(interpolation::find_unresolved(v));
    }
    if let Some(ref body) = request.body {
        unresolved.extend(interpolation::find_unresolved_in_json(body));
    }
    if !unresolved.is_empty() {
        unresolved.sort();
        unresolved.dedup();
        let names = unresolved.join(", ");
        return Ok(StepResult {
            name: step.name.clone(),
            passed: false,
            duration_ms: 0,
            assertion_results: vec![AssertionResult::fail(
                "interpolation",
                "all templates resolved",
                format!("unresolved: {}", names),
                format!(
                    "Unresolved template variables: {}. Check that prior captures succeeded and env vars are set.",
                    names
                ),
            )],
            request_info: Some(request_info),
            response_info: None,
            error_category: Some(FailureCategory::UnresolvedTemplate),
            response_status: None,
            response_summary: None,
            captures_set: vec![],
            location: step.location.clone(),
        });
    }

    // Verbose: print request details
    if opts.verbose {
        eprintln!(
            "  --> {} {} (timeout: {})",
            step.request.method,
            request.url,
            format_transport(request.transport)
        );
    }

    // Dry-run: show what would be sent, return a pass result
    if opts.dry_run {
        eprintln!(
            "  [dry-run] {} {} {}",
            step.name, step.request.method, request.url
        );
        return Ok(StepResult {
            name: step.name.clone(),
            passed: true,
            duration_ms: 0,
            assertion_results: vec![],
            request_info: Some(request_info.clone()),
            response_info: None,
            error_category: None,
            response_status: None,
            response_summary: None,
            captures_set: vec![],
            location: step.location.clone(),
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
        let response = execute_prepared_request(client, step, &request, base_dir);

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if opts.verbose {
                    eprintln!("  !! {}", error);
                }
                if attempt < max_retries {
                    std::thread::sleep(std::time::Duration::from_millis(
                        100 * (attempt as u64 + 1),
                    ));
                    continue;
                }
                return Ok(runtime_failure_step(step, 0, request_info.clone(), error));
            }
        };

        // Capture cookies from response into the appropriate jar
        if let Some(ref name) = jar_name {
            let jar = cookie_jars.entry(name.clone()).or_default();
            jar.capture_from_response(&response.url, &response.raw_headers);
        }

        record_redacted_capture_candidates(
            &response,
            &step.capture,
            redaction,
            redacted_values,
            &request.ctx,
        );

        if opts.verbose {
            eprintln!("  <-- {} ({}ms)", response.status, response.duration_ms);
            if max_retries > 0 && attempt > 0 {
                eprintln!("      (retry {}/{})", attempt, max_retries);
            }
        }

        let assertion_results = if let Some(ref assertion) = step.assertions {
            let interpolated = interpolate_assertion(assertion, &request.ctx);
            stamp_assertion_locations(step, assert::run_assertions(&interpolated, &response))
        } else {
            vec![]
        };

        let passed = assertion_results.iter().all(|a| a.passed);

        if passed {
            let resp_status = response.status;
            let resp_summary = summarize_response(&response);

            // Extract captures on success — graceful failure (P0 fix)
            let mut captured_keys = Vec::new();
            let capture_result = if !step.capture.is_empty() {
                match capture::extract_captures(
                    &capture::ResponseView {
                        status: response.status,
                        url: &response.url,
                        body: &response.body,
                        headers: &response.headers,
                        raw_headers: &response.raw_headers,
                    },
                    &step.capture,
                    &request.ctx,
                ) {
                    Ok(new_captures) => {
                        captured_keys = new_captures.keys().cloned().collect();
                        record_redacted_named_values(&new_captures, redaction, redacted_values);
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
                    request_info: Some(request_info.clone()),
                    response_info: Some(ResponseInfo {
                        status: response.status,
                        headers: response.headers,
                        body: Some(response.body),
                    }),
                    error_category: Some(FailureCategory::CaptureError),
                    response_status: Some(resp_status),
                    response_summary: Some(resp_summary),
                    captures_set: vec![],
                    location: step.location.clone(),
                });
            }

            // Run Lua script if present
            let (all_assertions, all_passed) = run_script_if_present(
                step,
                &response,
                captures,
                assertion_results,
                redaction,
                redacted_values,
            )?;

            return Ok(StepResult {
                name: step.name.clone(),
                passed: all_passed,
                duration_ms: response.duration_ms,
                assertion_results: all_assertions,
                request_info: Some(request_info.clone()),
                response_info: None,
                error_category: None,
                response_status: Some(resp_status),
                response_summary: Some(resp_summary),
                captures_set: captured_keys,
                location: step.location.clone(),
            });
        }

        last_result = Some((response, assertion_results));

        if attempt < max_retries {
            std::thread::sleep(std::time::Duration::from_millis(100 * (attempt as u64 + 1)));
        }
    }

    // All attempts failed
    let (response, assertion_results) = last_result.unwrap();
    let resp_status = response.status;
    let resp_summary = summarize_response(&response);

    Ok(StepResult {
        name: step.name.clone(),
        passed: false,
        duration_ms: response.duration_ms,
        assertion_results,
        request_info: Some(request_info),
        response_info: Some(ResponseInfo {
            status: response.status,
            headers: response.headers,
            body: Some(response.body),
        }),
        error_category: Some(FailureCategory::AssertionFailed),
        response_status: Some(resp_status),
        response_summary: Some(resp_summary),
        captures_set: vec![],
        location: step.location.clone(),
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
    redaction: &RedactionConfig,
    redacted_values: &mut BTreeSet<String>,
    client: &http::HttpClient,
    opts: &RunOptions,
    cookies_enabled: bool,
    cookie_jars: &mut HashMap<String, CookieJar>,
    base_dir: &Path,
) -> Result<StepResult, TarnError> {
    let interval_ms = parse_delay(&poll.interval).unwrap_or(1000);
    let jar_name = if cookies_enabled {
        resolve_jar_name(step)
    } else {
        None
    };

    // Persist enough context from each attempt so that a timeout can
    // emit a real diagnostic instead of "Polling timed out after N
    // attempts". NAZ-339: users cannot tell a real product bug from a
    // brittle assertion or slow eventual consistency without the final
    // observed value. We capture both the first and last attempts so the
    // timeout report can flag whether the system is making progress
    // (`"pending" → "ready"`, changing) or actually stuck.
    let mut first_snapshot: Option<PollSnapshot> = None;
    let mut last_snapshot: Option<PollSnapshot> = None;

    for attempt in 0..poll.max_attempts {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        }

        let request = prepare_request(
            step,
            env,
            captures,
            test_file,
            jar_name
                .as_ref()
                .and_then(|name| cookie_jars.get(name.as_str())),
        );
        let request_info = build_request_info(step, &request, base_dir);

        // Check for unresolved template expressions before sending
        let mut unresolved = interpolation::find_unresolved(&request.url);
        for v in request.headers.values() {
            unresolved.extend(interpolation::find_unresolved(v));
        }
        if let Some(ref body) = request.body {
            unresolved.extend(interpolation::find_unresolved_in_json(body));
        }
        if !unresolved.is_empty() {
            unresolved.sort();
            unresolved.dedup();
            let names = unresolved.join(", ");
            return Ok(StepResult {
                name: step.name.clone(),
                passed: false,
                duration_ms: 0,
                assertion_results: vec![AssertionResult::fail(
                    "interpolation",
                    "all templates resolved",
                    format!("unresolved: {}", names),
                    format!(
                        "Unresolved template variables: {}. Check that prior captures succeeded and env vars are set.",
                        names
                    ),
                )],
                request_info: Some(request_info),
                response_info: None,
                error_category: Some(FailureCategory::UnresolvedTemplate),
                response_status: None,
                response_summary: None,
                captures_set: vec![],
                location: step.location.clone(),
            });
        }

        if opts.verbose {
            eprintln!(
                "  [poll {}/{}] {} {}",
                attempt + 1,
                poll.max_attempts,
                step.request.method,
                request.url
            );
        }

        let response = execute_prepared_request(client, step, &request, base_dir);

        let response = match response {
            Ok(response) => response,
            Err(error) => return Ok(runtime_failure_step(step, 0, request_info, error)),
        };

        // Capture cookies into the appropriate jar
        if let Some(ref name) = jar_name {
            let jar = cookie_jars.entry(name.clone()).or_default();
            jar.capture_from_response(&response.url, &response.raw_headers);
        }

        record_redacted_capture_candidates(
            &response,
            &step.capture,
            redaction,
            redacted_values,
            &request.ctx,
        );

        // Check poll.until condition
        let until_interpolated = interpolate_assertion(&poll.until, &request.ctx);
        let until_results = assert::run_assertions(&until_interpolated, &response);
        let until_passed = until_results.iter().all(|a| a.passed);

        // Snapshot this attempt in case it turns out to be the final one.
        // We clone the response info pieces that are cheap (headers, url,
        // body) rather than holding a ref — the response is consumed on
        // each iteration, and on timeout we want the last attempt's data
        // even though the loop has moved on.
        let snapshot = PollSnapshot {
            attempt_index: attempt,
            response_status: response.status,
            response_summary: summarize_response(&response),
            response_info: ResponseInfo {
                status: response.status,
                headers: response.headers.clone(),
                body: Some(response.body.clone()),
            },
            request_info: request_info.clone(),
            until_results: until_results.clone(),
        };
        if first_snapshot.is_none() {
            first_snapshot = Some(snapshot.clone());
        }
        last_snapshot = Some(snapshot);

        if until_passed {
            // Condition met — run the step's own assertions
            let assertion_results = if let Some(ref assertion) = step.assertions {
                let interpolated = interpolate_assertion(assertion, &request.ctx);
                stamp_assertion_locations(step, assert::run_assertions(&interpolated, &response))
            } else {
                vec![]
            };

            let passed = assertion_results.iter().all(|a| a.passed);

            let resp_status = response.status;
            let resp_summary = summarize_response(&response);

            // Extract captures — graceful failure
            let mut captured_keys = Vec::new();
            if passed && !step.capture.is_empty() {
                match capture::extract_captures(
                    &capture::ResponseView {
                        status: response.status,
                        url: &response.url,
                        body: &response.body,
                        headers: &response.headers,
                        raw_headers: &response.raw_headers,
                    },
                    &step.capture,
                    &request.ctx,
                ) {
                    Ok(new_captures) => {
                        captured_keys = new_captures.keys().cloned().collect();
                        record_redacted_named_values(&new_captures, redaction, redacted_values);
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
                            request_info: Some(request_info.clone()),
                            response_info: Some(ResponseInfo {
                                status: response.status,
                                headers: response.headers,
                                body: Some(response.body),
                            }),
                            error_category: Some(FailureCategory::CaptureError),
                            response_status: Some(resp_status),
                            response_summary: Some(resp_summary),
                            captures_set: vec![],
                            location: step.location.clone(),
                        });
                    }
                }
            }

            // Run Lua script if present
            let (all_assertions, all_passed) = run_script_if_present(
                step,
                &response,
                captures,
                assertion_results,
                redaction,
                redacted_values,
            )?;

            return Ok(StepResult {
                name: step.name.clone(),
                passed: all_passed,
                duration_ms: response.duration_ms,
                assertion_results: all_assertions,
                request_info: Some(request_info.clone()),
                response_info: None,
                error_category: None,
                response_status: Some(resp_status),
                response_summary: Some(resp_summary),
                captures_set: captured_keys,
                location: step.location.clone(),
            });
        }
    }

    // Polling timed out. Build a diagnostic that captures the final
    // observed state plus a first-vs-last progress comparison so users
    // can tell an actually-stuck system apart from a brittle assertion
    // or slow eventual consistency.
    let last = match last_snapshot {
        Some(s) => s,
        None => {
            // poll.max_attempts == 0 is the only way we never entered the
            // loop body; fall back to the original minimal message.
            return Ok(StepResult {
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
                response_status: None,
                response_summary: None,
                captures_set: vec![],
                location: step.location.clone(),
            });
        }
    };

    let mut assertion_results: Vec<AssertionResult> = Vec::new();
    assertion_results.push(AssertionResult::fail(
        "poll",
        "condition met",
        format!("not met after {} attempts", poll.max_attempts),
        format!(
            "Polling timed out after {} attempts (interval: {}). Last response: HTTP {} {}.",
            poll.max_attempts, poll.interval, last.response_status, last.response_summary
        ),
    ));

    // Surface the final attempt's per-predicate results so the report
    // includes the actual value at each asserted JSONPath. We only carry
    // over failures — predicates that happened to match on the last
    // attempt would otherwise look confusing ("passed" assertions under
    // a timeout).
    for res in last.until_results.iter().filter(|r| !r.passed) {
        let mut decorated = res.clone();
        decorated.assertion = format!("poll final: {}", res.assertion);
        assertion_results.push(decorated);
    }

    // First-vs-last comparison: when the actual value changed between
    // attempts, the system is making progress but not reaching the
    // expected state — a strong hint that the assertion is too strict
    // rather than the endpoint being broken. When it stayed the same,
    // the system is actually stuck.
    if let Some(first) = first_snapshot {
        if first.attempt_index != last.attempt_index {
            for (i, last_res) in last.until_results.iter().enumerate() {
                if last_res.passed {
                    continue;
                }
                let first_actual = first
                    .until_results
                    .get(i)
                    .map(|r| r.actual.as_str())
                    .unwrap_or("<unknown>");
                let progress_label = if first_actual == last_res.actual {
                    "unchanged"
                } else {
                    "changed"
                };
                assertion_results.push(AssertionResult::fail(
                    format!("poll progress: {}", last_res.assertion),
                    first_actual.to_string(),
                    last_res.actual.clone(),
                    format!(
                        "{} across {} attempts: first {:?}, last {:?}",
                        progress_label,
                        last.attempt_index + 1,
                        first_actual,
                        last_res.actual
                    ),
                ));
            }
        }
    }

    Ok(StepResult {
        name: step.name.clone(),
        passed: false,
        duration_ms: 0,
        assertion_results,
        request_info: Some(last.request_info),
        response_info: Some(last.response_info),
        error_category: Some(FailureCategory::Timeout),
        response_status: Some(last.response_status),
        response_summary: Some(last.response_summary),
        captures_set: vec![],
        location: step.location.clone(),
    })
}

/// Per-attempt state we need to carry across iterations so that a poll
/// timeout can report the final observed value and whether the system
/// was making progress between attempts. Clone-heavy because attempts
/// are sparse and poll intervals typically dominate the total runtime;
/// the allocation is cheap compared to the HTTP round-trip.
#[derive(Debug, Clone)]
struct PollSnapshot {
    /// 0-based attempt index — used to decide whether first/last are
    /// the same snapshot (single-attempt poll).
    attempt_index: u32,
    response_status: u16,
    response_summary: String,
    response_info: ResponseInfo,
    request_info: RequestInfo,
    /// Per-predicate assertion results from the `poll.until` check on
    /// this attempt; their `actual` field is the observed value at each
    /// JSONPath.
    until_results: Vec<AssertionResult>,
}

/// Run Lua script after HTTP step if `script:` field is present.
/// Returns combined assertion results and overall pass/fail.
fn run_script_if_present(
    step: &Step,
    response: &http::HttpResponse,
    captures: &mut HashMap<String, serde_json::Value>,
    mut assertion_results: Vec<AssertionResult>,
    redaction: &RedactionConfig,
    redacted_values: &mut BTreeSet<String>,
) -> Result<(Vec<AssertionResult>, bool), TarnError> {
    if let Some(ref script) = step.script {
        let script_result = scripting::run_script(script, response, captures, &step.name)?;
        record_redacted_named_values(&script_result.captures, redaction, redacted_values);
        captures.extend(script_result.captures);
        assertion_results.extend(script_result.assertion_results);
    }
    let passed = assertion_results.iter().all(|a| a.passed);
    Ok((assertion_results, passed))
}

/// Generate a brief summary of an HTTP response for AI-friendly output.
fn summarize_response(response: &http::HttpResponse) -> String {
    let status_text = match response.status {
        200 => "200 OK",
        201 => "201 Created",
        204 => "204 No Content",
        301 => "301 Moved",
        302 => "302 Found",
        304 => "304 Not Modified",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        422 => "422 Unprocessable Entity",
        429 => "429 Too Many Requests",
        500 => "500 Internal Server Error",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        code => return format_response_summary(code, &response.body),
    };

    let body_hint = body_shape_hint(&response.body);
    if body_hint.is_empty() {
        status_text.to_string()
    } else {
        format!("{}: {}", status_text, body_hint)
    }
}

fn format_response_summary(status: u16, body: &serde_json::Value) -> String {
    let body_hint = body_shape_hint(body);
    if body_hint.is_empty() {
        format!("{}", status)
    } else {
        format!("{}: {}", status, body_hint)
    }
}

fn body_shape_hint(body: &serde_json::Value) -> String {
    match body {
        serde_json::Value::Array(arr) => format!("Array[{}]", arr.len()),
        serde_json::Value::Object(obj) => {
            if let Some(serde_json::Value::String(msg)) = obj.get("message") {
                truncate_str(msg, 80).to_string()
            } else if let Some(serde_json::Value::String(err)) = obj.get("error") {
                truncate_str(err, 80).to_string()
            } else {
                format!("Object{{{} keys}}", obj.len())
            }
        }
        serde_json::Value::String(s) => truncate_str(s, 80).to_string(),
        serde_json::Value::Null => String::new(),
        other => truncate_str(&other.to_string(), 80).to_string(),
    }
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        let end = s.floor_char_boundary(max_len.saturating_sub(3));
        // We return the truncated portion; caller may append "..."
        &s[..end]
    }
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

    if let Some(ref redirect) = assertion.redirect {
        result.redirect = Some(crate::model::RedirectAssertion {
            url: redirect
                .url
                .as_ref()
                .map(|url| interpolation::interpolate(url, ctx)),
            count: redirect.count,
        });
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

/// Directory basenames that are skipped during recursive test discovery
/// unless the caller opts out. These cover three classes of false positives:
/// nested Git worktrees that duplicate the whole suite
/// (`.git`, `.worktrees`), vendored dependency trees (`node_modules`,
/// `.venv`, `venv`), and build/scratch outputs (`dist`, `build`, `target`,
/// `tmp`, `.tarn`). Matched by basename, not full path, so a user's own
/// `tests/node_modules` is skipped the same way as a repo-root one.
pub const DEFAULT_DISCOVERY_IGNORES: &[&str] = &[
    ".git",
    ".worktrees",
    "node_modules",
    ".venv",
    "venv",
    "dist",
    "build",
    "target",
    "tmp",
    ".tarn",
];

/// Configuration for [`discover_test_files_with_report`]. Callers wanting the
/// documented defaults should use [`DiscoveryOptions::default`]; callers that
/// truly want to scan everything (e.g. the `--no-default-excludes` flag) build
/// an empty `ignored_dirs` vec.
#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    /// Directory names (basename-matched) to skip during recursion.
    pub ignored_dirs: Vec<String>,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            ignored_dirs: DEFAULT_DISCOVERY_IGNORES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }
}

/// Output of a discovery scan. `files` is the list of `.tarn.yaml` paths
/// that were selected; `excluded_roots` records the directories that were
/// skipped because they matched `DiscoveryOptions.ignored_dirs`, and
/// `duplicate_test_trees` flags the case where the search root contains more
/// than one `tests/` ancestor holding discovered files (a strong hint of
/// stale copies or nested worktrees surviving the excludes).
#[derive(Debug, Clone)]
pub struct DiscoveryReport {
    /// The root directory that was scanned.
    pub root: PathBuf,
    /// Discovered `.tarn.yaml` files, sorted lexicographically.
    pub files: Vec<String>,
    /// Directories that were skipped by the ignore rules. Full paths, sorted.
    pub excluded_roots: Vec<String>,
    /// Ancestor directories whose basename is `tests` and that contain
    /// at least one discovered file. Populated only when there are two or
    /// more such ancestors under `root`.
    pub duplicate_test_trees: Vec<String>,
}

/// Discover `.tarn.yaml` files recursively under `dir`, applying the
/// default ignore rules. Equivalent to
/// `discover_test_files_with_report(dir, &DiscoveryOptions::default())?.files`
/// — kept as a stable entry point for embedders (tarn-mcp, the LSP crate,
/// and existing tests) that do not need the summary data.
pub fn discover_test_files(dir: &Path) -> Result<Vec<String>, TarnError> {
    let report = discover_test_files_with_report(dir, &DiscoveryOptions::default())?;
    Ok(report.files)
}

/// Recursively discover `.tarn.yaml` files under `dir`, honoring
/// `opts.ignored_dirs`, and return a summary the CLI can print before the
/// run starts. The walker does not follow symlinked directories, so a
/// symlink that loops back to `dir` does not cause an infinite walk.
pub fn discover_test_files_with_report(
    dir: &Path,
    opts: &DiscoveryOptions,
) -> Result<DiscoveryReport, TarnError> {
    let mut files: Vec<String> = Vec::new();
    let mut excluded: Vec<String> = Vec::new();

    // DFS with an explicit stack avoids recursion blowing the stack on
    // deeply nested test fixtures and keeps ordering deterministic.
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(rd) => rd,
            Err(e) => {
                return Err(TarnError::Config(format!(
                    "Failed to read directory {}: {}",
                    current.display(),
                    e
                )));
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            // Don't traverse symlinks to directories — they can create
            // cycles and rarely point to something a caller wants in the
            // default recursive scan.
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };

            if file_type.is_dir() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if opts
                    .ignored_dirs
                    .iter()
                    .any(|ig| ig.as_str() == name_str.as_ref())
                {
                    excluded.push(path.display().to_string());
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() {
                let name = entry.file_name();
                if name.to_string_lossy().ends_with(".tarn.yaml") {
                    files.push(path.display().to_string());
                }
            }
        }
    }

    files.sort();
    excluded.sort();

    let duplicate_test_trees = detect_duplicate_test_trees(dir, &files);

    Ok(DiscoveryReport {
        root: dir.to_path_buf(),
        files,
        excluded_roots: excluded,
        duplicate_test_trees,
    })
}

/// Find ancestor directories named `tests` that contain at least one
/// discovered file. Returns them only when there is more than one, so
/// callers can warn the user that the run is pulling from separate
/// fixture trees (typically a sign of stale copies in a worktree).
fn detect_duplicate_test_trees(root: &Path, files: &[String]) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut tests_ancestors: BTreeSet<String> = BTreeSet::new();
    for file in files {
        let path = Path::new(file);
        for ancestor in path.ancestors().skip(1) {
            if !ancestor.starts_with(root) {
                break;
            }
            if ancestor.file_name().map(|n| n == "tests").unwrap_or(false) {
                tests_ancestors.insert(ancestor.display().to_string());
                // Don't break — record every `tests` ancestor on the path.
                // `tests/unit/foo` and `tests/unit` both count, and the
                // duplicate-tree check looks at the unique set.
            }
        }
    }
    if tests_ancestors.len() >= 2 {
        tests_ancestors.into_iter().collect()
    } else {
        Vec::new()
    }
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

    #[test]
    fn discover_test_files_scales_to_large_suites() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..250 {
            std::fs::write(dir.path().join(format!("test-{i}.tarn.yaml")), "name: t\nsteps:\n  - name: s\n    request:\n      method: GET\n      url: http://localhost\n").unwrap();
        }

        let files = discover_test_files(dir.path()).unwrap();
        assert_eq!(files.len(), 250);
        assert!(files.first().unwrap().ends_with("test-0.tarn.yaml"));
    }

    #[test]
    fn discover_skips_default_ignored_dirs() {
        let dir = tempfile::tempdir().unwrap();
        for ignored in &[
            ".git",
            ".worktrees/head-baseline/tests",
            "node_modules/pkg",
            ".venv/lib",
            "dist/snapshot",
            "build/cache",
            "target/debug/fixtures",
            "tmp",
            ".tarn/cache",
        ] {
            let sub = dir.path().join(ignored);
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join("stale.tarn.yaml"), "name: stale\n").unwrap();
        }

        let kept = dir.path().join("tests");
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::write(kept.join("real.tarn.yaml"), "name: real\n").unwrap();

        let report =
            discover_test_files_with_report(dir.path(), &DiscoveryOptions::default()).unwrap();

        assert_eq!(report.files.len(), 1);
        assert!(report.files[0].ends_with("real.tarn.yaml"));
        // Every top-level ignored directory should be listed in
        // excluded_roots so the summary can name them.
        for name in &[".git", ".worktrees", "node_modules", ".venv", "target"] {
            assert!(
                report
                    .excluded_roots
                    .iter()
                    .any(|r| r.ends_with(name) || r.contains(&format!("/{}", name))),
                "expected exclusion of {name} in {:?}",
                report.excluded_roots
            );
        }
    }

    #[test]
    fn discover_with_empty_ignores_recurses_everywhere() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join(".worktrees/head-baseline/tests");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("stale.tarn.yaml"), "name: stale\n").unwrap();

        let report = discover_test_files_with_report(
            dir.path(),
            &DiscoveryOptions {
                ignored_dirs: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(report.files.len(), 1);
        assert!(report.excluded_roots.is_empty());
    }

    #[test]
    fn discover_flags_duplicate_tests_trees() {
        // Simulate what EQHUB hit: a legitimate tests/ tree and a stale
        // copy inside a non-ignored directory so the duplicate-tree check
        // can still fire even after default excludes.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("tests");
        let stale = dir.path().join("baseline/tests");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(real.join("a.tarn.yaml"), "").unwrap();
        std::fs::write(stale.join("b.tarn.yaml"), "").unwrap();

        let report =
            discover_test_files_with_report(dir.path(), &DiscoveryOptions::default()).unwrap();
        assert_eq!(report.files.len(), 2);
        assert_eq!(report.duplicate_test_trees.len(), 2);
    }

    #[test]
    fn run_file_returns_failed_step_on_connection_error() {
        let yaml = r#"
name: Runtime failure
steps:
  - name: GET missing server
    request:
      method: GET
      url: "http://127.0.0.1:1/health"
    timeout: 50
    assert:
      status: 200
"#;
        let test_file: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();

        let result = run_file(
            &test_file,
            "runtime.tarn.yaml",
            &HashMap::new(),
            &[],
            &RunOptions::default(),
        )
        .unwrap();

        let step = &result.test_results[0].step_results[0];
        assert!(!step.passed);
        // On Windows, connecting to a closed port may timeout instead of
        // returning connection refused due to OS-level TCP differences.
        assert!(
            step.error_category == Some(FailureCategory::ConnectionError)
                || step.error_category == Some(FailureCategory::Timeout)
        );
        assert_eq!(
            step.request_info.as_ref().unwrap().url,
            "http://127.0.0.1:1/health"
        );
        assert!(step.response_info.is_none());
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

    #[test]
    fn format_transport_renders_combined_values() {
        assert_eq!(
            format_transport(http::RequestTransportOptions {
                timeout_ms: Some(5000),
                connect_timeout_ms: Some(250),
                ..http::RequestTransportOptions::default()
            }),
            "5000ms, connect=250ms"
        );
        assert_eq!(
            format_transport(http::RequestTransportOptions {
                timeout_ms: None,
                connect_timeout_ms: Some(250),
                ..http::RequestTransportOptions::default()
            }),
            "connect=250ms"
        );
    }

    #[test]
    fn collect_redacted_env_values_uses_named_vars() {
        let env = HashMap::from([
            ("base_url".to_string(), "https://example.com".to_string()),
            ("api_token".to_string(), "env-secret".to_string()),
        ]);
        let redaction = RedactionConfig {
            env_vars: vec!["api_token".into()],
            ..RedactionConfig::default()
        };

        let values = collect_redacted_env_values(&env, &redaction);
        assert_eq!(values.into_iter().collect::<Vec<_>>(), vec!["env-secret"]);
    }

    #[test]
    fn record_redacted_capture_candidates_harvests_named_capture_values() {
        let response = http::HttpResponse {
            status: 200,
            url: "http://example.com/final".to_string(),
            redirect_count: 0,
            headers: HashMap::new(),
            raw_headers: vec![],
            body_bytes: vec![],
            body: serde_json::json!({"token": "captured-secret"}),
            duration_ms: 0,
            timings: http::ResponseTimings {
                total_ms: 0,
                ttfb_ms: 0,
                body_read_ms: 0,
                connect_ms: None,
                tls_ms: None,
            },
        };
        let capture_map = HashMap::from([(
            "session".to_string(),
            crate::model::CaptureSpec::JsonPath("$.token".into()),
        )]);
        let redaction = RedactionConfig {
            captures: vec!["session".into()],
            ..RedactionConfig::default()
        };
        let mut values = BTreeSet::new();

        record_redacted_capture_candidates(
            &response,
            &capture_map,
            &redaction,
            &mut values,
            &Context::new(),
        );

        assert_eq!(
            values.into_iter().collect::<Vec<_>>(),
            vec!["captured-secret".to_string()]
        );
    }

    // --- Step cookies / named jars ---

    #[test]
    fn resolve_jar_name_default() {
        let yaml = r#"
name: test
steps:
  - name: step
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(resolve_jar_name(&tf.steps[0]), Some("default".to_string()));
    }

    #[test]
    fn resolve_jar_name_explicit_false() {
        let yaml = r#"
name: test
steps:
  - name: step
    cookies: false
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(resolve_jar_name(&tf.steps[0]), None);
    }

    #[test]
    fn resolve_jar_name_explicit_true() {
        let yaml = r#"
name: test
steps:
  - name: step
    cookies: true
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(resolve_jar_name(&tf.steps[0]), Some("default".to_string()));
    }

    #[test]
    fn resolve_jar_name_named() {
        let yaml = r#"
name: test
steps:
  - name: step
    cookies: "admin"
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(resolve_jar_name(&tf.steps[0]), Some("admin".to_string()));
    }

    // --- effective_cookie_mode ---

    #[test]
    fn effective_cookie_mode_default_is_auto() {
        assert_eq!(effective_cookie_mode(None, false), CookieMode::Auto);
    }

    #[test]
    fn effective_cookie_mode_off_file_beats_cli_per_test() {
        // Explicit `cookies: off` in the file disables cookies entirely.
        // The CLI --cookie-jar-per-test flag must not re-enable them.
        assert_eq!(
            effective_cookie_mode(Some(CookieMode::Off), true),
            CookieMode::Off
        );
    }

    #[test]
    fn effective_cookie_mode_cli_upgrades_auto_to_per_test() {
        assert_eq!(
            effective_cookie_mode(Some(CookieMode::Auto), true),
            CookieMode::PerTest
        );
        assert_eq!(effective_cookie_mode(None, true), CookieMode::PerTest);
    }

    #[test]
    fn effective_cookie_mode_file_per_test_without_cli() {
        assert_eq!(
            effective_cookie_mode(Some(CookieMode::PerTest), false),
            CookieMode::PerTest
        );
    }

    #[test]
    fn prepare_request_only_injects_matching_cookies() {
        let yaml = r#"
name: test
steps:
  - name: step
    request:
      method: GET
      url: "https://api.example.com/users"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let mut jar = CookieJar::new();
        jar.capture_from_response(
            "https://example.com/login",
            &[("set-cookie".to_string(), "session=abc123".to_string())],
        );
        jar.capture_from_response(
            "https://example.com/login",
            &[(
                "set-cookie".to_string(),
                "tenant=acme; Domain=example.com; Path=/".to_string(),
            )],
        );

        let request = prepare_request(
            &tf.steps[0],
            &HashMap::new(),
            &HashMap::new(),
            &tf,
            Some(&jar),
        );

        assert_eq!(
            request.headers.get("Cookie"),
            Some(&"tenant=acme".to_string())
        );
    }

    #[test]
    fn prepare_request_builds_form_body_and_content_type() {
        let yaml = r#"
name: test
steps:
  - name: submit form
    request:
      method: POST
      url: "https://api.example.com/login"
      form:
        email: "{{ env.email }}"
        password: "{{ capture.password }}"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let request = prepare_request(
            &tf.steps[0],
            &HashMap::from([("email".to_string(), "user@example.com".to_string())]),
            &HashMap::from([("password".to_string(), serde_json::json!("secret"))]),
            &tf,
            None,
        );

        assert_eq!(
            request.headers.get("Content-Type"),
            Some(&"application/x-www-form-urlencoded".to_string())
        );
        assert_eq!(
            request.body,
            Some(serde_json::json!({
                "email": "user@example.com",
                "password": "secret"
            }))
        );
        assert_eq!(
            request.form,
            Some(IndexMap::from([
                ("email".to_string(), "user@example.com".to_string()),
                ("password".to_string(), "secret".to_string()),
            ]))
        );
    }

    #[test]
    fn prepare_request_preserves_explicit_form_content_type_override() {
        let yaml = r#"
name: test
steps:
  - name: submit form
    request:
      method: POST
      url: "https://api.example.com/login"
      headers:
        Content-Type: "application/x-www-form-urlencoded; charset=utf-8"
      form:
        email: "user@example.com"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let request = prepare_request(&tf.steps[0], &HashMap::new(), &HashMap::new(), &tf, None);

        assert_eq!(
            request.headers.get("Content-Type"),
            Some(&"application/x-www-form-urlencoded; charset=utf-8".to_string())
        );
    }

    #[test]
    fn prepare_request_injects_bearer_auth_when_header_missing() {
        let yaml = r#"
name: auth
steps:
  - name: get profile
    request:
      method: GET
      url: "https://api.example.com/me"
      auth:
        bearer: "{{ env.token }}"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let request = prepare_request(
            &tf.steps[0],
            &HashMap::from([("token".to_string(), "secret-token".to_string())]),
            &HashMap::new(),
            &tf,
            None,
        );

        assert_eq!(
            request.headers.get("Authorization").map(String::as_str),
            Some("Bearer secret-token")
        );
    }

    #[test]
    fn prepare_request_keeps_explicit_authorization_header() {
        let yaml = r#"
name: auth
steps:
  - name: get profile
    request:
      method: GET
      url: "https://api.example.com/me"
      headers:
        Authorization: "ApiKey raw-header-wins"
      auth:
        bearer: "{{ env.token }}"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let request = prepare_request(
            &tf.steps[0],
            &HashMap::from([("token".to_string(), "secret-token".to_string())]),
            &HashMap::new(),
            &tf,
            None,
        );

        assert_eq!(
            request.headers.get("Authorization").map(String::as_str),
            Some("ApiKey raw-header-wins")
        );
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
    connect_timeout: 300
    follow_redirects: false
    max_redirs: 2
    delay: "500ms"
    assert:
      status: 200
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let step = &tf.steps[0];
        assert_eq!(step.retries, Some(3));
        assert_eq!(step.timeout, Some(2000));
        assert_eq!(step.connect_timeout, Some(300));
        assert_eq!(step.follow_redirects, Some(false));
        assert_eq!(step.max_redirs, Some(2));
        assert_eq!(step.delay, Some("500ms".to_string()));
    }

    #[test]
    fn defaults_with_retries() {
        let yaml = r#"
name: Default retries
defaults:
  retries: 2
  timeout: 5000
  connect_timeout: 250
  follow_redirects: false
  max_redirs: 1
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
        assert_eq!(defaults.connect_timeout, Some(250));
        assert_eq!(defaults.follow_redirects, Some(false));
        assert_eq!(defaults.max_redirs, Some(1));
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
        assert_eq!(step.connect_timeout, None);
        assert_eq!(step.follow_redirects, None);
        assert_eq!(step.max_redirs, None);
        assert_eq!(step.delay, None);
    }

    #[test]
    fn connect_timeout_supports_hyphen_alias() {
        let yaml = r#"
name: Alias
defaults:
  connect-timeout: 111
  follow-redirects: false
  max-redirs: 9
steps:
  - name: simple
    connect-timeout: 222
    follow-redirects: true
    max-redirs: 3
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let defaults = tf.defaults.unwrap();
        assert_eq!(defaults.connect_timeout, Some(111));
        assert_eq!(defaults.follow_redirects, Some(false));
        assert_eq!(defaults.max_redirs, Some(9));
        assert_eq!(tf.steps[0].connect_timeout, Some(222));
        assert_eq!(tf.steps[0].follow_redirects, Some(true));
        assert_eq!(tf.steps[0].max_redirs, Some(3));
    }
}
