use crate::assert;
use crate::assert::types::{
    AssertionResult, FailureCategory, FileResult, RequestInfo, ResponseInfo, StepResult, TestResult,
};
use crate::capture;
use crate::cookie::CookieJar;
use crate::error::TarnError;
use crate::fixtures;
use crate::http;
use crate::interpolation::{self, Context};
use crate::model::{
    Assertion, AuthConfig, CookieMode, HttpTransportConfig, PollConfig, RedactionConfig, Step,
    StepCookies, TestFile,
};
use crate::parser;
use crate::report::event_stream::EventStream;
use crate::report::fixture_writer::{self, FixtureWriteConfig};
use crate::report::progress::{ProgressReporter, ReportContext};
use crate::scripting;
use crate::selector::{self, Selector};
use base64::Engine;
use indexmap::IndexMap;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Options controlling how tests are run.
#[derive(Debug, Clone)]
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
    /// When true, capture response body, headers, and captures into
    /// the report for every step — not just failed ones. Individual
    /// steps can opt in via `debug: true` even when this flag is off.
    /// (NAZ-244.)
    pub verbose_responses: bool,
    /// Maximum response body size (bytes) to embed in the report when
    /// `verbose_responses` or step-level `debug: true` is active. Bodies
    /// larger than this are truncated and a `"...<truncated: N bytes>"`
    /// marker is appended. Defaults to 8 KiB.
    pub max_body_bytes: usize,
    /// Per-step fixture writer settings. When
    /// [`FixtureWriteConfig::enabled`] is false no fixtures are
    /// persisted — this is the `--no-fixtures` CLI path.
    pub fixtures: FixtureWriteConfig,
}

/// Default cap for verbose/debug response body embedding.
pub const DEFAULT_MAX_BODY_BYTES: usize = 8 * 1024;

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            verbose: false,
            dry_run: false,
            http: HttpTransportConfig::default(),
            cookie_jar_per_test: false,
            fail_fast_within_test: false,
            verbose_responses: false,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            fixtures: FixtureWriteConfig::default(),
        }
    }
}

/// Name of the default cookie jar used when no `cookies: <name>` is set on a step.
const DEFAULT_JAR_NAME: &str = "default";

/// Observers the runner fires as a file / test / step lifecycle
/// progresses. Both fields are optional so callers can mix and match
/// (e.g. CI wants NDJSON stdout + events.jsonl on disk; a plain local
/// run wants human progress and no events; watch-mode reruns might
/// want neither). Grouped into one struct so adding a new observer in
/// the future does not require yet another parameter on every call
/// site.
#[derive(Default, Clone)]
pub struct RunObservers<'a> {
    pub progress: Option<&'a (dyn ProgressReporter + Send + Sync)>,
    pub events: Option<&'a Arc<EventStream>>,
}

impl<'a> RunObservers<'a> {
    pub fn new() -> Self {
        Self {
            progress: None,
            events: None,
        }
    }

    pub fn with_progress(
        mut self,
        progress: Option<&'a (dyn ProgressReporter + Send + Sync)>,
    ) -> Self {
        self.progress = progress;
        self
    }

    pub fn with_events(mut self, events: Option<&'a Arc<EventStream>>) -> Self {
        self.events = events;
        self
    }
}

/// Location metadata used by the fixture writer so every step knows
/// which `(file, test)` directory to persist under. Setup / teardown
/// use the sentinel test slugs from [`crate::fixtures`].
#[derive(Debug, Clone)]
struct FixtureScope<'a> {
    /// Absolute path of the test file that owns the scope.
    file_path: &'a str,
    /// Test name or sentinel (`"setup"` / `"teardown"` / `"<flat>"`).
    test_label: &'a str,
}

/// Scheduling metadata extracted from a `.tarn.yaml` file before the
/// scheduler decides where the file belongs. Kept deliberately small so
/// the scheduler can inspect every discovered file cheaply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulingMetadata {
    /// The file's path (as reported to the user in results).
    pub file: String,
    /// Forced onto the serial worker when true, either because the file
    /// sets `serial_only: true` at the top level or because any of its
    /// named tests do. Individual-test pinning promotes the whole file
    /// to serial since Tarn's parallelism unit is the file.
    pub serial_only: bool,
    /// Optional resource group name. Files sharing the same `group:`
    /// string must land in the same parallel bucket so they run
    /// sequentially relative to each other.
    pub group: Option<String>,
}

impl SchedulingMetadata {
    /// Build metadata directly from a parsed `TestFile`. Any named test
    /// marked `serial_only: true` escalates the whole file to serial —
    /// see docs on [`crate::model::TestGroup::serial_only`].
    pub fn from_test_file(file: &str, test_file: &TestFile) -> Self {
        let any_test_serial = test_file.tests.values().any(|t| t.serial_only);
        Self {
            file: file.to_string(),
            serial_only: test_file.serial_only || any_test_serial,
            group: test_file.group.clone(),
        }
    }
}

/// Output of the scheduler planning step.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SchedulePlan {
    /// Buckets to run in parallel with each other. Files inside a
    /// bucket run sequentially (to preserve `group:` ordering and keep
    /// per-bucket shared-resource assumptions intact).
    pub parallel_buckets: Vec<Vec<String>>,
    /// Files that must run after every parallel bucket finishes, in
    /// sequence, on a single worker. Ordering matches input discovery
    /// order so reports stay deterministic.
    pub serial: Vec<String>,
}

/// Plan how a flat list of files should be dispatched across `jobs`
/// parallel workers under `--parallel`. Pure function — no I/O, no
/// rayon — so it can be exhaustively unit-tested.
///
/// Rules (in order):
///
/// 1. Any file flagged `serial_only` (either file-level or because one
///    of its named tests set `serial_only: true`) is pulled out of the
///    parallel set and deferred to the serial bucket.
/// 2. The remaining files are bucketed by their `group:` string. Files
///    without a group get their own singleton bucket so they can run on
///    any worker. Files sharing a group land in the same bucket so
///    they run sequentially relative to each other.
/// 3. Buckets are assigned across the available workers round-robin
///    based on `jobs`. When `jobs` is 0 or 1 the plan collapses to a
///    single parallel bucket (functionally sequential).
///
/// Within a bucket, file ordering matches the input slice so operators
/// can still reason about side-effect ordering inside a resource group.
pub fn plan_schedule(metadata: &[SchedulingMetadata], jobs: usize) -> SchedulePlan {
    let worker_count = jobs.max(1);
    let mut serial: Vec<String> = Vec::new();
    let mut parallel_metadata: Vec<&SchedulingMetadata> = Vec::new();
    for entry in metadata {
        if entry.serial_only {
            serial.push(entry.file.clone());
        } else {
            parallel_metadata.push(entry);
        }
    }

    // Bucket parallel-safe files by group. Files without a group are
    // given a unique synthetic key (their index) so every ungrouped file
    // becomes its own bucket — this preserves the previous behaviour
    // where every file could run on any worker. `IndexMap` preserves
    // insertion order, which gives deterministic bucket ordering across
    // runs with the same inputs.
    let mut buckets: IndexMap<String, Vec<String>> = IndexMap::new();
    for (idx, entry) in parallel_metadata.iter().enumerate() {
        let key = match entry.group.as_deref() {
            Some(name) if !name.is_empty() => format!("group:{}", name),
            _ => format!("__ungrouped__:{}", idx),
        };
        buckets.entry(key).or_default().push(entry.file.clone());
    }

    // Distribute buckets across workers round-robin so a small number of
    // groups does not starve the pool. `worker_buckets` collapses many
    // logical groups into `worker_count` scheduling lanes that rayon can
    // drive in parallel.
    let mut worker_buckets: Vec<Vec<String>> = vec![Vec::new(); worker_count];
    for (i, (_key, files)) in buckets.into_iter().enumerate() {
        let slot = i % worker_count;
        worker_buckets[slot].extend(files);
    }
    worker_buckets.retain(|b| !b.is_empty());

    SchedulePlan {
        parallel_buckets: worker_buckets,
        serial,
    }
}

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

/// Compose the `--test-filter` / `--step-filter` CLI shorthand flags into
/// a single wildcard [`Selector`] that applies to every discovered file.
///
/// The step filter accepts a zero-based numeric index or an exact step
/// name, matching [`selector::StepSelector`]'s parse rules. Returns an
/// error when both filters are empty (the caller is expected to skip the
/// synthesis in that case) or when the step value would be ambiguous
/// (empty string).
pub fn build_filter_selector(
    test_filter: Option<&str>,
    step_filter: Option<&str>,
) -> Result<Selector, String> {
    if test_filter.is_none() && step_filter.is_none() {
        return Err("build_filter_selector called without any filter".to_string());
    }
    let test = test_filter
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let step = match step_filter {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err("--step-filter value is empty".to_string());
            }
            if let Ok(idx) = trimmed.parse::<usize>() {
                Some(selector::StepSelector::Index(idx))
            } else {
                Some(selector::StepSelector::Name(trimmed.to_string()))
            }
        }
    };
    Ok(Selector::wildcard(test, step))
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
/// Thin wrapper over [`run_file_with_observers`] that carries only a
/// progress reporter, preserved for call sites that do not need the
/// `events.jsonl` stream (watch mode, library consumers, legacy tests).
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
    let observers = RunObservers::new().with_progress(progress);
    run_file_with_observers(
        test_file,
        file_path,
        env,
        tag_filter,
        selectors,
        opts,
        cookie_jars,
        &observers,
    )
}

/// Run a single test file with an externally managed cookie jar set and
/// a combined [`RunObservers`] bundle (progress reporter + events.jsonl
/// stream). The runner fires both observers at matching lifecycle
/// points; either may be absent.
///
/// When `selectors` is non-empty, only tests and steps matching at least
/// one selector run. Setup and teardown always run for a file that has
/// any matching work, so captures and cleanup behave consistently.
#[allow(clippy::too_many_arguments)]
pub fn run_file_with_observers(
    test_file: &TestFile,
    file_path: &str,
    env: &HashMap<String, String>,
    tag_filter: &[String],
    selectors: &[Selector],
    opts: &RunOptions,
    cookie_jars: &mut HashMap<String, CookieJar>,
    observers: &RunObservers<'_>,
) -> Result<FileResult, TarnError> {
    let progress = observers.progress;
    let events = observers.events;
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

    // Build interpolation context with resolved env.
    //
    // `optional_unset` tracks captures that were declared `optional:`,
    // backed by `default:` on a missed path, or gated out by `when:` —
    // i.e. the ones that should produce a distinct "declared optional
    // and not set" error when referenced downstream. Kept next to
    // `captures` (not merged into a single enum map) so the existing
    // `HashMap<String, serde_json::Value>` type on `TestResult` and the
    // `Context` stays source-compatible with consumers that serialize
    // the final captures.
    let mut captures: HashMap<String, serde_json::Value> = HashMap::new();
    let mut optional_unset: HashSet<String> = HashSet::new();

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
    if let Some(ev) = events {
        ev.emit_file_started(file_path);
    }

    // Run setup steps
    let setup_results = run_steps(
        &test_file.setup,
        env,
        &mut captures,
        &mut optional_unset,
        test_file,
        &redaction,
        &mut redacted_values,
        &client,
        opts,
        cookies_enabled,
        cookie_jars,
        &base_dir,
        FixtureScope {
            file_path,
            test_label: fixtures::SETUP_TEST_SLUG,
        },
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
    if let Some(ev) = events {
        // Setup steps are reported under the synthetic `__setup__`
        // test label so consumers can correlate them back to the file
        // without inventing a test name. Every real test has its own
        // `test_started` / `test_completed` pair; the synthetic labels
        // carry no such pair.
        emit_step_events(
            ev,
            file_path,
            fixtures::SETUP_TEST_SLUG,
            &test_file.setup,
            &setup_results,
        );
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
            let mut step_optional_unset = optional_unset.clone();
            let step_results = run_steps(
                &selected_steps,
                env,
                &mut step_captures,
                &mut step_optional_unset,
                test_file,
                &redaction,
                &mut redacted_values,
                &client,
                opts,
                cookies_enabled,
                cookie_jars,
                &base_dir,
                FixtureScope {
                    file_path,
                    test_label: fixtures::FLAT_TEST_SLUG,
                },
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
            if let Some(ev) = events {
                // Flat-format files are modeled as a single synthetic test
                // whose name equals the file's own name. test_started is
                // fired just before step events so event order matches
                // ordinary named-test runs.
                ev.emit_test_started(file_path, &test_result.name);
                emit_step_events(
                    ev,
                    file_path,
                    &test_result.name,
                    &selected_steps,
                    &test_result.step_results,
                );
                ev.emit_test_completed(file_path, &test_result);
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
            let mut test_optional_unset = optional_unset.clone();
            let step_results = run_steps(
                &selected_steps,
                env,
                &mut test_captures,
                &mut test_optional_unset,
                test_file,
                &redaction,
                &mut redacted_values,
                &client,
                opts,
                cookies_enabled,
                cookie_jars,
                &base_dir,
                FixtureScope {
                    file_path,
                    test_label: name,
                },
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
            if let Some(ev) = events {
                ev.emit_test_started(file_path, &test_result.name);
                emit_step_events(
                    ev,
                    file_path,
                    &test_result.name,
                    &selected_steps,
                    &test_result.step_results,
                );
                ev.emit_test_completed(file_path, &test_result);
            }
            test_results.push(test_result);
        }
    }

    // Run teardown steps (always, even on failure)
    let teardown_results = run_steps(
        &test_file.teardown,
        env,
        &mut captures,
        &mut optional_unset,
        test_file,
        &redaction,
        &mut redacted_values,
        &client,
        opts,
        cookies_enabled,
        cookie_jars,
        &base_dir,
        FixtureScope {
            file_path,
            test_label: fixtures::TEARDOWN_TEST_SLUG,
        },
    )?;

    if let Some(p) = progress {
        let snapshot: Vec<String> = redacted_values.iter().cloned().collect();
        let ctx = ReportContext {
            redaction: &redaction,
            redacted_values: &snapshot,
        };
        p.teardown_finished(&teardown_results, &ctx);
    }
    if let Some(ev) = events {
        emit_step_events(
            ev,
            file_path,
            fixtures::TEARDOWN_TEST_SLUG,
            &test_file.teardown,
            &teardown_results,
        );
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
    if let Some(ev) = events {
        ev.emit_file_completed(&file_result);
    }

    Ok(file_result)
}

/// Fire `step_started` + `step_completed` events for a sequence of
/// steps, plus synthetic `capture_failure` / `polling_timeout` events
/// derived from the step's `error_category`. Called with the original
/// `Step` slice so the URL and method can be recovered even for steps
/// that never made it past template interpolation (`request_info` is
/// absent on cascade skips and unresolved templates).
fn emit_step_events(
    events: &Arc<EventStream>,
    file_path: &str,
    test_name: &str,
    steps: &[Step],
    results: &[StepResult],
) {
    for (index, result) in results.iter().enumerate() {
        let (method, url) = match &result.request_info {
            Some(info) => (info.method.clone(), info.url.clone()),
            None => {
                // Fall back to the raw, un-interpolated model. Keeps
                // the event populated for cascade-skipped steps so a
                // reader can still see where in the file the skip sat.
                let fallback = steps.get(index);
                (
                    fallback
                        .map(|s| s.request.method.clone())
                        .unwrap_or_default(),
                    fallback.map(|s| s.request.url.clone()).unwrap_or_default(),
                )
            }
        };
        events.emit_step_started(file_path, test_name, index, &result.name, &method, &url);

        // Synthesize derived events from the step's classification.
        // These fire *before* step_completed so a reader sees the
        // diagnostic-rich event first, then the terminal status line —
        // matching how `failures.json` carries the same information.
        if let Some(category) = result.error_category {
            match category {
                FailureCategory::CaptureError | FailureCategory::SkippedDueToFailedCapture => {
                    let (message, missing) = capture_failure_details(result, steps.get(index));
                    events.emit_capture_failure(
                        file_path,
                        test_name,
                        index,
                        &result.name,
                        &message,
                        &missing,
                    );
                }
                FailureCategory::Timeout if is_poll_timeout(result) => {
                    let (attempts, last_status) = poll_metadata(result);
                    events.emit_polling_timeout(
                        crate::report::event_stream::StepCoords {
                            file: file_path,
                            test: test_name,
                            step: &result.name,
                            step_index: index,
                        },
                        crate::report::event_stream::PollingTimeoutInfo {
                            elapsed_ms: result.duration_ms,
                            attempts,
                            last_status,
                        },
                    );
                }
                _ => {}
            }
        }

        events.emit_step_completed(file_path, test_name, index, result);
    }
}

/// Extract the primary human-readable message and the list of missing
/// capture names for a step whose `error_category` signals a capture
/// failure or a cascade skip caused by an upstream capture failure.
fn capture_failure_details(result: &StepResult, step: Option<&Step>) -> (String, Vec<String>) {
    let message = result
        .assertion_results
        .iter()
        .find(|a| !a.passed)
        .map(|a| a.message.clone())
        .unwrap_or_default();
    let missing: Vec<String> = match step {
        // On an actual `CaptureError`, the missing names are the
        // capture keys declared on the step that we could not record.
        Some(s) if matches!(result.error_category, Some(FailureCategory::CaptureError)) => s
            .capture
            .keys()
            .filter(|k| !result.captures_set.iter().any(|set| set == *k))
            .cloned()
            .collect(),
        // On a cascade skip, the assertion's `actual` field holds the
        // upstream name the runner already classified as missing;
        // surface it so the event still carries the correlation key.
        _ => result
            .assertion_results
            .iter()
            .find(|a| !a.passed)
            .and_then(|a| parse_cascade_missing_names(&a.actual))
            .unwrap_or_default(),
    };
    (message, missing)
}

/// Parse the runner's cascade-skip `actual` string, which lists the
/// upstream capture names like `"user_id, email"`. Returns `None` when
/// the string is empty or unparseable so the caller can fall back.
fn parse_cascade_missing_names(actual: &str) -> Option<Vec<String>> {
    let trimmed = actual.trim();
    if trimmed.is_empty() {
        return None;
    }
    let names: Vec<String> = trimmed
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

/// A step's `Timeout` category covers both polling timeouts (at least
/// one assertion is `"poll"`) and transport-level request timeouts.
/// Only the former maps onto the `polling_timeout` event.
fn is_poll_timeout(result: &StepResult) -> bool {
    result
        .assertion_results
        .iter()
        .any(|a| a.assertion == "poll")
}

/// Recover the attempt count and last observed response status for a
/// polling timeout. The runner stamps both into the poll assertion's
/// `actual` field in NAZ-339 as part of the structured timeout
/// diagnostic, but they are not otherwise exposed on `StepResult`; this
/// helper gives a best-effort extraction that never fails the event
/// pipeline — unrecognised shapes yield `(0, None)` so the event still
/// fires, just with weaker metadata.
fn poll_metadata(result: &StepResult) -> (u32, Option<u16>) {
    let last_status = result.response_status;
    let attempts = result
        .assertion_results
        .iter()
        .filter(|a| a.assertion == "poll")
        .count() as u32;
    (attempts.max(1), last_status)
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
    optional_unset: &mut HashSet<String>,
    test_file: &TestFile,
    redaction: &RedactionConfig,
    redacted_values: &mut BTreeSet<String>,
    client: &http::HttpClient,
    opts: &RunOptions,
    cookies_enabled: bool,
    cookie_jars: &mut HashMap<String, CookieJar>,
    base_dir: &Path,
    fixture_scope: FixtureScope<'_>,
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

    for (step_index, step) in steps.iter().enumerate() {
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
            optional_unset,
            test_file,
            redaction,
            redacted_values,
            client,
            opts,
            cookies_enabled,
            cookie_jars,
            base_dir,
        )?;

        // Persist the fixture if enabled. Dry runs, unresolved
        // templates and cascade skips never produce enough data to
        // be worth recording, so we gate the write on the presence
        // of a real `request_info`.
        persist_fixture_for_step(
            opts,
            &fixture_scope,
            step_index,
            &result,
            captures,
            redaction,
            redacted_values,
        );

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

/// Persist a fixture for a freshly-completed step, if fixture
/// writing is enabled.
///
/// Dry-runs and cascade skips never hit this path because the caller
/// gates on either the presence of real `request_info` or the
/// `error_category` of the result. Write failures are logged to
/// stderr; a missing fixture must never block the run itself.
fn persist_fixture_for_step(
    opts: &RunOptions,
    scope: &FixtureScope<'_>,
    step_index: usize,
    result: &StepResult,
    captures: &HashMap<String, serde_json::Value>,
    redaction: &RedactionConfig,
    redacted_values: &BTreeSet<String>,
) {
    if !opts.fixtures.enabled {
        return;
    }
    // Dry runs never produce a fixture worth inspecting — the
    // request was never sent, the response never existed.
    if opts.dry_run {
        return;
    }
    // Skipped steps (`SkippedDueToFailedCapture`, `SkippedDueToFailFast`)
    // carry no request info; recording a fixture for them would
    // overwrite the most recent real fixture with a placeholder.
    if result.request_info.is_none() {
        return;
    }

    let secret_vec: Vec<String> = redacted_values.iter().cloned().collect();
    let mut fixture = fixture_writer::build_fixture(result, redaction, &secret_vec);
    fixture_writer::attach_captures(
        &mut fixture,
        captures,
        &result.captures_set,
        redaction,
        &secret_vec,
    );

    let file_path = Path::new(scope.file_path);
    if let Err(err) = fixture_writer::write_step_fixture(
        &opts.fixtures,
        file_path,
        scope.test_label,
        step_index,
        &fixture,
    ) {
        eprintln!(
            "tarn: fixture write failed for {}::{}::{}: {}",
            scope.file_path, scope.test_label, result.name, err
        );
    }
}

fn skipped_due_to_failed_capture(step: &Step, failed_refs: &[String]) -> StepResult {
    let message = format!(
        "Skipped: step references capture(s) that failed earlier in this test: {}. \
         Fix the root-cause step first — this cascade failure is a direct consequence.",
        failed_refs.join(", ")
    );
    StepResult {
        name: step.name.clone(),
        description: step.description.clone(),
        debug: step.debug,
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
        response_shape_mismatch: None,
    }
}

fn fail_fast_skipped_step(step: &Step) -> StepResult {
    StepResult {
        name: step.name.clone(),
        description: step.description.clone(),
        debug: step.debug,
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
        response_shape_mismatch: None,
    }
}

/// Produce a passing skipped-step result when an inline `if:` /
/// `unless:` predicate selected the skip branch. `passed: true` is
/// deliberate: the step's conditional author asked for the skip, so
/// the test as a whole stays green.
fn condition_skipped_step(step: &Step, message: String) -> StepResult {
    StepResult {
        name: step.name.clone(),
        description: step.description.clone(),
        debug: step.debug,
        passed: true,
        duration_ms: 0,
        assertion_results: vec![AssertionResult::pass("condition", "skip", message)],
        request_info: None,
        response_info: None,
        error_category: Some(FailureCategory::SkippedByCondition),
        response_status: None,
        response_summary: None,
        captures_set: vec![],
        location: step.location.clone(),
        response_shape_mismatch: None,
    }
}

/// Evaluate inline `if:` / `unless:` conditions against the current
/// interpolation context. Returns `Some(StepResult)` when the step
/// should be skipped (already populated with the skip-category
/// result), or `None` when the step should run.
///
/// Truthy rules mirror the user-facing contract in `model::Step`:
///   - unset / missing / optional-unset capture → falsy
///   - empty string → falsy
///   - `"false"` / `"0"` / `"null"` (case-insensitive) → falsy
///   - anything else (including whitespace, non-empty strings,
///     numbers rendered as strings) → truthy
fn evaluate_step_condition(
    step: &Step,
    env: &HashMap<String, String>,
    captures: &HashMap<String, serde_json::Value>,
    optional_unset: &HashSet<String>,
) -> Option<StepResult> {
    if step.run_if.is_none() && step.unless.is_none() {
        return None;
    }
    let ctx = Context {
        env: env.clone(),
        captures: captures.clone(),
        optional_unset: optional_unset.clone(),
    };

    if let Some(ref expr) = step.run_if {
        let rendered = interpolation::interpolate(expr, &ctx);
        let truthy = is_truthy(&rendered);
        if !truthy {
            return Some(condition_skipped_step(
                step,
                format!(
                    "`if:` expression {} evaluated falsy; step skipped.",
                    quote(expr)
                ),
            ));
        }
    }
    if let Some(ref expr) = step.unless {
        let rendered = interpolation::interpolate(expr, &ctx);
        let truthy = is_truthy(&rendered);
        if truthy {
            return Some(condition_skipped_step(
                step,
                format!(
                    "`unless:` expression {} evaluated truthy; step skipped.",
                    quote(expr)
                ),
            ));
        }
    }
    None
}

/// Tarn's cross-the-board truthy rule. Centralizing it here keeps
/// `if:` and `unless:` semantically exact inverses and documents the
/// exact behavior in one place so parity with docs is easy to verify.
fn is_truthy(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    // An unresolved capture/env placeholder that passed through
    // interpolation unchanged is also falsy — users writing
    // `if: "{{ capture.x }}"` expect "if x is set" behavior, which
    // the optional-unset code path already maps to an empty string;
    // but generic missing-capture / missing-env cases leave the
    // literal `{{ ... }}` in place and should behave identically.
    if is_unresolved_placeholder(trimmed) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    !matches!(lower.as_str(), "false" | "0" | "null")
}

/// `true` when the entire string is a single unresolved `{{ expr }}`
/// template. Used by [`is_truthy`] so an `if:` referencing an unset
/// variable evaluates falsy without the user having to pre-default it.
fn is_unresolved_placeholder(s: &str) -> bool {
    let trimmed = s.trim();
    if !trimmed.starts_with("{{") || !trimmed.ends_with("}}") {
        return false;
    }
    // Conservative: only single-expression strings with no surrounding
    // literals. Anything else is user-provided literal-plus-template
    // and should go through the normal truthy rule.
    let inner = &trimmed[2..trimmed.len() - 2];
    !inner.contains("{{") && !inner.contains("}}")
}

fn quote(expr: &str) -> String {
    format!("'{}'", expr)
}

/// Generate a "unresolved template" step result if the request still
/// contains `{{ ... }}` placeholders after interpolation. Splits the
/// diagnostic between two classes — generic unresolved variables
/// (typos, forgotten setup) and variables explicitly declared
/// `optional:` / `when:`-gated in an earlier step — so the user sees
/// the actionable root cause instead of one umbrella message.
fn unresolved_template_step(
    step: &Step,
    request: &PreparedRequest,
    request_info: &RequestInfo,
    ctx: &Context,
) -> Option<StepResult> {
    let mut raw = interpolation::find_unresolved(&request.url);
    for v in request.headers.values() {
        raw.extend(interpolation::find_unresolved(v));
    }
    if let Some(ref body) = request.body {
        raw.extend(interpolation::find_unresolved_in_json(body));
    }
    if let Some(ref form) = request.form {
        for v in form.values() {
            raw.extend(interpolation::find_unresolved(v));
        }
    }
    if raw.is_empty() {
        return None;
    }

    let mut classification = interpolation::classify_unresolved(&raw, ctx);
    classification.optional_unset_refs.sort();
    classification.optional_unset_refs.dedup();
    classification.unresolved.sort();
    classification.unresolved.dedup();

    // Optional-unset references wear a distinct message wording and
    // error category. The runner rule is "optional-unset wins" —
    // when both classes are present, a reference to an optional
    // capture is the more actionable signal (the user made an
    // explicit contract; the next step must handle the absence).
    if !classification.optional_unset_refs.is_empty() {
        let names = classification.optional_unset_refs.join(", ");
        let primary = classification
            .optional_unset_refs
            .first()
            .cloned()
            .unwrap_or_default();
        return Some(StepResult {
            name: step.name.clone(),
            description: step.description.clone(),
            debug: step.debug,
            passed: false,
            duration_ms: 0,
            assertion_results: vec![AssertionResult::fail(
                "interpolation",
                "optional capture is set".to_string(),
                format!("unset: {}", names),
                format!(
                    "template variable '{}' was declared optional and not set. \
                     Gate this step with `if:`/`unless:` or provide a `default:` on the capture.",
                    primary
                ),
            )],
            request_info: Some(request_info.clone()),
            response_info: None,
            error_category: Some(FailureCategory::UnresolvedTemplate),
            response_status: None,
            response_summary: None,
            captures_set: vec![],
            location: step.location.clone(),
            response_shape_mismatch: None,
        });
    }

    let names = classification.unresolved.join(", ");
    Some(StepResult {
        name: step.name.clone(),
        description: step.description.clone(),
        debug: step.debug,
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
        request_info: Some(request_info.clone()),
        response_info: None,
        error_category: Some(FailureCategory::UnresolvedTemplate),
        response_status: None,
        response_summary: None,
        captures_set: vec![],
        location: step.location.clone(),
        response_shape_mismatch: None,
    })
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
        description: step.description.clone(),
        debug: step.debug,
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
        response_shape_mismatch: None,
    }
}

/// NAZ-415: run the shape-drift heuristic against the first
/// JSONPath-based capture spec in `capture_map` and return a
/// diagnosis. This is a best-effort picker — if several captures in
/// the same step each use JSONPath, we diagnose the first one that
/// produces candidates (or, failing that, the first one at all) so
/// the lifted hint matches the dominant cause without requiring the
/// runner to know which specific capture the error message is for.
/// Header / cookie / body / status / url captures are skipped because
/// drift diagnosis is about response-body shape.
fn diagnose_capture_shape_drift(
    capture_map: &HashMap<String, crate::model::CaptureSpec>,
    body: &serde_json::Value,
    ctx: &crate::interpolation::Context,
) -> Option<crate::report::shape_diagnosis::ShapeMismatchDiagnosis> {
    let mut fallback: Option<crate::report::shape_diagnosis::ShapeMismatchDiagnosis> = None;
    for (name, spec) in capture_map {
        let resolved = match capture::resolve_capture_spec_public(name, spec, ctx) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let Some(path) = capture::capture_jsonpath(&resolved) else {
            continue;
        };
        let diag = crate::report::shape_diagnosis::diagnose(path, body);
        if !diag.candidate_fixes.is_empty() {
            return Some(diag);
        }
        if fallback.is_none() {
            fallback = Some(diag);
        }
    }
    fallback
}

/// NAZ-415: propagate the first shape-drift diagnosis attached to a
/// failing assertion up to the step-level `response_shape_mismatch`
/// field so downstream reporting can surface it without scanning
/// every assertion.
///
/// Category upgrade rule: a step's `error_category` is lifted to
/// [`FailureCategory::ResponseShapeMismatch`] **only** when all of the
/// following hold:
///
/// 1. The step is currently categorized as `AssertionFailed` or
///    `CaptureError`. Any other category (network, timeout, parse,
///    cascade skip) is already more specific and wins.
/// 2. The diagnosis has at least one `High` confidence candidate
///    (`high_confidence == true`).
/// 3. Every failing assertion on the step is a pure "no match for
///    path" miss — i.e. every failing assertion carries the same
///    diagnosis. If the step also has a non-drift failure (status
///    mismatch, value mismatch, header miss), we keep the original
///    category so the report doesn't misrepresent a compound failure
///    as "just drift".
fn lift_shape_diagnosis(mut step: StepResult) -> StepResult {
    if step.passed {
        return step;
    }
    let failing: Vec<&AssertionResult> = step.failures();
    if failing.is_empty() {
        return step;
    }
    let all_drift = failing.iter().all(|a| a.response_shape_mismatch.is_some());
    let first_diagnosis = failing
        .iter()
        .find_map(|a| a.response_shape_mismatch.clone());
    let Some(diagnosis) = first_diagnosis else {
        return step;
    };

    // Always surface the observed-shape hint — even for low-confidence
    // drift, agents benefit from "here are the observed keys".
    step.response_shape_mismatch = Some(diagnosis.clone());

    let upgradable = matches!(
        step.error_category,
        Some(FailureCategory::AssertionFailed) | Some(FailureCategory::CaptureError)
    );
    if all_drift && diagnosis.high_confidence && upgradable {
        step.error_category = Some(FailureCategory::ResponseShapeMismatch);
    }
    step
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
        if let Ok(capture::CaptureOutcome::Set(value)) =
            capture::extract_capture(&view, name, spec, ctx)
        {
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

/// Build a [`ResponseInfo`] suitable for report embedding, truncating the
/// body payload if its serialized form exceeds `max_body_bytes`. When the
/// body is truncated we replace it with a JSON string marker so every
/// downstream consumer (JSON, HTML, compact, llm) sees the same shape.
fn build_response_info(response: &http::HttpResponse, max_body_bytes: usize) -> ResponseInfo {
    ResponseInfo {
        status: response.status,
        headers: response.headers.clone(),
        body: Some(truncate_report_body(&response.body, max_body_bytes)),
    }
}

/// Truncate a JSON body for report embedding. Bodies whose serialized
/// form is within `max_bytes` are returned unchanged. Larger bodies are
/// replaced with a string marker so the output stays machine-parseable
/// (`"...<truncated: N bytes>"`).
pub(crate) fn truncate_report_body(
    body: &serde_json::Value,
    max_bytes: usize,
) -> serde_json::Value {
    if max_bytes == 0 {
        return body.clone();
    }
    let serialized = serde_json::to_string(body).unwrap_or_default();
    if serialized.len() <= max_bytes {
        return body.clone();
    }
    // Walk to a UTF-8 boundary so we never slice inside a multi-byte
    // character; serde_json's output is always valid UTF-8.
    let end = serialized
        .char_indices()
        .take_while(|(idx, _)| *idx < max_bytes)
        .last()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    let prefix = &serialized[..end];
    serde_json::Value::String(format!(
        "{}...<truncated: {} bytes>",
        prefix,
        serialized.len()
    ))
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
    optional_unset: &HashSet<String>,
    test_file: &TestFile,
    cookie_jar: Option<&CookieJar>,
) -> PreparedRequest {
    let ctx = Context {
        env: env.clone(),
        captures: captures.clone(),
        optional_unset: optional_unset.clone(),
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
    optional_unset: &mut HashSet<String>,
    test_file: &TestFile,
    redaction: &RedactionConfig,
    redacted_values: &mut BTreeSet<String>,
    client: &http::HttpClient,
    opts: &RunOptions,
    cookies_enabled: bool,
    cookie_jars: &mut HashMap<String, CookieJar>,
    base_dir: &Path,
) -> Result<StepResult, TarnError> {
    // NAZ-415: run the step, then lift any shape-drift diagnosis
    // attached to a failing body/capture assertion onto the
    // step-level `response_shape_mismatch` field. The inner function
    // is the historical body of this fn — this wrapper exists so
    // every return path is post-processed in exactly one place.
    let result = run_step_inner(
        step,
        env,
        captures,
        optional_unset,
        test_file,
        redaction,
        redacted_values,
        client,
        opts,
        cookies_enabled,
        cookie_jars,
        base_dir,
    )?;
    Ok(lift_shape_diagnosis(result))
}

#[allow(clippy::too_many_arguments)]
fn run_step_inner(
    step: &Step,
    env: &HashMap<String, String>,
    captures: &mut HashMap<String, serde_json::Value>,
    optional_unset: &mut HashSet<String>,
    test_file: &TestFile,
    redaction: &RedactionConfig,
    redacted_values: &mut BTreeSet<String>,
    client: &http::HttpClient,
    opts: &RunOptions,
    cookies_enabled: bool,
    cookie_jars: &mut HashMap<String, CookieJar>,
    base_dir: &Path,
) -> Result<StepResult, TarnError> {
    // Evaluate `if:` / `unless:` before anything else so a falsy gate
    // produces a clean skipped step without spending a delay or
    // preparing a request. Interpolation uses the same context as a
    // normal request so `{{ capture.x }}` / `{{ env.y }}` work, and
    // `optional_unset` is honored — an optional capture that never set
    // correctly evaluates to a falsy empty string rather than the raw
    // `{{ capture.x }}` placeholder.
    if let Some(skipped) = evaluate_step_condition(step, env, captures, optional_unset) {
        return Ok(skipped);
    }

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
            optional_unset,
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
        optional_unset,
        test_file,
        jar_name
            .as_ref()
            .and_then(|name| cookie_jars.get(name.as_str())),
    );
    let request_info = build_request_info(step, &request, base_dir);

    // Check for unresolved template expressions (e.g. failed captures, missing env vars).
    // Classify against the live context so a reference to an
    // optional-and-unset capture surfaces a distinct error message
    // instead of the generic "unresolved template" fallback — the
    // shape of the report's failure message is load-bearing for users
    // debugging missing captures.
    if let Some(result) = unresolved_template_step(step, &request, &request_info, &request.ctx) {
        return Ok(result);
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
            description: step.description.clone(),
            debug: step.debug,
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
            response_shape_mismatch: None,
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

            // Extract captures on success — graceful failure (P0 fix).
            // `optional:` / `default:` / `when:` captures that legally
            // miss come back in `optional_unset` so later steps can
            // reference them without triggering an unresolved-template
            // failure.
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
                    Ok(extraction) => {
                        captured_keys = extraction.values.keys().cloned().collect();
                        record_redacted_named_values(
                            &extraction.values,
                            redaction,
                            redacted_values,
                        );
                        captures.extend(extraction.values);
                        for name in extraction.optional_unset {
                            // New optional-unset declarations shadow any
                            // stale concrete value from an earlier step
                            // — the contract is "this step's optional
                            // capture produced no value".
                            captures.remove(&name);
                            optional_unset.insert(name);
                        }
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
                // NAZ-415: if the first failing JSONPath capture missed on
                // an object response, attach a shape-drift diagnosis so
                // the lift pass can surface it at the step level.
                let mut capture_assertion = AssertionResult::fail(
                    "capture",
                    "successful extraction",
                    "extraction failed",
                    format!("{}", capture_err),
                );
                if let Some(diag) =
                    diagnose_capture_shape_drift(&step.capture, &response.body, &request.ctx)
                {
                    capture_assertion = capture_assertion.with_shape_diagnosis(diag);
                }
                all_assertions.push(capture_assertion);
                return Ok(StepResult {
                    name: step.name.clone(),
                    description: step.description.clone(),
                    debug: step.debug,
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
                    response_shape_mismatch: None,
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

            // Always populate response_info so the fixture writer can
            // persist passing responses (for debug / diff / hover in the
            // LSP). Render-time gates in the JSON/LLM formatters decide
            // whether a passing step actually surfaces its response in
            // the CLI output (honors `--verbose-responses` and
            // step-level `debug: true`).
            let response_info = Some(build_response_info(&response, opts.max_body_bytes));

            return Ok(StepResult {
                name: step.name.clone(),
                description: step.description.clone(),
                debug: step.debug,
                passed: all_passed,
                duration_ms: response.duration_ms,
                assertion_results: all_assertions,
                request_info: Some(request_info.clone()),
                response_info,
                error_category: if all_passed {
                    None
                } else {
                    Some(FailureCategory::AssertionFailed)
                },
                response_status: Some(resp_status),
                response_summary: Some(resp_summary),
                captures_set: captured_keys,
                location: step.location.clone(),
                response_shape_mismatch: None,
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
        description: step.description.clone(),
        debug: step.debug,
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
        response_shape_mismatch: None,
    })
}

/// Execute a step with polling: re-execute until `poll.until` assertions pass.
#[allow(clippy::too_many_arguments)]
fn run_step_poll(
    step: &Step,
    poll: &PollConfig,
    env: &HashMap<String, String>,
    captures: &mut HashMap<String, serde_json::Value>,
    optional_unset: &mut HashSet<String>,
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
            optional_unset,
            test_file,
            jar_name
                .as_ref()
                .and_then(|name| cookie_jars.get(name.as_str())),
        );
        let request_info = build_request_info(step, &request, base_dir);

        // Check for unresolved template expressions before sending.
        // Shares its classification with the non-poll path so a single
        // optional-unset message wording serves both entry points.
        if let Some(result) = unresolved_template_step(step, &request, &request_info, &request.ctx)
        {
            return Ok(result);
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
                    Ok(extraction) => {
                        captured_keys = extraction.values.keys().cloned().collect();
                        record_redacted_named_values(
                            &extraction.values,
                            redaction,
                            redacted_values,
                        );
                        captures.extend(extraction.values);
                        for name in extraction.optional_unset {
                            captures.remove(&name);
                            optional_unset.insert(name);
                        }
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
                            description: step.description.clone(),
                            debug: step.debug,
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
                            response_shape_mismatch: None,
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

            // Always populate response_info so the fixture writer has
            // data for passing polled steps too. Render-time gates
            // decide whether the CLI output surfaces the response.
            let response_info = Some(build_response_info(&response, opts.max_body_bytes));

            return Ok(StepResult {
                name: step.name.clone(),
                description: step.description.clone(),
                debug: step.debug,
                passed: all_passed,
                duration_ms: response.duration_ms,
                assertion_results: all_assertions,
                request_info: Some(request_info.clone()),
                response_info,
                error_category: if all_passed {
                    None
                } else {
                    Some(FailureCategory::AssertionFailed)
                },
                response_status: Some(resp_status),
                response_summary: Some(resp_summary),
                captures_set: captured_keys,
                location: step.location.clone(),
                response_shape_mismatch: None,
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
                description: step.description.clone(),
                debug: step.debug,
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
                response_shape_mismatch: None,
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
        description: step.description.clone(),
        debug: step.debug,
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
        response_shape_mismatch: None,
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

/// Direction given by a debug session callback after each step outcome.
///
/// Returned by the closure passed to [`run_test_steps`]. The runner reads
/// this value after each step's [`StepOutcome`] and decides whether to
/// advance to the next step, re-run the current one without advancing, or
/// abort the remaining steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepControl {
    /// Advance to the next step (or stop if the test is finished).
    Continue,
    /// Re-run the current step without advancing the index. Captures and
    /// cookies survive the retry so late-attempt success can still feed
    /// downstream steps.
    Retry,
    /// Abort the remaining steps. The session finishes with
    /// whatever step results have accumulated up to this point.
    Stop,
}

/// Phase identifier so callbacks (and the LSP debug UI) can tell a setup
/// step apart from a "real" test step or a teardown step. The runner
/// emits setup/teardown outcomes too so the debugger can surface captures
/// and responses from shared fixtures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepPhase {
    /// A step in the file's `setup:` block.
    Setup,
    /// A step inside the targeted test (`tests.<name>.steps` or the
    /// file-level flat `steps:`).
    Test,
    /// A step in the file's `teardown:` block. Teardown is emitted even if
    /// a previous step triggered [`StepControl::Stop`] so users can still
    /// see clean-up results.
    Teardown,
}

/// Payload emitted by [`run_test_steps`] between steps. Combines the
/// finished step's raw [`StepResult`], its source phase, and a snapshot of
/// the captures visible after the step ran so the debugger can render a
/// full UI state (captures panel, last-response panel, step counter)
/// without reaching back into runner internals.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    /// Which phase produced this step (setup / test / teardown).
    pub phase: StepPhase,
    /// Zero-based index of the step within its phase's step list.
    pub step_index: usize,
    /// Full `StepResult` — including assertion results, request/response
    /// info, and the list of capture names the step wrote.
    pub result: StepResult,
    /// Snapshot of every capture visible to subsequent steps after this
    /// step finished. Only includes the callers' view of capture state —
    /// the runner's internal machinery (cookies, redaction cache) is not
    /// surfaced here.
    pub captures: HashMap<String, serde_json::Value>,
}

/// Options for [`run_test_steps`]. Separate from [`RunOptions`] because
/// step-by-step runs rarely need parallel cookie jars, multi-file CLI
/// plumbing, etc. — a debug session always targets a single test.
#[derive(Debug, Clone, Default)]
pub struct StepByStepOptions {
    /// Underlying runner options (verbose, dry-run, HTTP transport, etc.).
    pub run: RunOptions,
    /// When true, the teardown phase is skipped. Useful for a debug
    /// session that aborted mid-way and should not mutate external state.
    pub skip_teardown: bool,
    /// When true, the setup phase is skipped. Handy when the caller wants
    /// to single-step through test steps without re-running shared
    /// fixtures — the debugger uses this after a
    /// [`StepControl::Retry`] against a setup step has already rebuilt
    /// the initial state.
    pub skip_setup: bool,
}

/// Outcome of a full step-by-step run. Collected for callers that want
/// to hand the session back to the normal reporting pipeline after a
/// debug session ends.
#[derive(Debug, Clone, Default)]
pub struct StepByStepReport {
    /// Results from setup steps that were actually executed (setup stops
    /// on [`StepControl::Stop`] and does not run when `skip_setup` is
    /// true).
    pub setup_results: Vec<StepResult>,
    /// Results from the targeted test's steps.
    pub test_results: Vec<StepResult>,
    /// Results from teardown steps.
    pub teardown_results: Vec<StepResult>,
    /// Captures visible at the end of the session.
    pub captures: HashMap<String, serde_json::Value>,
    /// True when the callback returned [`StepControl::Stop`] at any
    /// point so the caller can tell an aborted session from a
    /// fully-finished one.
    pub aborted: bool,
}

/// Drive a single test in step-by-step mode, invoking `on_step` after
/// each step completes to decide what to do next.
///
/// The callback receives a [`StepOutcome`] with the finished step's full
/// [`StepResult`] plus a post-step captures snapshot, and returns a
/// [`StepControl`] value that tells the runner whether to advance, retry
/// the current step, or stop.
///
/// This is the core primitive the LSP debug session ([`tarn-lsp`'s
/// `debug_session`]) wraps with channel-based pausing so a human user
/// can step through a Tarn test interactively. The runner here is
/// synchronous: it calls `on_step` inline between steps, and the LSP
/// wrapper turns that inline call into "publish a `tarn/captureState`
/// notification, wait for a `tarn.debug*` command, map it to a
/// `StepControl`".
pub fn run_test_steps<F>(
    test_file: &TestFile,
    file_path: &str,
    env: &HashMap<String, String>,
    test_name: &str,
    opts: &StepByStepOptions,
    mut on_step: F,
) -> Result<StepByStepReport, TarnError>
where
    F: FnMut(&StepOutcome) -> StepControl,
{
    let client = http::HttpClient::new(&opts.run.http)?;
    let redaction = test_file.redaction.clone().unwrap_or_default();
    let mut redacted_values = collect_redacted_env_values(env, &redaction);
    let mut cookie_jars: HashMap<String, CookieJar> = HashMap::new();
    let cookie_mode = effective_cookie_mode(test_file.cookies, opts.run.cookie_jar_per_test);
    let cookies_enabled = cookie_mode != CookieMode::Off;
    let base_dir = Path::new(file_path)
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    // Locate the steps this session should drive. The caller names the
    // enclosing test: either a named group (`tests.<name>`), the file's
    // flat `steps:` block (test_name == test_file.name), or an empty
    // lookup error.
    let test_steps: Vec<Step> = if let Some(group) = test_file.tests.get(test_name) {
        group.steps.clone()
    } else if test_file.name == test_name && !test_file.steps.is_empty() {
        test_file.steps.clone()
    } else {
        return Err(TarnError::Config(format!(
            "run_test_steps: no test named `{}` in `{}`",
            test_name, file_path
        )));
    };

    let mut captures: HashMap<String, serde_json::Value> = HashMap::new();
    let mut optional_unset: HashSet<String> = HashSet::new();
    let mut aborted = false;
    let mut report = StepByStepReport::default();

    // Phase 1: setup
    if !opts.skip_setup && !test_file.setup.is_empty() {
        let outcome = drive_phase_steps(
            &test_file.setup,
            StepPhase::Setup,
            env,
            &mut captures,
            &mut optional_unset,
            test_file,
            &redaction,
            &mut redacted_values,
            &client,
            &opts.run,
            cookies_enabled,
            &mut cookie_jars,
            &base_dir,
            &mut on_step,
        )?;
        report.setup_results = outcome.results;
        aborted |= outcome.aborted;
    }

    // Phase 2: target test. Still run even when setup failed — the LSP
    // debugger wants to show the user each failed step so they can retry
    // after a fix, not silently skip to teardown.
    if !aborted {
        let outcome = drive_phase_steps(
            &test_steps,
            StepPhase::Test,
            env,
            &mut captures,
            &mut optional_unset,
            test_file,
            &redaction,
            &mut redacted_values,
            &client,
            &opts.run,
            cookies_enabled,
            &mut cookie_jars,
            &base_dir,
            &mut on_step,
        )?;
        report.test_results = outcome.results;
        aborted |= outcome.aborted;
    }

    // Phase 3: teardown. Always runs (like the regular runner) unless the
    // caller explicitly disables it. Teardown ignores `aborted` — it's a
    // clean-up pass that should fire whether or not the user stopped the
    // debug session early.
    if !opts.skip_teardown && !test_file.teardown.is_empty() {
        let outcome = drive_phase_steps(
            &test_file.teardown,
            StepPhase::Teardown,
            env,
            &mut captures,
            &mut optional_unset,
            test_file,
            &redaction,
            &mut redacted_values,
            &client,
            &opts.run,
            cookies_enabled,
            &mut cookie_jars,
            &base_dir,
            &mut on_step,
        )?;
        report.teardown_results = outcome.results;
        aborted |= outcome.aborted;
    }

    report.captures = captures;
    report.aborted = aborted;
    Ok(report)
}

/// Execution result of one phase of [`run_test_steps`].
struct DrivenPhaseOutcome {
    results: Vec<StepResult>,
    aborted: bool,
}

/// Run one phase's step list with callback-driven flow control. Shared
/// implementation behind [`run_test_steps`].
#[allow(clippy::too_many_arguments)]
fn drive_phase_steps<F>(
    steps: &[Step],
    phase: StepPhase,
    env: &HashMap<String, String>,
    captures: &mut HashMap<String, serde_json::Value>,
    optional_unset: &mut HashSet<String>,
    test_file: &TestFile,
    redaction: &RedactionConfig,
    redacted_values: &mut BTreeSet<String>,
    client: &http::HttpClient,
    opts: &RunOptions,
    cookies_enabled: bool,
    cookie_jars: &mut HashMap<String, CookieJar>,
    base_dir: &Path,
    on_step: &mut F,
) -> Result<DrivenPhaseOutcome, TarnError>
where
    F: FnMut(&StepOutcome) -> StepControl,
{
    let mut results: Vec<StepResult> = Vec::with_capacity(steps.len());
    let mut aborted = false;
    let mut index = 0;

    while index < steps.len() {
        let step = &steps[index];
        let result = run_step(
            step,
            env,
            captures,
            optional_unset,
            test_file,
            redaction,
            redacted_values,
            client,
            opts,
            cookies_enabled,
            cookie_jars,
            base_dir,
        )?;

        let outcome = StepOutcome {
            phase,
            step_index: index,
            result: result.clone(),
            captures: captures.clone(),
        };

        match on_step(&outcome) {
            StepControl::Continue => {
                results.push(result);
                index += 1;
            }
            StepControl::Retry => {
                // Do not record this result. Captures from the retry will
                // overwrite anything the retried step wrote; the runner's
                // `run_step` is idempotent with respect to captures
                // (they're re-extracted on every call) so rerunning a
                // failed step is safe.
                //
                // We deliberately do not touch `results` so the previous
                // attempt stays out of the final report.
            }
            StepControl::Stop => {
                results.push(result);
                aborted = true;
                break;
            }
        }
    }

    Ok(DrivenPhaseOutcome { results, aborted })
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
            &HashSet::new(),
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
            &HashSet::new(),
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
        let request = prepare_request(
            &tf.steps[0],
            &HashMap::new(),
            &HashMap::new(),
            &HashSet::new(),
            &tf,
            None,
        );

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
            &HashSet::new(),
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
            &HashSet::new(),
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

    // --- NAZ-242: inline if: / unless: truthy semantics ---

    fn build_step_with_condition(run_if: Option<&str>, unless: Option<&str>) -> Step {
        let yaml = r#"
name: Gate
steps:
  - name: step
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let mut tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let step = &mut tf.steps[0];
        step.run_if = run_if.map(str::to_string);
        step.unless = unless.map(str::to_string);
        step.clone()
    }

    #[test]
    fn is_truthy_rules_match_spec() {
        for falsy in ["", "   ", "false", "FALSE", "0", "null", "Null"] {
            assert!(!is_truthy(falsy), "expected {:?} to be falsy", falsy);
        }
        for truthy in ["true", "1", "ok", " anything ", "00", "false-but-not"] {
            assert!(is_truthy(truthy), "expected {:?} to be truthy", truthy);
        }
        assert!(!is_truthy("{{ capture.missing }}"));
        assert!(!is_truthy("{{env.unset}}"));
    }

    #[test]
    fn if_expression_truthy_runs_the_step() {
        let step = build_step_with_condition(Some("{{ capture.request_uuid }}"), None);
        let mut captures = HashMap::new();
        captures.insert("request_uuid".into(), serde_json::json!("abc-123"));
        assert!(
            evaluate_step_condition(&step, &HashMap::new(), &captures, &HashSet::new()).is_none()
        );
    }

    #[test]
    fn if_expression_falsy_unset_skips_the_step() {
        let step = build_step_with_condition(Some("{{ capture.request_uuid }}"), None);
        let result =
            evaluate_step_condition(&step, &HashMap::new(), &HashMap::new(), &HashSet::new())
                .expect("falsy condition must produce a skip");
        assert!(result.passed, "skip-by-condition is not a failure");
        assert_eq!(
            result.error_category,
            Some(FailureCategory::SkippedByCondition)
        );
        assert_eq!(result.error_code(), None);
    }

    #[test]
    fn if_expression_falsy_optional_unset_skips_the_step() {
        let step = build_step_with_condition(Some("{{ capture.maybe }}"), None);
        let mut optional_unset = HashSet::new();
        optional_unset.insert("maybe".to_string());
        let result =
            evaluate_step_condition(&step, &HashMap::new(), &HashMap::new(), &optional_unset)
                .expect("falsy optional-unset must produce a skip");
        assert_eq!(
            result.error_category,
            Some(FailureCategory::SkippedByCondition)
        );
    }

    #[test]
    fn unless_is_the_inverse_of_if() {
        let step = build_step_with_condition(None, Some("{{ capture.request_uuid }}"));
        let mut captures = HashMap::new();
        captures.insert("request_uuid".into(), serde_json::json!("abc-123"));
        let skipped = evaluate_step_condition(&step, &HashMap::new(), &captures, &HashSet::new())
            .expect("truthy unless must skip");
        assert_eq!(
            skipped.error_category,
            Some(FailureCategory::SkippedByCondition)
        );

        assert!(
            evaluate_step_condition(&step, &HashMap::new(), &HashMap::new(), &HashSet::new())
                .is_none()
        );
    }

    #[test]
    fn no_condition_means_no_short_circuit() {
        let step = build_step_with_condition(None, None);
        assert!(
            evaluate_step_condition(&step, &HashMap::new(), &HashMap::new(), &HashSet::new())
                .is_none()
        );
    }

    // ---- NAZ-249 Scheduler: plan_schedule + SchedulingMetadata ----

    fn meta(file: &str, serial_only: bool, group: Option<&str>) -> SchedulingMetadata {
        SchedulingMetadata {
            file: file.to_string(),
            serial_only,
            group: group.map(|g| g.to_string()),
        }
    }

    #[test]
    fn plan_schedule_partitions_serial_only_files_onto_serial_bucket() {
        let metadata = vec![
            meta("a.tarn.yaml", false, None),
            meta("b.tarn.yaml", true, None),
            meta("c.tarn.yaml", false, None),
            meta("d.tarn.yaml", true, None),
        ];

        let plan = plan_schedule(&metadata, 3);

        assert_eq!(plan.serial, vec!["b.tarn.yaml", "d.tarn.yaml"]);
        assert_eq!(plan.parallel_buckets.len(), 2);
        let parallel_files: BTreeSet<String> =
            plan.parallel_buckets.iter().flatten().cloned().collect();
        assert_eq!(
            parallel_files,
            BTreeSet::from(["a.tarn.yaml".into(), "c.tarn.yaml".into()])
        );
    }

    #[test]
    fn plan_schedule_groups_same_group_onto_single_bucket() {
        let metadata = vec![
            meta("pg1.tarn.yaml", false, Some("pg")),
            meta("other.tarn.yaml", false, None),
            meta("pg2.tarn.yaml", false, Some("pg")),
            meta("another.tarn.yaml", false, None),
            meta("pg3.tarn.yaml", false, Some("pg")),
        ];

        let plan = plan_schedule(&metadata, 4);

        assert!(plan.serial.is_empty());
        let pg_bucket = plan
            .parallel_buckets
            .iter()
            .find(|b| b.iter().any(|f| f == "pg1.tarn.yaml"))
            .expect("pg bucket should exist");
        assert_eq!(
            pg_bucket,
            &vec![
                "pg1.tarn.yaml".to_string(),
                "pg2.tarn.yaml".to_string(),
                "pg3.tarn.yaml".to_string()
            ]
        );
        assert!(!pg_bucket.iter().any(|f| f == "other.tarn.yaml"));
        assert!(!pg_bucket.iter().any(|f| f == "another.tarn.yaml"));
    }

    #[test]
    fn plan_schedule_separate_groups_parallelize_across_buckets() {
        let metadata = vec![
            meta("pg1.tarn.yaml", false, Some("pg")),
            meta("pg2.tarn.yaml", false, Some("pg")),
            meta("s3a.tarn.yaml", false, Some("s3")),
            meta("s3b.tarn.yaml", false, Some("s3")),
        ];

        let plan = plan_schedule(&metadata, 2);
        assert!(plan.serial.is_empty());

        assert_eq!(plan.parallel_buckets.len(), 2);
        for bucket in &plan.parallel_buckets {
            let all_pg = bucket.iter().all(|f| f.starts_with("pg"));
            let all_s3 = bucket.iter().all(|f| f.starts_with("s3"));
            assert!(all_pg || all_s3, "bucket must not mix groups: {:?}", bucket);
        }
    }

    #[test]
    fn plan_schedule_single_worker_collapses_buckets() {
        let metadata = vec![
            meta("a.tarn.yaml", false, None),
            meta("b.tarn.yaml", false, None),
            meta("c.tarn.yaml", false, Some("pg")),
            meta("d.tarn.yaml", false, Some("pg")),
        ];

        let plan = plan_schedule(&metadata, 1);
        assert!(plan.serial.is_empty());
        assert_eq!(plan.parallel_buckets.len(), 1);
        assert_eq!(plan.parallel_buckets[0].len(), 4);
    }

    #[test]
    fn plan_schedule_zero_jobs_is_treated_as_one() {
        let metadata = vec![meta("a.tarn.yaml", false, None)];
        let plan = plan_schedule(&metadata, 0);
        assert_eq!(plan.parallel_buckets.len(), 1);
        assert_eq!(plan.parallel_buckets[0], vec!["a.tarn.yaml".to_string()]);
    }

    #[test]
    fn plan_schedule_empty_input_returns_empty_plan() {
        let plan = plan_schedule(&[], 4);
        assert!(plan.parallel_buckets.is_empty());
        assert!(plan.serial.is_empty());
    }

    #[test]
    fn plan_schedule_all_serial_only_leaves_parallel_empty() {
        let metadata = vec![
            meta("a.tarn.yaml", true, None),
            meta("b.tarn.yaml", true, Some("pg")),
        ];
        let plan = plan_schedule(&metadata, 4);
        assert!(plan.parallel_buckets.is_empty());
        assert_eq!(plan.serial, vec!["a.tarn.yaml", "b.tarn.yaml"]);
    }

    #[test]
    fn plan_schedule_serial_only_preserves_input_order() {
        let metadata = vec![
            meta("z.tarn.yaml", true, None),
            meta("a.tarn.yaml", true, None),
            meta("m.tarn.yaml", true, None),
        ];
        let plan = plan_schedule(&metadata, 2);
        assert_eq!(
            plan.serial,
            vec!["z.tarn.yaml", "a.tarn.yaml", "m.tarn.yaml"]
        );
    }

    #[test]
    fn scheduling_metadata_from_test_file_detects_file_level_serial_only() {
        let yaml = r#"
name: Ser
serial_only: true
steps:
  - name: s
    request:
      method: GET
      url: http://localhost
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let md = SchedulingMetadata::from_test_file("f.tarn.yaml", &tf);
        assert!(md.serial_only);
        assert_eq!(md.group, None);
    }

    #[test]
    fn scheduling_metadata_escalates_when_any_test_is_serial_only() {
        let yaml = r#"
name: Escalate
tests:
  fast:
    steps:
      - name: s
        request:
          method: GET
          url: http://localhost
  slow:
    serial_only: true
    steps:
      - name: s
        request:
          method: GET
          url: http://localhost
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let md = SchedulingMetadata::from_test_file("f.tarn.yaml", &tf);
        assert!(md.serial_only);
    }

    #[test]
    fn scheduling_metadata_reads_group() {
        let yaml = r#"
name: Grouped
group: postgres
steps:
  - name: s
    request:
      method: GET
      url: http://localhost
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let md = SchedulingMetadata::from_test_file("f.tarn.yaml", &tf);
        assert!(!md.serial_only);
        assert_eq!(md.group.as_deref(), Some("postgres"));
    }

    #[test]
    fn scheduling_metadata_defaults_when_unset() {
        let yaml = r#"
name: Default
steps:
  - name: s
    request:
      method: GET
      url: http://localhost
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let md = SchedulingMetadata::from_test_file("f.tarn.yaml", &tf);
        assert!(!md.serial_only);
        assert_eq!(md.group, None);
    }

    // --- NAZ-256: --test-filter / --step-filter shorthand flags ---

    #[test]
    fn build_filter_selector_requires_at_least_one_filter() {
        assert!(build_filter_selector(None, None).is_err());
    }

    #[test]
    fn build_filter_selector_accepts_test_only() {
        let sel = build_filter_selector(Some("login"), None).unwrap();
        assert_eq!(sel.test.as_deref(), Some("login"));
        assert!(sel.step.is_none());
    }

    #[test]
    fn build_filter_selector_maps_numeric_step_to_index() {
        let sel = build_filter_selector(Some("login"), Some("2")).unwrap();
        assert_eq!(sel.step, Some(crate::selector::StepSelector::Index(2)));
    }

    #[test]
    fn build_filter_selector_maps_named_step() {
        let sel = build_filter_selector(Some("login"), Some("bye")).unwrap();
        assert_eq!(
            sel.step,
            Some(crate::selector::StepSelector::Name("bye".into()))
        );
    }

    #[test]
    fn build_filter_selector_rejects_empty_step_value() {
        assert!(build_filter_selector(None, Some("   ")).is_err());
    }

    #[test]
    fn build_filter_selector_produces_wildcard_file() {
        let sel = build_filter_selector(Some("login"), None).unwrap();
        assert!(sel.matches_file("any/path/foo.tarn.yaml"));
        assert!(sel.matches_file("another/bar.tarn.yaml"));
    }

    // --- NAZ-256: run_test_steps callback API ---
    //
    // These tests drive `run_test_steps` against a dry-run test file so
    // no HTTP server is needed. The callback interception runs on every
    // step regardless of whether it actually hit the network; dry-run
    // short-circuits inside `run_step` but still produces a StepResult.

    fn dry_run_test_file() -> crate::model::TestFile {
        let yaml = r#"
name: Debug callback fixture
tests:
  scenario:
    steps:
      - name: s1
        request:
          method: GET
          url: "http://127.0.0.1:1/health"
      - name: s2
        request:
          method: GET
          url: "http://127.0.0.1:1/health"
      - name: s3
        request:
          method: GET
          url: "http://127.0.0.1:1/health"
"#;
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn run_test_steps_continue_runs_every_step_in_order() {
        let tf = dry_run_test_file();
        let mut names = Vec::new();
        let opts = StepByStepOptions {
            run: RunOptions {
                dry_run: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let report = run_test_steps(
            &tf,
            "dry.tarn.yaml",
            &HashMap::new(),
            "scenario",
            &opts,
            |outcome| {
                names.push(outcome.result.name.clone());
                StepControl::Continue
            },
        )
        .unwrap();
        assert_eq!(names, vec!["s1", "s2", "s3"]);
        assert_eq!(report.test_results.len(), 3);
        assert!(!report.aborted);
    }

    #[test]
    fn run_test_steps_stop_short_circuits_remaining() {
        let tf = dry_run_test_file();
        let mut names = Vec::new();
        let opts = StepByStepOptions {
            run: RunOptions {
                dry_run: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let report = run_test_steps(
            &tf,
            "dry.tarn.yaml",
            &HashMap::new(),
            "scenario",
            &opts,
            |outcome| {
                names.push(outcome.result.name.clone());
                if outcome.result.name == "s1" {
                    StepControl::Stop
                } else {
                    StepControl::Continue
                }
            },
        )
        .unwrap();
        assert_eq!(names, vec!["s1"]);
        assert!(report.aborted);
        assert_eq!(report.test_results.len(), 1);
    }

    #[test]
    fn run_test_steps_retry_reruns_current_step_without_advancing() {
        let tf = dry_run_test_file();
        let mut attempts: HashMap<String, u32> = HashMap::new();
        let opts = StepByStepOptions {
            run: RunOptions {
                dry_run: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let report = run_test_steps(
            &tf,
            "dry.tarn.yaml",
            &HashMap::new(),
            "scenario",
            &opts,
            |outcome| {
                let name = outcome.result.name.clone();
                let counter = attempts.entry(name.clone()).or_insert(0);
                *counter += 1;
                if name == "s2" && *counter < 3 {
                    StepControl::Retry
                } else {
                    StepControl::Continue
                }
            },
        )
        .unwrap();

        // Every step runs at least once; s2 runs 3 times (2 retries + 1 final).
        assert_eq!(attempts["s1"], 1);
        assert_eq!(attempts["s2"], 3);
        assert_eq!(attempts["s3"], 1);
        // The final report only records the advancing result — retries
        // are discarded so the final view shows one row per step.
        assert_eq!(report.test_results.len(), 3);
    }

    #[test]
    fn run_test_steps_unknown_test_returns_config_error() {
        let tf = dry_run_test_file();
        let opts = StepByStepOptions::default();
        let err = run_test_steps(
            &tf,
            "dry.tarn.yaml",
            &HashMap::new(),
            "does_not_exist",
            &opts,
            |_| StepControl::Continue,
        )
        .unwrap_err();
        assert!(err.to_string().contains("no test named"));
    }
}
