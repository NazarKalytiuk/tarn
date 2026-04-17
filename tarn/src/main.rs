use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use std::path::{Path, PathBuf};
use std::process;

use tarn::assert::types::{FailureCategory, RunResult, StepResult, TestResult};
use tarn::bench;
use tarn::config::{self, TarnConfig};
use tarn::cookie;
use tarn::env;
use tarn::error::TarnError;
use tarn::format;
use tarn::model::{HttpTransportConfig, HttpVersionPreference, TestFile};

#[cfg(test)]
use tarn::model::Defaults;
use tarn::parser;
use tarn::report::json::JsonOutputMode;
use tarn::report::progress::{HumanProgress, NdjsonProgress, ProgressMode, ProgressReporter};
use tarn::report::{self, OutputFormat, OutputTarget, RenderOptions};
use tarn::runner;
use tarn::selector::{self, Selector};

#[derive(Parser)]
#[command(name = "tarn", version, about = "CLI-first API testing tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run test files
    Run {
        /// Test file or directory to run
        path: Option<String>,

        /// Output format target(s): human, json, junit, tap, html, curl, curl-all, or FORMAT=PATH
        #[arg(long, value_delimiter = ',', default_value = "human")]
        format: Vec<String>,

        /// JSON report mode: verbose (default) or compact
        #[arg(long = "json-mode", default_value = "verbose")]
        json_mode: String,

        /// Filter by tag (comma-separated, AND logic)
        #[arg(long)]
        tag: Option<String>,

        /// Select a single file, test, or step (repeatable).
        /// Form: FILE[::TEST[::STEP]]. STEP may be a name or a 0-based index.
        /// Multiple --select flags union; combined with --tag they AND.
        #[arg(long = "select", value_name = "FILE[::TEST[::STEP]]")]
        select: Vec<String>,

        /// Override environment variables (key=value)
        #[arg(long = "var", value_name = "KEY=VALUE")]
        vars: Vec<String>,

        /// Environment name (loads tarn.env.{name}.yaml)
        #[arg(long = "env")]
        env_name: Option<String>,

        /// Print full request/response for every step
        #[arg(short, long)]
        verbose: bool,

        /// Show only failed tests and steps in the output
        #[arg(long = "only-failed")]
        only_failed: bool,

        /// Disable streaming progress output (print the final report in one batch)
        #[arg(long = "no-progress")]
        no_progress: bool,

        /// Stream NDJSON events to stdout for machine-readable progress.
        /// Mutually exclusive with any --format target that writes to stdout.
        #[arg(long = "ndjson", conflicts_with = "no_progress")]
        ndjson: bool,

        /// Show interpolated requests without sending them
        #[arg(long)]
        dry_run: bool,

        /// Watch for changes and rerun on file save
        #[arg(short, long)]
        watch: bool,

        /// Run test files in parallel
        #[arg(long)]
        parallel: bool,

        /// Number of parallel workers (default: number of CPUs)
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Load and persist cookie jars to a JSON file
        #[arg(long = "cookie-jar")]
        cookie_jar: Option<String>,

        /// Reset the default cookie jar between named tests in each file.
        /// Overrides the file's declared `cookies:` mode (except `off`).
        #[arg(long = "cookie-jar-per-test")]
        cookie_jar_per_test: bool,

        /// Explicit proxy URL for HTTP/HTTPS requests
        #[arg(long)]
        proxy: Option<String>,

        /// Hosts that should bypass the configured proxy
        #[arg(long = "no-proxy")]
        no_proxy: Option<String>,

        /// Additional PEM CA bundle to trust
        #[arg(long)]
        cacert: Option<String>,

        /// Client certificate PEM file
        #[arg(long)]
        cert: Option<String>,

        /// Client private key PEM file
        #[arg(long)]
        key: Option<String>,

        /// Disable TLS certificate and hostname verification
        #[arg(long)]
        insecure: bool,

        /// Force HTTP/1.1
        #[arg(long = "http1.1", conflicts_with = "http2")]
        http1_1: bool,

        /// Force HTTP/2
        #[arg(long, conflicts_with = "http1_1")]
        http2: bool,

        /// Additional header name to redact in reports (repeatable,
        /// case-insensitive). Merges with — never narrows — the built-in
        /// defaults and any `redaction:` block from config or test files.
        #[arg(long = "redact-header", value_name = "NAME")]
        redact_header: Vec<String>,

        /// Include normally-ignored directories (`.git`, `.worktrees`,
        /// `node_modules`, `.venv`/`venv`, `dist`, `build`, `target`, `tmp`,
        /// `.tarn`) during test discovery. Use with care — it commonly
        /// doubles the run by picking up stale worktree copies.
        #[arg(long = "no-default-excludes")]
        no_default_excludes: bool,

        /// Override the path for the always-on JSON artifact. Defaults to
        /// `<project-root>/.tarn/last-run.json`. Every run overwrites the
        /// file so machines can always find the most recent result
        /// without rerunning in `--format json`.
        #[arg(long = "report-json", value_name = "PATH")]
        report_json: Option<PathBuf>,

        /// Skip writing the always-on JSON artifact. Use for transient
        /// runs where persisting the result on disk is undesirable.
        #[arg(long = "no-last-run-json")]
        no_last_run_json: bool,
    },

    /// Validate test files without running
    Validate {
        /// Test file or directory to validate
        path: Option<String>,

        /// Output format: human (default) or json
        #[arg(long, default_value = "human")]
        format: String,

        /// Include normally-ignored directories during discovery (see `run --no-default-excludes`).
        #[arg(long = "no-default-excludes")]
        no_default_excludes: bool,
    },

    /// Format Tarn test files into canonical YAML
    Fmt {
        /// Test file or directory to format
        path: Option<String>,

        /// Check whether files are already formatted without writing changes
        #[arg(long)]
        check: bool,
    },

    /// List all tests (dry run)
    List {
        /// Filter by tag
        #[arg(long)]
        tag: Option<String>,

        /// Restrict listing to a single file (skips workspace discovery)
        #[arg(long)]
        file: Option<PathBuf>,

        /// Output format: human (default) or json
        #[arg(long, default_value = "human")]
        format: String,

        /// Include normally-ignored directories during discovery (see `run --no-default-excludes`).
        #[arg(long = "no-default-excludes")]
        no_default_excludes: bool,
    },

    /// List project-defined named environments
    Env {
        /// Output environment metadata as JSON
        #[arg(long)]
        json: bool,
    },

    /// Convert common-case .hurl files into Tarn YAML
    ImportHurl {
        /// Source .hurl file
        path: String,

        /// Output path for the generated .tarn.yaml file
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Initialize a new Tarn project
    Init,

    /// Benchmark a test step with concurrent requests
    Bench {
        /// Test file to benchmark
        path: String,

        /// Total number of requests
        #[arg(short = 'n', long, default_value = "100")]
        requests: u64,

        /// Number of concurrent workers
        #[arg(short, long, default_value = "10")]
        concurrency: u64,

        /// Step index to benchmark (0-based)
        #[arg(long, default_value = "0")]
        step: usize,

        /// Ramp-up duration (e.g., "5s", "500ms")
        #[arg(long)]
        ramp_up: Option<String>,

        /// Override environment variables
        #[arg(long = "var", value_name = "KEY=VALUE")]
        vars: Vec<String>,

        /// Environment name
        #[arg(long = "env")]
        env_name: Option<String>,

        /// Output format: human, json
        #[arg(long, default_value = "human")]
        format: String,

        /// Additional benchmark exports: json=PATH or csv=PATH
        #[arg(long, value_delimiter = ',')]
        export: Vec<String>,

        /// Fail if throughput drops below this requests/second threshold
        #[arg(long = "fail-under-rps")]
        fail_under_rps: Option<f64>,

        /// Fail if the benchmark error rate rises above this percentage
        #[arg(long = "fail-above-error-rate")]
        fail_above_error_rate: Option<f64>,

        /// Fail if p95 latency rises above this many milliseconds
        #[arg(long = "fail-above-p95-ms")]
        fail_above_p95_ms: Option<u64>,

        /// Fail if p99 latency rises above this many milliseconds
        #[arg(long = "fail-above-p99-ms")]
        fail_above_p99_ms: Option<u64>,

        /// Explicit proxy URL for HTTP/HTTPS requests
        #[arg(long)]
        proxy: Option<String>,

        /// Hosts that should bypass the configured proxy
        #[arg(long = "no-proxy")]
        no_proxy: Option<String>,

        /// Additional PEM CA bundle to trust
        #[arg(long)]
        cacert: Option<String>,

        /// Client certificate PEM file
        #[arg(long)]
        cert: Option<String>,

        /// Client private key PEM file
        #[arg(long)]
        key: Option<String>,

        /// Disable TLS certificate and hostname verification
        #[arg(long)]
        insecure: bool,

        /// Force HTTP/1.1
        #[arg(long = "http1.1", conflicts_with = "http2")]
        http1_1: bool,

        /// Force HTTP/2
        #[arg(long, conflicts_with = "http1_1")]
        http2: bool,
    },

    /// Update tarn to the latest version
    Update {
        /// Check for updates without installing
        #[arg(long)]
        check: bool,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}

fn main() {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        Commands::Run {
            path,
            format,
            json_mode,
            tag,
            select,
            vars,
            env_name,
            verbose,
            only_failed,
            no_progress,
            ndjson,
            dry_run,
            watch,
            parallel,
            jobs,
            cookie_jar,
            cookie_jar_per_test,
            proxy,
            no_proxy,
            cacert,
            cert,
            key,
            insecure,
            http1_1,
            http2,
            redact_header,
            no_default_excludes,
            report_json,
            no_last_run_json,
        } => run_command(
            path,
            &format,
            &json_mode,
            &vars,
            env_name.as_deref(),
            tag.as_deref(),
            &select,
            verbose,
            only_failed,
            no_progress,
            ndjson,
            dry_run,
            watch,
            parallel,
            jobs,
            cookie_jar.as_deref(),
            cookie_jar_per_test,
            HttpTransportConfig {
                proxy,
                no_proxy,
                cacert,
                cert,
                key,
                insecure,
                http_version: cli_http_version(http1_1, http2),
            },
            &redact_header,
            no_default_excludes,
            report_json.as_deref(),
            no_last_run_json,
        ),
        Commands::Bench {
            path,
            requests,
            concurrency,
            step,
            ramp_up,
            vars,
            env_name,
            format,
            export,
            fail_under_rps,
            fail_above_error_rate,
            fail_above_p95_ms,
            fail_above_p99_ms,
            proxy,
            no_proxy,
            cacert,
            cert,
            key,
            insecure,
            http1_1,
            http2,
        } => bench_command(
            &path,
            requests,
            concurrency,
            step,
            ramp_up.as_deref(),
            &vars,
            env_name.as_deref(),
            &format,
            &export,
            bench::BenchThresholds {
                min_throughput_rps: fail_under_rps,
                max_error_rate: fail_above_error_rate,
                max_p95_ms: fail_above_p95_ms,
                max_p99_ms: fail_above_p99_ms,
            },
            &HttpTransportConfig {
                proxy,
                no_proxy,
                cacert,
                cert,
                key,
                insecure,
                http_version: cli_http_version(http1_1, http2),
            },
        ),
        Commands::Validate {
            path,
            format,
            no_default_excludes,
        } => validate_command(path, &format, no_default_excludes),
        Commands::Fmt { path, check } => fmt_command(path, check),
        Commands::List {
            tag,
            file,
            format,
            no_default_excludes,
        } => list_command(
            tag.as_deref(),
            file.as_deref(),
            &format,
            no_default_excludes,
        ),
        Commands::Env { json } => env_command(json),
        Commands::ImportHurl { path, output } => import_hurl_command(&path, output.as_deref()),
        Commands::Init => init_command(),
        Commands::Update { check } => update_command(check),
        Commands::Completions { shell } => {
            generate(shell, &mut Cli::command(), "tarn", &mut std::io::stdout());
            0
        }
    };

    process::exit(exit_code);
}

#[allow(clippy::too_many_arguments)]
fn run_command(
    path: Option<String>,
    format_specs: &[String],
    json_mode: &str,
    vars: &[String],
    env_name: Option<&str>,
    tag: Option<&str>,
    select: &[String],
    verbose: bool,
    only_failed: bool,
    no_progress: bool,
    ndjson: bool,
    dry_run: bool,
    watch: bool,
    parallel: bool,
    jobs: Option<usize>,
    cookie_jar_path: Option<&str>,
    cookie_jar_per_test: bool,
    cli_http_transport: HttpTransportConfig,
    extra_redact_headers: &[String],
    no_default_excludes: bool,
    report_json_path: Option<&Path>,
    no_last_run_json: bool,
) -> i32 {
    let project =
        match load_project_context(path.as_deref().map(Path::new).unwrap_or(Path::new("."))) {
            Ok(project) => project,
            Err(e) => {
                eprintln!("Error: {}", e);
                return e.exit_code();
            }
        };
    let tag_filter = tag.map(runner::parse_tag_filter).unwrap_or_default();
    let selectors = match selector::parse_all(select) {
        Ok(s) => s,
        Err(errs) => {
            for err in errs {
                eprintln!("Error: {}", err);
            }
            return 2;
        }
    };
    let mut output_targets = match parse_output_targets(format_specs) {
        Ok(targets) => targets,
        Err(e) => {
            eprintln!(
                "Error: {}. Use: human, json, junit, tap, html, curl, curl-all, or FORMAT=PATH",
                e
            );
            return 2;
        }
    };

    // Every run produces a machine-readable JSON artifact by default so
    // human-mode runs can still be inspected programmatically after the
    // fact. We resolve the path here (user override > default under the
    // project root) and prepend an explicit OutputTarget — this reuses
    // the existing `emit_run_outputs` plumbing (path resolution, error
    // reporting, the "<format> report saved to …" message) instead of
    // carving out a side-channel writer.
    if !no_last_run_json {
        let artifact_path = match report_json_path {
            Some(p) => p.to_path_buf(),
            None => project.root_dir.join(".tarn").join("last-run.json"),
        };
        output_targets.insert(
            0,
            OutputTarget {
                format: OutputFormat::Json,
                path: Some(artifact_path),
            },
        );
    }
    let json_output_mode = match json_mode.parse::<JsonOutputMode>() {
        Ok(mode) => mode,
        Err(e) => {
            eprintln!("Error: {}. Use: verbose or compact", e);
            return 2;
        }
    };

    let cli_vars = match env::parse_cli_vars(vars) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            return e.exit_code();
        }
    };

    let discovery_report = match resolve_files_with_report(path, no_default_excludes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            return e.exit_code();
        }
    };
    let files = discovery_report.files.clone();

    if files.is_empty() {
        eprintln!("No test files found");
        return 2;
    }

    // Only print the human-mode discovery summary when a human-mode
    // stdout target actually exists: if every output target is a
    // structured file (json=run.json, junit=out.xml, etc.) the summary
    // would be stray noise, and NDJSON requires a clean stream.
    let emits_human_stdout = !ndjson
        && output_targets
            .iter()
            .any(|t| matches!(t.format, OutputFormat::Human) && t.path.is_none());
    if emits_human_stdout {
        if let Some(summary) = format_discovery_summary(&discovery_report, no_default_excludes) {
            println!("{}", summary);
        }
    }

    let run_opts = runner::RunOptions {
        verbose,
        dry_run,
        http: cli_http_transport,
        cookie_jar_per_test,
        fail_fast_within_test: project.config.fail_fast_within_test,
    };
    let effective_parallel = parallel || project.config.parallel;

    if cookie_jar_path.is_some() && effective_parallel {
        eprintln!("Error: `--cookie-jar` is not supported with parallel execution");
        return 2;
    }

    let render_opts = RenderOptions { only_failed };

    // Validate --ndjson does not collide with a non-human stdout format.
    // A stdout-bound `human` target is the default and gets silently
    // dropped by the NDJSON emitter (handled via suppress_stdout_outputs
    // inside execute_run). Any other structured format on stdout would
    // tear the NDJSON stream, so refuse the run.
    if ndjson {
        let conflicting_stdout_format = output_targets.iter().any(|t| {
            t.writes_to_stdout() && t.path.is_none() && !matches!(t.format, OutputFormat::Human)
        });
        if conflicting_stdout_format {
            eprintln!(
                "Error: --ndjson writes to stdout and conflicts with another --format target that also writes to stdout. Route the other format to a file (e.g. --format json=run.json)."
            );
            return 2;
        }
    }

    // Build the run closure (used by both normal and watch mode)
    let do_run = |run_files: &[String]| {
        execute_run(
            run_files,
            &cli_vars,
            env_name,
            &tag_filter,
            &selectors,
            &run_opts,
            &output_targets,
            json_output_mode,
            render_opts,
            no_progress,
            ndjson,
            cookie_jar_path,
            effective_parallel,
            jobs,
            extra_redact_headers,
        )
    };

    if watch {
        tarn::watch::run_watch_loop(&files, do_run);
    } else {
        do_run(&files)
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_run(
    files: &[String],
    cli_vars: &[(String, String)],
    env_name: Option<&str>,
    tag_filter: &[String],
    selectors: &[Selector],
    run_opts: &runner::RunOptions,
    output_targets: &[OutputTarget],
    json_output_mode: JsonOutputMode,
    render_opts: RenderOptions,
    no_progress: bool,
    ndjson: bool,
    cookie_jar_path: Option<&str>,
    parallel: bool,
    jobs: Option<usize>,
    extra_redact_headers: &[String],
) -> i32 {
    let start = std::time::Instant::now();

    let human_on_stdout = output_targets.iter().any(|t| {
        matches!(t.format, OutputFormat::Human) && t.writes_to_stdout() && t.path.is_none()
    });

    let progress: Option<Box<dyn ProgressReporter + Send + Sync>> = if ndjson {
        let mode = if parallel {
            ProgressMode::Parallel
        } else {
            ProgressMode::Sequential
        };
        Some(Box::new(NdjsonProgress::new(
            Box::new(std::io::stdout()),
            mode,
        )))
    } else if no_progress {
        None
    } else {
        build_progress_reporter(parallel, human_on_stdout, render_opts)
    };
    let streamed_human_to_stdout = progress.is_some() && !ndjson && human_on_stdout;
    let progress_ref: Option<&(dyn ProgressReporter + Send + Sync)> = progress
        .as_ref()
        .map(|p| p.as_ref() as &(dyn ProgressReporter + Send + Sync));

    let file_results = if parallel {
        run_files_parallel(
            files,
            cli_vars,
            env_name,
            tag_filter,
            selectors,
            run_opts,
            jobs,
            progress_ref,
            extra_redact_headers,
        )
    } else {
        run_files_sequential(
            files,
            cli_vars,
            env_name,
            tag_filter,
            selectors,
            run_opts,
            cookie_jar_path,
            progress_ref,
            extra_redact_headers,
        )
    };

    let file_results = match file_results {
        Ok(r) => r,
        Err((code, msg)) => {
            eprintln!("Error: {}", msg);
            return code;
        }
    };

    let run_result = RunResult {
        file_results,
        duration_ms: start.elapsed().as_millis() as u64,
    };

    if let Some(p) = progress_ref {
        p.run_finished(&run_result);
    }

    // Suppress batch outputs to stdout when --ndjson owns stdout. The final
    // JSON report is still emitted to any file-bound --format target.
    let suppress_stdout_outputs = ndjson;

    if let Err(e) = emit_run_outputs(
        &run_result,
        output_targets,
        json_output_mode,
        render_opts,
        streamed_human_to_stdout,
        suppress_stdout_outputs,
    ) {
        eprintln!("Error: {}", e);
        return 3;
    }

    run_result_exit_code(&run_result)
}

/// Build the appropriate streaming progress reporter based on mode and which
/// format owns stdout. When human is the stdout target, we stream to stdout so
/// the user sees live output; otherwise we stream to stderr to keep stdout clean
/// for structured formats.
fn build_progress_reporter(
    parallel: bool,
    human_on_stdout: bool,
    render_opts: RenderOptions,
) -> Option<Box<dyn ProgressReporter + Send + Sync>> {
    let writer: Box<dyn std::io::Write + Send> = if human_on_stdout {
        Box::new(std::io::stdout())
    } else {
        Box::new(std::io::stderr())
    };
    let mode = if parallel {
        ProgressMode::Parallel
    } else {
        ProgressMode::Sequential
    };
    Some(Box::new(HumanProgress::new(writer, render_opts, mode)))
}

fn parse_output_targets(specs: &[String]) -> Result<Vec<OutputTarget>, String> {
    let targets = specs
        .iter()
        .map(|spec| spec.parse::<OutputTarget>())
        .collect::<Result<Vec<_>, _>>()?;

    let stdout_targets = targets
        .iter()
        .filter(|target| target.writes_to_stdout())
        .count();
    if stdout_targets > 1 {
        return Err(
            "Multiple stdout formats requested. Keep only one bare format and use FORMAT=PATH for additional outputs"
                .into(),
        );
    }

    Ok(targets)
}

fn emit_run_outputs(
    run_result: &RunResult,
    output_targets: &[OutputTarget],
    json_output_mode: JsonOutputMode,
    render_opts: RenderOptions,
    streamed_human_to_stdout: bool,
    suppress_stdout_outputs: bool,
) -> Result<(), String> {
    for target in output_targets {
        if suppress_stdout_outputs && target.writes_to_stdout() && target.path.is_none() {
            continue;
        }
        let is_stdout_human = matches!(target.format, OutputFormat::Human)
            && target.writes_to_stdout()
            && target.path.is_none();
        let output = match target.format {
            OutputFormat::Json => {
                tarn::report::json::render_with_options(run_result, json_output_mode, render_opts)
            }
            OutputFormat::Human if is_stdout_human && streamed_human_to_stdout => {
                tarn::report::human::render_summary(run_result)
            }
            _ => report::render_with_options(run_result, target.format, render_opts),
        };
        match target.format {
            OutputFormat::Html => {
                let report_path = target
                    .path
                    .clone()
                    .unwrap_or_else(|| std::env::temp_dir().join("tarn-report.html"));
                write_output_file(&report_path, &output)
                    .map_err(|e| format!("Failed to write HTML report: {}", e))?;
                eprintln!("HTML report saved to {}", report_path.display());
                if target.path.is_none() {
                    open_report_in_browser(&report_path);
                }
            }
            _ => {
                if let Some(path) = target.path.as_ref() {
                    write_output_file(path, &output).map_err(|e| {
                        format!(
                            "Failed to write {} output to {}: {}",
                            format_name(target.format),
                            path.display(),
                            e
                        )
                    })?;
                    eprintln!(
                        "{} report saved to {}",
                        format_name(target.format),
                        path.display()
                    );
                } else {
                    print!("{}", output);
                }
            }
        }
    }

    Ok(())
}

fn write_output_file(path: &Path, content: &str) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, content)
}

fn format_name(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Human => "human",
        OutputFormat::Json => "json",
        OutputFormat::Junit => "junit",
        OutputFormat::Tap => "tap",
        OutputFormat::Html => "html",
        OutputFormat::Curl => "curl",
        OutputFormat::CurlAll => "curl-all",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchOutputFormat {
    Human,
    Json,
    Csv,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BenchOutputTarget {
    format: BenchOutputFormat,
    path: PathBuf,
}

impl std::str::FromStr for BenchOutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            "csv" => Ok(Self::Csv),
            other => Err(format!("Unknown bench output format: '{}'", other)),
        }
    }
}

impl std::str::FromStr for BenchOutputTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((format, path)) = s.split_once('=') else {
            return Err("Expected FORMAT=PATH".into());
        };
        Ok(Self {
            format: format.parse()?,
            path: PathBuf::from(path),
        })
    }
}

fn open_report_in_browser(report_path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(report_path).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(report_path)
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start"])
            .arg(report_path)
            .spawn();
    }
}

#[allow(clippy::too_many_arguments)]
fn run_files_sequential(
    files: &[String],
    cli_vars: &[(String, String)],
    env_name: Option<&str>,
    tag_filter: &[String],
    selectors: &[Selector],
    run_opts: &runner::RunOptions,
    cookie_jar_path: Option<&str>,
    progress: Option<&(dyn ProgressReporter + Send + Sync)>,
    extra_redact_headers: &[String],
) -> Result<Vec<tarn::assert::types::FileResult>, (i32, String)> {
    let mut results = Vec::new();
    let mut cookie_jars = if let Some(path) = cookie_jar_path {
        cookie::load_named_jars(Path::new(path)).map_err(|e| (e.exit_code(), e.to_string()))?
    } else {
        std::collections::HashMap::new()
    };

    for file_path in files {
        let path = Path::new(file_path);
        let mut test_file = parser::parse_file(path).map_err(|e| (e.exit_code(), e.to_string()))?;
        let project = load_project_context(path.parent().unwrap_or(Path::new(".")))
            .map_err(|e| (e.exit_code(), e.to_string()))?;
        apply_project_defaults(&mut test_file, &project.config);
        apply_cli_extra_redact_headers(&mut test_file, extra_redact_headers);
        let file_run_opts = runner::RunOptions {
            http: resolve_http_transport_config(&project.config, &run_opts.http),
            ..run_opts.clone()
        };
        let resolved_env = resolve_env_for_file(&test_file, path, env_name, cli_vars)
            .map_err(|e| (e.exit_code(), e.to_string()))?;
        let result = runner::run_file_with_cookie_jars(
            &test_file,
            file_path,
            &resolved_env,
            tag_filter,
            selectors,
            &file_run_opts,
            &mut cookie_jars,
            progress,
        )
        .map_err(|e| (e.exit_code(), e.to_string()))?;
        results.push(result);
    }

    if let Some(path) = cookie_jar_path {
        cookie::save_named_jars(Path::new(path), &cookie_jars)
            .map_err(|e| (e.exit_code(), e.to_string()))?;
    }

    Ok(results)
}

#[allow(clippy::too_many_arguments)]
fn run_files_parallel(
    files: &[String],
    cli_vars: &[(String, String)],
    env_name: Option<&str>,
    tag_filter: &[String],
    selectors: &[Selector],
    run_opts: &runner::RunOptions,
    jobs: Option<usize>,
    progress: Option<&(dyn ProgressReporter + Send + Sync)>,
    extra_redact_headers: &[String],
) -> Result<Vec<tarn::assert::types::FileResult>, (i32, String)> {
    use rayon::prelude::*;

    if let Some(j) = jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(j)
            .build_global()
            .ok();
    }

    let mut results: Vec<tarn::assert::types::FileResult> = files
        .par_iter()
        .map(|file_path| {
            let path = Path::new(file_path);
            let mut test_file =
                parser::parse_file(path).map_err(|e| (e.exit_code(), e.to_string()))?;
            let project = load_project_context(path.parent().unwrap_or(Path::new(".")))
                .map_err(|e| (e.exit_code(), e.to_string()))?;
            apply_project_defaults(&mut test_file, &project.config);
            apply_cli_extra_redact_headers(&mut test_file, extra_redact_headers);
            let file_run_opts = runner::RunOptions {
                http: resolve_http_transport_config(&project.config, &run_opts.http),
                ..run_opts.clone()
            };
            let resolved_env = resolve_env_for_file(&test_file, path, env_name, cli_vars)
                .map_err(|e| (e.exit_code(), e.to_string()))?;
            let mut local_jars = std::collections::HashMap::new();
            let result = runner::run_file_with_cookie_jars(
                &test_file,
                file_path,
                &resolved_env,
                tag_filter,
                selectors,
                &file_run_opts,
                &mut local_jars,
                None,
            )
            .map_err(|e| (e.exit_code(), e.to_string()))?;
            if let Some(p) = progress {
                p.file_finished(&result);
            }
            Ok(result)
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Sort for deterministic output
    results.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(results)
}

#[allow(clippy::too_many_arguments)]
fn bench_command(
    path: &str,
    requests: u64,
    concurrency: u64,
    step_index: usize,
    ramp_up: Option<&str>,
    vars: &[String],
    env_name: Option<&str>,
    format: &str,
    export_specs: &[String],
    thresholds: bench::BenchThresholds,
    cli_http_transport: &HttpTransportConfig,
) -> i32 {
    let cli_vars = match env::parse_cli_vars(vars) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            return e.exit_code();
        }
    };

    let file_path = Path::new(path);
    let mut test_file = match parser::parse_file(file_path) {
        Ok(tf) => tf,
        Err(e) => {
            eprintln!("Error: {}", e);
            return e.exit_code();
        }
    };
    let project = match load_project_context(file_path.parent().unwrap_or(Path::new("."))) {
        Ok(project) => project,
        Err(e) => {
            eprintln!("Error: {}", e);
            return e.exit_code();
        }
    };
    apply_project_defaults(&mut test_file, &project.config);
    let http_transport = resolve_http_transport_config(&project.config, cli_http_transport);

    let resolved_env = match resolve_env_for_file(&test_file, file_path, env_name, &cli_vars) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error: {}", e);
            return e.exit_code();
        }
    };

    let ramp_up_duration = ramp_up.and_then(|s| {
        let s = s.trim();
        if let Some(ms) = s.strip_suffix("ms") {
            ms.parse::<u64>().ok().map(std::time::Duration::from_millis)
        } else if let Some(secs) = s.strip_suffix('s') {
            secs.parse::<u64>().ok().map(std::time::Duration::from_secs)
        } else {
            s.parse::<u64>().ok().map(std::time::Duration::from_millis)
        }
    });

    let opts = bench::BenchOptions {
        requests,
        concurrency,
        ramp_up: ramp_up_duration,
        thresholds,
    };
    let export_targets = match parse_bench_output_targets(export_specs) {
        Ok(targets) => targets,
        Err(error) => {
            eprintln!("Error: {}", error);
            return 2;
        }
    };

    match bench::run_bench(
        &test_file,
        step_index,
        &resolved_env,
        &opts,
        &http_transport,
    ) {
        Ok(result) => {
            let output = match format {
                "json" => bench::render_json(&result),
                "csv" => bench::render_csv(&result),
                _ => bench::render_human(&result),
            };
            print!("{}", output);
            if let Err(error) = emit_bench_outputs(&result, &export_targets) {
                eprintln!("Error: {}", error);
                return 3;
            }
            if result.failed == 0 && result.passed_gates {
                0
            } else {
                1
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            e.exit_code()
        }
    }
}

fn parse_bench_output_targets(specs: &[String]) -> Result<Vec<BenchOutputTarget>, String> {
    specs
        .iter()
        .map(|spec| spec.parse::<BenchOutputTarget>())
        .collect()
}

fn emit_bench_outputs(
    result: &bench::BenchResult,
    targets: &[BenchOutputTarget],
) -> Result<(), String> {
    for target in targets {
        let output = match target.format {
            BenchOutputFormat::Human => bench::render_human(result),
            BenchOutputFormat::Json => bench::render_json(result),
            BenchOutputFormat::Csv => bench::render_csv(result),
        };
        write_output_file(&target.path, &output).map_err(|error| {
            format!(
                "Failed to write bench {} output to {}: {}",
                match target.format {
                    BenchOutputFormat::Human => "human",
                    BenchOutputFormat::Json => "json",
                    BenchOutputFormat::Csv => "csv",
                },
                target.path.display(),
                error
            )
        })?;
    }

    Ok(())
}

fn resolve_files(path: Option<String>) -> Result<Vec<String>, TarnError> {
    Ok(resolve_files_with_report(path, false)?.files)
}

/// Like [`resolve_files`] but also returns the excluded directories and any
/// detected duplicate `tests/` trees so the caller (currently only the
/// `run` subcommand) can surface them in a discovery summary.
fn resolve_files_with_report(
    path: Option<String>,
    no_default_excludes: bool,
) -> Result<runner::DiscoveryReport, TarnError> {
    let opts = if no_default_excludes {
        runner::DiscoveryOptions {
            ignored_dirs: Vec::new(),
        }
    } else {
        runner::DiscoveryOptions::default()
    };

    match path {
        Some(p) => {
            let path = Path::new(&p);
            if path.is_file() {
                // Explicit single-file inputs bypass the walker: there's
                // nothing to exclude, and the report should still list the
                // file so the downstream caller can treat all code paths
                // uniformly.
                Ok(runner::DiscoveryReport {
                    root: path
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| Path::new(".").to_path_buf()),
                    files: vec![p],
                    excluded_roots: Vec::new(),
                    duplicate_test_trees: Vec::new(),
                })
            } else if path.is_dir() {
                runner::discover_test_files_with_report(path, &opts)
            } else {
                Err(TarnError::Config(format!("Path not found: {}", p)))
            }
        }
        None => {
            let project = load_project_context(Path::new("."))?;
            let tests_dir = project.root_dir.join(&project.config.test_dir);
            if tests_dir.is_dir() {
                runner::discover_test_files_with_report(&tests_dir, &opts)
            } else {
                runner::discover_test_files_with_report(&project.root_dir, &opts)
            }
        }
    }
}

/// Render a one-to-three line discovery summary for human output. Returns
/// `None` when the report is uninteresting (a single-file input, no
/// exclusions, no duplicate trees) so the command stays quiet by default.
fn format_discovery_summary(
    report: &runner::DiscoveryReport,
    no_default_excludes: bool,
) -> Option<String> {
    let has_interesting_content = !report.excluded_roots.is_empty()
        || !report.duplicate_test_trees.is_empty()
        || report.files.len() > 1;
    if !has_interesting_content {
        return None;
    }

    let mut out = String::new();
    out.push_str(&format!(
        "Discovered {} test file{} under {}\n",
        report.files.len(),
        if report.files.len() == 1 { "" } else { "s" },
        report.root.display(),
    ));

    if no_default_excludes {
        out.push_str("  (default excludes disabled — scanning every nested directory)\n");
    } else if !report.excluded_roots.is_empty() {
        // Keep the list compact: most repos will only trip one or two
        // (usually `.git` and `node_modules`). Collapse by basename so
        // twenty `.git` copies deep in worktrees don't flood the console.
        let mut names: Vec<String> = report
            .excluded_roots
            .iter()
            .filter_map(|p| {
                Path::new(p)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .collect();
        names.sort();
        names.dedup();
        out.push_str(&format!("  Excluded: {}\n", names.join(", ")));
    }

    if !report.duplicate_test_trees.is_empty() {
        out.push_str("  Warning: multiple `tests` trees discovered under this root:\n");
        for tree in &report.duplicate_test_trees {
            out.push_str(&format!("    - {}\n", tree));
        }
        out.push_str(
            "  If this was not intentional, pass an explicit path or add the stale copy to your ignores.\n",
        );
    }

    Some(out.trim_end().to_string())
}

fn validate_command(path: Option<String>, format: &str, no_default_excludes: bool) -> i32 {
    let json_format = match format.to_ascii_lowercase().as_str() {
        "human" => false,
        "json" => true,
        other => {
            eprintln!(
                "Error: unknown validate format '{}'. Use 'human' or 'json'.",
                other
            );
            return 2;
        }
    };

    let files = match resolve_files_with_report(path, no_default_excludes) {
        Ok(r) => r.files,
        Err(e) => {
            if json_format {
                let output = serde_json::json!({
                    "files": [],
                    "error": e.to_string(),
                });
                println!("{}", serde_json::to_string_pretty(&output).unwrap());
            } else {
                eprintln!("Error: {}", e);
            }
            return e.exit_code();
        }
    };

    if files.is_empty() {
        if json_format {
            let output = serde_json::json!({ "files": [] });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        } else {
            eprintln!("No test files found");
        }
        return 2;
    }

    if json_format {
        validate_files_json(&files)
    } else {
        validate_files_human(&files)
    }
}

fn validate_files_human(files: &[String]) -> i32 {
    let mut all_valid = true;
    for file_path in files {
        let path = Path::new(file_path);
        match parser::parse_file(path) {
            Ok(_) => {
                println!("  ✓ {}", file_path);
                // Lint pass: surface brittle-pattern warnings after a
                // successful parse. Warnings never fail validation —
                // they are advisory so operators can tighten tests
                // before the next rerun (NAZ-342).
                for warning in collect_lint_warnings(path) {
                    let loc = warning
                        .location
                        .as_ref()
                        .map(|l| format!(" ({}:{}:{})", file_path, l.line, l.column))
                        .unwrap_or_default();
                    println!("    ⚠ warning{}: {}", loc, warning.message);
                }
            }
            Err(e) => {
                println!("  ✗ {}: {}", file_path, e);
                all_valid = false;
            }
        }
    }
    if all_valid {
        0
    } else {
        2
    }
}

/// Rerun just the warning-severity pass of `validate_document` on a
/// parsed file. Returns an empty vector when the file doesn't parse —
/// the caller has already surfaced that error.
fn collect_lint_warnings(path: &Path) -> Vec<tarn::validation::ValidationMessage> {
    let Ok(source) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    tarn::validation::validate_document(path, &source)
        .into_iter()
        .filter(|m| matches!(m.severity, tarn::validation::Severity::Warning))
        .collect()
}

fn validate_files_json(files: &[String]) -> i32 {
    let mut all_valid = true;
    let mut file_entries = Vec::with_capacity(files.len());
    for file_path in files {
        let path = Path::new(file_path);
        let errors = collect_validation_errors(path);
        let valid = errors.is_empty();
        if !valid {
            all_valid = false;
        }
        let error_json: Vec<serde_json::Value> = errors
            .iter()
            .map(|err| {
                let mut obj = serde_json::Map::new();
                obj.insert(
                    "message".into(),
                    serde_json::Value::String(err.message.clone()),
                );
                if let Some(line) = err.line {
                    obj.insert("line".into(), serde_json::Value::from(line));
                }
                if let Some(col) = err.column {
                    obj.insert("column".into(), serde_json::Value::from(col));
                }
                serde_json::Value::Object(obj)
            })
            .collect();
        // Warnings only appear when the file parsed successfully. They
        // are advisory and do not flip `valid` to false.
        let warnings_json: Vec<serde_json::Value> = if valid {
            collect_lint_warnings(path)
                .iter()
                .map(|warning| {
                    let mut obj = serde_json::Map::new();
                    obj.insert(
                        "code".into(),
                        serde_json::Value::String(warning.code.as_str().to_string()),
                    );
                    obj.insert(
                        "message".into(),
                        serde_json::Value::String(warning.message.clone()),
                    );
                    if let Some(loc) = &warning.location {
                        obj.insert("line".into(), serde_json::Value::from(loc.line));
                        obj.insert("column".into(), serde_json::Value::from(loc.column));
                    }
                    serde_json::Value::Object(obj)
                })
                .collect()
        } else {
            Vec::new()
        };
        file_entries.push(serde_json::json!({
            "file": file_path,
            "valid": valid,
            "errors": error_json,
            "warnings": warnings_json,
        }));
    }
    let output = serde_json::json!({ "files": file_entries });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    if all_valid {
        0
    } else {
        2
    }
}

#[derive(Debug)]
struct ValidationError {
    message: String,
    line: Option<usize>,
    column: Option<usize>,
}

/// Collect structured validation errors for a single file.
///
/// When the error originates from serde_yaml's raw parse we extract
/// line and column directly from the error's location. For semantic
/// errors that come out of `parser::parse_file` we attempt to recover
/// line and column by matching the `path:line:column:` prefix that
/// `enhance_parse_error` embeds in its message.
fn collect_validation_errors(path: &Path) -> Vec<ValidationError> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return vec![ValidationError {
                message: format!("Failed to read {}: {}", path.display(), e),
                line: None,
                column: None,
            }]
        }
    };

    if let Err(yaml_err) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
        let location = yaml_err.location();
        return vec![ValidationError {
            message: yaml_err.to_string(),
            line: location.as_ref().map(|l| l.line()),
            column: location.as_ref().map(|l| l.column()),
        }];
    }

    match parser::parse_file(path) {
        Ok(_) => Vec::new(),
        Err(err) => {
            let raw = err.to_string();
            let (message, line, column) = extract_location_prefix(&raw, path);
            vec![ValidationError {
                message,
                line,
                column,
            }]
        }
    }
}

/// Parse the `"<path>:<line>:<column>: <rest>"` prefix that
/// `enhance_parse_error` writes into parser error messages. When the
/// prefix is absent, the full message is returned as the error text with
/// `line` and `column` set to `None`.
fn extract_location_prefix(message: &str, path: &Path) -> (String, Option<usize>, Option<usize>) {
    let prefix = format!("{}:", path.display());
    let Some(rest) = message.strip_prefix(&prefix) else {
        return (message.to_string(), None, None);
    };
    let mut parts = rest.splitn(3, ':');
    let line_part = parts.next();
    let col_part = parts.next();
    let tail = parts.next();
    let (Some(line_str), Some(col_str), Some(tail)) = (line_part, col_part, tail) else {
        return (message.to_string(), None, None);
    };
    let (Ok(line), Ok(col)) = (
        line_str.trim().parse::<usize>(),
        col_str.trim().parse::<usize>(),
    ) else {
        return (message.to_string(), None, None);
    };
    (tail.trim_start().to_string(), Some(line), Some(col))
}

fn fmt_command(path: Option<String>, check: bool) -> i32 {
    let files = match resolve_files(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error: {}", e);
            return e.exit_code();
        }
    };

    if files.is_empty() {
        eprintln!("No test files found");
        return 2;
    }

    let mut changed = Vec::new();
    for file_path in &files {
        let path = Path::new(file_path);
        let original = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) => {
                eprintln!("Error: Failed to read {}: {}", path.display(), e);
                return 2;
            }
        };
        // NAZ-302: route through the shared `tarn::format::format_document`
        // library surface so CLI and LSP share one implementation. The
        // library's "broken YAML → identity no-op" contract is right for
        // the LSP (don't error while the user types) but wrong for the
        // CLI (must still exit 2 on a broken file so CI fails loud). We
        // pre-check YAML parseability and, if the pre-parse fails, route
        // through `parser::format_str` once more to recover the same
        // enhanced "file:line:col" error message the CLI used to emit.
        // On every happy-path call this pre-check is a single O(file)
        // serde_yaml parse, no extra format work.
        if serde_yaml::from_str::<serde_yaml::Value>(&original).is_err() {
            match parser::format_str(&original, path) {
                Ok(_) => unreachable!("format_str cannot succeed after a raw parse failure"),
                Err(e) => {
                    eprintln!("Error: {}", e);
                    return e.exit_code();
                }
            }
        }
        let formatted = match format::format_document(&original) {
            Ok(formatted) => formatted,
            Err(e) => {
                eprintln!("Error: Parse error: {}: {}", path.display(), e);
                return 2;
            }
        };

        if original != formatted {
            changed.push(file_path.clone());
            if !check {
                if let Err(e) = std::fs::write(path, formatted) {
                    eprintln!("Error: Failed to write {}: {}", path.display(), e);
                    return 2;
                }
                println!("formatted {}", file_path);
            }
        }
    }

    if check {
        if changed.is_empty() {
            println!("All Tarn files are already formatted");
            0
        } else {
            for file in &changed {
                println!("needs formatting {}", file);
            }
            1
        }
    } else {
        if changed.is_empty() {
            println!("All Tarn files already formatted");
        }
        0
    }
}

fn list_command(
    tag: Option<&str>,
    file: Option<&Path>,
    format: &str,
    no_default_excludes: bool,
) -> i32 {
    let json_format = match format.to_ascii_lowercase().as_str() {
        "human" => false,
        "json" => true,
        other => {
            eprintln!(
                "Error: unknown list format '{}'. Use 'human' or 'json'.",
                other
            );
            return 2;
        }
    };

    let tag_filter = tag.map(runner::parse_tag_filter).unwrap_or_default();

    let files = match file {
        Some(path) => match resolve_single_list_file(path) {
            Ok(paths) => paths,
            Err(e) => {
                if json_format {
                    let output = serde_json::json!({
                        "files": [],
                        "error": e.to_string(),
                    });
                    println!("{}", serde_json::to_string_pretty(&output).unwrap());
                } else {
                    eprintln!("Error: {}", e);
                }
                return e.exit_code();
            }
        },
        None => match resolve_files_with_report(None, no_default_excludes) {
            Ok(r) => r.files,
            Err(e) => {
                if json_format {
                    let output = serde_json::json!({
                        "files": [],
                        "error": e.to_string(),
                    });
                    println!("{}", serde_json::to_string_pretty(&output).unwrap());
                } else {
                    eprintln!("Error: {}", e);
                }
                return e.exit_code();
            }
        },
    };

    if json_format {
        list_files_json(&files, &tag_filter, file.is_some())
    } else {
        list_files_human(&files, &tag_filter)
    }
}

fn resolve_single_list_file(path: &Path) -> Result<Vec<String>, TarnError> {
    if !path.exists() {
        return Err(TarnError::Config(format!(
            "Path not found: {}",
            path.display()
        )));
    }
    if !path.is_file() {
        return Err(TarnError::Config(format!(
            "--file expects a file, not a directory: {}",
            path.display()
        )));
    }
    Ok(vec![path.display().to_string()])
}

fn list_files_human(files: &[String], tag_filter: &[String]) -> i32 {
    if files.is_empty() {
        println!("No test files found");
        return 0;
    }

    for file_path in files {
        let path = Path::new(file_path);
        match parser::parse_file(path) {
            Ok(tf) => {
                let matches_simple =
                    !tf.steps.is_empty() && runner::matches_tags(&tf.tags, tag_filter);
                let matching_groups: Vec<_> = tf
                    .tests
                    .iter()
                    .filter(|(_, group)| {
                        let combined_tags: Vec<String> =
                            tf.tags.iter().chain(group.tags.iter()).cloned().collect();
                        runner::matches_tags(&combined_tags, tag_filter)
                    })
                    .collect();

                if !tag_filter.is_empty() && !matches_simple && matching_groups.is_empty() {
                    continue;
                }

                println!("{}", file_path);
                println!("  \u{25cf} {}", tf.name);
                if !tf.tags.is_empty() {
                    println!("    tags: {}", tf.tags.join(", "));
                }
                if !tf.setup.is_empty() {
                    println!("    setup: {} step(s)", tf.setup.len());
                }
                for step in tf
                    .steps
                    .iter()
                    .filter(|_| matches_simple || tag_filter.is_empty())
                {
                    println!("    - {}", step.name);
                }
                for (name, group) in matching_groups {
                    let desc = group
                        .description
                        .as_deref()
                        .map(|d| format!(" — {}", d))
                        .unwrap_or_default();
                    println!("    {}{}", name, desc);
                    for step in &group.steps {
                        println!("      - {}", step.name);
                    }
                }
                if !tf.teardown.is_empty() {
                    println!("    teardown: {} step(s)", tf.teardown.len());
                }
                println!();
            }
            Err(e) => {
                eprintln!("  ✗ {}: {}", file_path, e);
            }
        }
    }

    0
}

fn list_files_json(files: &[String], tag_filter: &[String], scoped_to_file: bool) -> i32 {
    let mut file_entries: Vec<serde_json::Value> = Vec::with_capacity(files.len());
    let mut had_error = false;

    for file_path in files {
        let path = Path::new(file_path);
        match parser::parse_file(path) {
            Ok(tf) => {
                let matches_simple =
                    !tf.steps.is_empty() && runner::matches_tags(&tf.tags, tag_filter);
                let matching_groups: Vec<(&String, &tarn::model::TestGroup)> = tf
                    .tests
                    .iter()
                    .filter(|(_, group)| {
                        let combined_tags: Vec<String> =
                            tf.tags.iter().chain(group.tags.iter()).cloned().collect();
                        runner::matches_tags(&combined_tags, tag_filter)
                    })
                    .collect();

                if !tag_filter.is_empty() && !matches_simple && matching_groups.is_empty() {
                    continue;
                }

                let steps_json: Vec<serde_json::Value> = tf
                    .steps
                    .iter()
                    .filter(|_| matches_simple || tag_filter.is_empty())
                    .map(|s| serde_json::json!({ "name": s.name }))
                    .collect();

                let setup_json: Vec<serde_json::Value> = tf
                    .setup
                    .iter()
                    .map(|s| serde_json::json!({ "name": s.name }))
                    .collect();

                let teardown_json: Vec<serde_json::Value> = tf
                    .teardown
                    .iter()
                    .map(|s| serde_json::json!({ "name": s.name }))
                    .collect();

                let tests_json: Vec<serde_json::Value> = matching_groups
                    .iter()
                    .map(|(name, group)| {
                        let group_steps: Vec<serde_json::Value> = group
                            .steps
                            .iter()
                            .map(|s| serde_json::json!({ "name": s.name }))
                            .collect();
                        let mut obj = serde_json::Map::new();
                        obj.insert(
                            "name".into(),
                            serde_json::Value::String((*name).to_string()),
                        );
                        if let Some(desc) = group.description.as_deref() {
                            obj.insert(
                                "description".into(),
                                serde_json::Value::String(desc.to_string()),
                            );
                        }
                        obj.insert(
                            "tags".into(),
                            serde_json::Value::Array(
                                group
                                    .tags
                                    .iter()
                                    .cloned()
                                    .map(serde_json::Value::String)
                                    .collect(),
                            ),
                        );
                        obj.insert("steps".into(), serde_json::Value::Array(group_steps));
                        serde_json::Value::Object(obj)
                    })
                    .collect();

                file_entries.push(serde_json::json!({
                    "file": file_path,
                    "name": tf.name,
                    "tags": tf.tags,
                    "setup": setup_json,
                    "steps": steps_json,
                    "tests": tests_json,
                    "teardown": teardown_json,
                }));
            }
            Err(e) => {
                had_error = true;
                file_entries.push(serde_json::json!({
                    "file": file_path,
                    "error": e.to_string(),
                }));
            }
        }
    }

    let output = serde_json::json!({ "files": file_entries });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());

    if had_error && scoped_to_file {
        2
    } else {
        0
    }
}

fn import_hurl_command(path: &str, output: Option<&str>) -> i32 {
    let input_path = Path::new(path);
    let converted = match tarn::hurl_import::convert_file(input_path) {
        Ok(converted) => converted,
        Err(e) => {
            eprintln!("Error: {}", e);
            return e.exit_code();
        }
    };

    let output_path = output
        .map(PathBuf::from)
        .unwrap_or_else(|| default_hurl_output_path(input_path));

    if let Err(e) = write_output_file(&output_path, &converted) {
        eprintln!("Error: Failed to write {}: {}", output_path.display(), e);
        return 3;
    }

    println!(
        "converted {} -> {}",
        input_path.display(),
        output_path.display()
    );
    0
}

fn default_hurl_output_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("converted");
    parent.join(format!("{stem}.tarn.yaml"))
}

#[derive(Debug, Clone)]
struct ProjectContext {
    root_dir: PathBuf,
    config: TarnConfig,
}

fn load_project_context(start_dir: &Path) -> Result<ProjectContext, TarnError> {
    let start_dir = absolute_path(start_dir);
    // Normalize to a directory: callers frequently pass a file path (e.g.
    // `tarn run tests/health.tarn.yaml`) and downstream code treats
    // `root_dir` as a directory for `.join("tests")`,
    // `.join(".tarn/last-run.json")`, etc. Without this step those joins
    // would produce `<file>/…` which is not a valid filesystem path.
    let search_root = if start_dir.is_file() {
        start_dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| start_dir.clone())
    } else {
        start_dir.clone()
    };
    let root_dir = config::find_project_root(&search_root).unwrap_or(search_root);
    let config = config::load_config(&root_dir)?;
    Ok(ProjectContext { root_dir, config })
}

fn resolve_env_for_file(
    test_file: &TestFile,
    file_path: &Path,
    env_name: Option<&str>,
    cli_vars: &[(String, String)],
) -> Result<std::collections::HashMap<String, String>, TarnError> {
    let start_dir = file_path.parent().unwrap_or(Path::new("."));
    let project = load_project_context(start_dir)?;
    env::resolve_env_with_profiles(
        &test_file.env,
        env_name,
        cli_vars,
        &project.root_dir,
        &project.config.env_file,
        &project.config.environments,
    )
}

fn apply_project_defaults(test_file: &mut TestFile, config: &TarnConfig) {
    let project_defaults = config.request_defaults();
    let defaults = test_file
        .defaults
        .get_or_insert_with(|| project_defaults.clone());

    for (key, value) in &project_defaults.headers {
        defaults
            .headers
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
    if defaults.auth.is_none() {
        defaults.auth = project_defaults.auth.clone();
    }
    if defaults.timeout.is_none() {
        defaults.timeout = project_defaults.timeout;
    }
    if defaults.connect_timeout.is_none() {
        defaults.connect_timeout = project_defaults.connect_timeout;
    }
    if defaults.follow_redirects.is_none() {
        defaults.follow_redirects = project_defaults.follow_redirects;
    }
    if defaults.max_redirs.is_none() {
        defaults.max_redirs = project_defaults.max_redirs;
    }
    if defaults.retries.is_none() {
        defaults.retries = project_defaults.retries;
    }
    if defaults.delay.is_none() {
        defaults.delay = project_defaults.delay.clone();
    }

    if test_file.redaction.is_none() {
        test_file.redaction = config.redaction.clone();
    }
}

/// Merge `--redact-header` CLI values into `test_file.redaction` after
/// the project/file redaction config has already been resolved. Uses
/// `RedactionConfig::merge_headers` so the CLI can only widen — never
/// narrow — the effective list, and header matching stays
/// case-insensitive. If the test file has no redaction block yet, a
/// default one is materialized so CLI headers still take effect.
fn apply_cli_extra_redact_headers(test_file: &mut TestFile, extra: &[String]) {
    if extra.is_empty() {
        return;
    }
    let redaction = test_file
        .redaction
        .get_or_insert_with(tarn::model::RedactionConfig::default);
    redaction.merge_headers(extra.iter().map(String::as_str));
}

fn resolve_http_transport_config(
    config: &TarnConfig,
    cli_http_transport: &HttpTransportConfig,
) -> HttpTransportConfig {
    HttpTransportConfig::merge(&config.http_transport(), cli_http_transport)
}

fn cli_http_version(http1_1: bool, http2: bool) -> Option<HttpVersionPreference> {
    if http1_1 {
        Some(HttpVersionPreference::Http1_1)
    } else if http2 {
        Some(HttpVersionPreference::Http2)
    } else {
        None
    }
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn env_command(json: bool) -> i32 {
    let project = match load_project_context(Path::new(".")) {
        Ok(project) => project,
        Err(e) => {
            eprintln!("Error: {}", e);
            return e.exit_code();
        }
    };

    let mut environments: Vec<_> = project.config.environments.iter().collect();
    environments.sort_by(|a, b| a.0.cmp(b.0));

    let redaction = project.config.redaction.clone().unwrap_or_default();

    if json {
        let environments_json: Vec<serde_json::Value> = environments
            .iter()
            .map(|(name, profile)| {
                let source_file = profile
                    .env_file
                    .clone()
                    .unwrap_or_else(|| default_named_env_path(&project.config.env_file, name));
                let redacted_vars = redact_inline_vars(&profile.vars, &redaction);
                serde_json::json!({
                    "name": name,
                    "source_file": source_file,
                    "vars": redacted_vars,
                })
            })
            .collect();

        let output = serde_json::json!({
            "project_root": project.root_dir,
            "default_env_file": project.config.env_file,
            "environments": environments_json,
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
        return 0;
    }

    if environments.is_empty() {
        println!("No named environments configured in tarn.config.yaml");
        return 0;
    }

    println!("Named environments:");
    for (name, profile) in environments {
        println!(
            "  {name:<16} env_file={} vars={}",
            profile
                .env_file
                .clone()
                .unwrap_or_else(|| default_named_env_path(&project.config.env_file, name)),
            profile.vars.len()
        );
    }
    0
}

/// Apply the project redaction policy to a map of inline environment
/// variables from `tarn.config.yaml` so that `tarn env --json` never
/// prints literal secrets. Values for keys listed in
/// `redaction.env_vars` (case-insensitive) are replaced with the
/// configured replacement marker. Keys themselves are preserved so
/// consumers (editors, CI dashboards) still see which variables exist.
fn redact_inline_vars(
    vars: &std::collections::HashMap<String, String>,
    redaction: &tarn::model::RedactionConfig,
) -> std::collections::BTreeMap<String, String> {
    let redact_set: std::collections::HashSet<String> = redaction
        .env_vars
        .iter()
        .map(|k| k.to_ascii_lowercase())
        .collect();
    let mut out = std::collections::BTreeMap::new();
    for (key, value) in vars {
        let redacted = if redact_set.contains(&key.to_ascii_lowercase()) {
            redaction.replacement.clone()
        } else {
            value.clone()
        };
        out.insert(key.clone(), redacted);
    }
    out
}

fn default_named_env_path(env_file: &str, name: &str) -> String {
    let path = Path::new(env_file);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(env_file);
    match path.extension().and_then(|value| value.to_str()) {
        Some(ext) => format!("{stem}.{name}.{ext}"),
        None => format!("{stem}.{name}"),
    }
}

fn run_result_exit_code(run_result: &RunResult) -> i32 {
    let mut exit_code = if run_result.passed() { 0 } else { 1 };

    for step in all_steps(run_result) {
        match step.error_category {
            Some(FailureCategory::ConnectionError)
            | Some(FailureCategory::Timeout)
            | Some(FailureCategory::CaptureError) => return 3,
            Some(FailureCategory::ParseError) | Some(FailureCategory::UnresolvedTemplate) => {
                exit_code = exit_code.max(2)
            }
            // Cascade fallout and fail-fast skips never bump the exit
            // code above 1 — the root-cause step already set the
            // correct category (usually CaptureError → 3). Treating
            // a skip as a fresh runtime error would double-count the
            // same failure. `SkippedByCondition` is a passing skip
            // (inline `if:` / `unless:`), so it is also a no-op here.
            Some(FailureCategory::SkippedDueToFailedCapture)
            | Some(FailureCategory::SkippedDueToFailFast)
            | Some(FailureCategory::SkippedByCondition)
            | Some(FailureCategory::AssertionFailed)
            | None => {}
        }
    }

    exit_code
}

fn all_steps(run_result: &RunResult) -> impl Iterator<Item = &StepResult> {
    run_result.file_results.iter().flat_map(|file| {
        file.setup_results
            .iter()
            .chain(file.test_results.iter().flat_map(steps_from_test))
            .chain(file.teardown_results.iter())
    })
}

fn steps_from_test(test: &TestResult) -> impl Iterator<Item = &StepResult> {
    test.step_results.iter()
}

fn update_command(check_only: bool) -> i32 {
    eprint!("Checking for updates... ");
    let info = match tarn::update::check_for_update() {
        Ok(info) => info,
        Err(e) => {
            eprintln!("failed");
            eprintln!("Error: {}", e);
            return 3;
        }
    };

    if !info.is_newer {
        eprintln!("up to date");
        println!("tarn v{} is the latest version", info.current_version);
        return 0;
    }

    println!("update available");
    println!(
        "  Current: v{}\n  Latest:  v{}",
        info.current_version, info.latest_version
    );

    if check_only {
        return 0;
    }

    if info.download_url.is_none() {
        eprintln!("No binary available for your platform. Build from source instead.");
        return 3;
    }

    println!();
    match tarn::update::perform_update(&info) {
        Ok(()) => {
            println!("\n  Updated tarn to v{}", info.latest_version);
            0
        }
        Err(e) => {
            eprintln!("Update failed: {}", e);
            3
        }
    }
}

fn init_command() -> i32 {
    let dirs = ["tests", "examples", "fixtures"];
    for d in &dirs {
        if let Err(e) = std::fs::create_dir_all(d) {
            eprintln!("Failed to create {}: {}", d, e);
            return 3;
        }
    }

    let env_file = r#"base_url: "http://localhost:3000"

# Optional credentials used by the example templates in ./examples/
admin_email: "admin@example.com"
admin_password: "secret"
alice_email: "alice@example.com"
alice_password: "secret"
bob_email: "bob@example.com"
bob_password: "secret"
"#;

    let config_file = r#"test_dir: "tests"
env_file: "tarn.env.yaml"
timeout: 10000
retries: 0
parallel: false
# proxy: "http://127.0.0.1:8080"
# no_proxy: "localhost,127.0.0.1"
# cacert: "certs/ca.pem"
# cert: "certs/client.pem"
# key: "certs/client-key.pem"
# insecure: false
"#;

    let files = [
        (
            "tests/health.tarn.yaml",
            include_str!("../init-scaffolds/health.tarn.yaml"),
        ),
        (
            "examples/auth-flow.tarn.yaml",
            include_str!("../init-scaffolds/auth-flow.tarn.yaml"),
        ),
        (
            "examples/polling-job.tarn.yaml",
            include_str!("../init-scaffolds/polling-job.tarn.yaml"),
        ),
        (
            "examples/multipart-upload.tarn.yaml",
            include_str!("../init-scaffolds/multipart-upload.tarn.yaml"),
        ),
        (
            "examples/multi-user-session.tarn.yaml",
            include_str!("../init-scaffolds/multi-user-session.tarn.yaml"),
        ),
        (
            "fixtures/upload-demo.txt",
            include_str!("../init-scaffolds/fixtures/upload-demo.txt"),
        ),
        ("tarn.env.yaml", env_file),
        ("tarn.config.yaml", config_file),
    ];

    for (path, content) in &files {
        if Path::new(path).exists() {
            println!("  skip {} (already exists)", path);
        } else {
            if let Err(e) = std::fs::write(path, content) {
                eprintln!("Failed to write {}: {}", path, e);
                return 3;
            }
            println!("  created {}", path);
        }
    }

    println!(
        "\nProject initialized! Start with `tests/health.tarn.yaml`, then adapt the advanced templates in `examples/` for auth, polling, multipart, and multi-user flows."
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use tarn::assert::types::{AssertionResult, FileResult};
    use tempfile::TempDir;

    fn with_current_dir<F>(path: &Path, f: F)
    where
        F: FnOnce(),
    {
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(path).unwrap();
        f();
        std::env::set_current_dir(previous).unwrap();
    }

    #[test]
    fn resolve_env_for_file_uses_project_root_config() {
        let dir = tempfile::tempdir().unwrap();
        let tests_dir = dir.path().join("suite");
        std::fs::create_dir_all(&tests_dir).unwrap();
        std::fs::write(
            dir.path().join("tarn.config.yaml"),
            "test_dir: suite\nenv_file: custom.env.yaml\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("custom.env.yaml"),
            "base_url: http://from-root\n",
        )
        .unwrap();

        let test_path = tests_dir.join("health.tarn.yaml");
        std::fs::write(
            &test_path,
            "name: Health\nsteps:\n  - name: GET\n    request:\n      method: GET\n      url: \"{{ env.base_url }}/health\"\n",
        )
        .unwrap();

        let test_file = parser::parse_file(&test_path).unwrap();
        let env = resolve_env_for_file(&test_file, &test_path, None, &[]).unwrap();

        assert_eq!(env.get("base_url").unwrap(), "http://from-root");
    }

    #[test]
    fn resolve_env_for_file_finds_default_env_root_without_config() {
        let dir = tempfile::tempdir().unwrap();
        let tests_dir = dir.path().join("tests");
        std::fs::create_dir_all(&tests_dir).unwrap();
        std::fs::write(
            dir.path().join("tarn.env.yaml"),
            "base_url: http://from-root\n",
        )
        .unwrap();

        let test_path = tests_dir.join("health.tarn.yaml");
        std::fs::write(
            &test_path,
            "name: Health\nsteps:\n  - name: GET\n    request:\n      method: GET\n      url: \"{{ env.base_url }}/health\"\n",
        )
        .unwrap();

        let test_file = parser::parse_file(&test_path).unwrap();
        let env = resolve_env_for_file(&test_file, &test_path, None, &[]).unwrap();

        assert_eq!(env.get("base_url").unwrap(), "http://from-root");
    }

    #[test]
    fn fmt_command_rewrites_unformatted_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("example.tarn.yaml");
        std::fs::write(
            &file_path,
            r#"
name: Example
steps:
  - request:
      url: "http://localhost:3000"
      method: GET
    name: Check
"#,
        )
        .unwrap();

        let exit_code = fmt_command(Some(file_path.display().to_string()), false);
        assert_eq!(exit_code, 0);

        let formatted = std::fs::read_to_string(&file_path).unwrap();
        assert!(formatted.contains(
            "- name: Check\n  request:\n    method: GET\n    url: http://localhost:3000\n"
        ));
    }

    #[test]
    fn fmt_command_check_detects_unformatted_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("example.tarn.yaml");
        std::fs::write(
            &file_path,
            r#"
name: Example
steps:
  - request:
      url: "http://localhost:3000"
      method: GET
    name: Check
"#,
        )
        .unwrap();

        let exit_code = fmt_command(Some(file_path.display().to_string()), true);
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn init_command_scaffolds_advanced_examples_outside_test_dir() {
        let dir = tempfile::tempdir().unwrap();

        with_current_dir(dir.path(), || {
            let exit_code = init_command();
            assert_eq!(exit_code, 0);
        });

        assert!(dir.path().join("tests/health.tarn.yaml").exists());
        assert!(dir.path().join("examples/auth-flow.tarn.yaml").exists());
        assert!(dir.path().join("examples/polling-job.tarn.yaml").exists());
        assert!(dir
            .path()
            .join("examples/multipart-upload.tarn.yaml")
            .exists());
        assert!(dir
            .path()
            .join("examples/multi-user-session.tarn.yaml")
            .exists());
        assert!(dir.path().join("fixtures/upload-demo.txt").exists());

        let config = std::fs::read_to_string(dir.path().join("tarn.config.yaml")).unwrap();
        assert!(config.contains("test_dir: \"tests\""));
    }

    #[test]
    fn default_hurl_output_path_rewrites_extension() {
        let output = default_hurl_output_path(Path::new("tests/users.hurl"));
        assert_eq!(output, PathBuf::from("tests/users.tarn.yaml"));
    }

    #[test]
    fn run_result_exit_code_prefers_runtime_failure_categories() {
        let make_step = |category| StepResult {
            name: "step".into(),
            description: None,
            passed: false,
            duration_ms: 0,
            assertion_results: vec![AssertionResult::fail("runtime", "ok", "error", "boom")],
            request_info: None,
            response_info: None,
            error_category: category,
            response_status: None,
            response_summary: None,
            captures_set: vec![],
            location: None,
        };

        let make_file = |step| FileResult {
            file: "test.tarn.yaml".into(),
            name: "test".into(),
            passed: false,
            duration_ms: 0,
            redaction: tarn::model::RedactionConfig::default(),
            redacted_values: vec![],
            setup_results: vec![],
            test_results: vec![TestResult {
                name: "group".into(),
                description: None,
                passed: false,
                duration_ms: 0,
                step_results: vec![step],
                captures: std::collections::HashMap::new(),
            }],
            teardown_results: vec![],
        };

        let parse_result = RunResult {
            file_results: vec![make_file(make_step(Some(FailureCategory::ParseError)))],
            duration_ms: 0,
        };
        assert_eq!(run_result_exit_code(&parse_result), 2);

        let runtime_result = RunResult {
            file_results: vec![make_file(make_step(Some(FailureCategory::ConnectionError)))],
            duration_ms: 0,
        };
        assert_eq!(run_result_exit_code(&runtime_result), 3);
    }

    #[test]
    fn parse_output_targets_accepts_multiple_formats_and_paths() {
        let targets = parse_output_targets(&[
            "human".to_string(),
            "json=reports/run.json".to_string(),
            "html=reports/run.html".to_string(),
        ])
        .unwrap();

        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].format, OutputFormat::Human);
        assert_eq!(
            targets[1].path.as_deref(),
            Some(Path::new("reports/run.json"))
        );
        assert_eq!(targets[2].format, OutputFormat::Html);
    }

    #[test]
    fn parse_output_targets_rejects_multiple_stdout_formats() {
        let err = parse_output_targets(&["human".to_string(), "json".to_string()]).unwrap_err();
        assert!(err.contains("Multiple stdout formats requested"));
    }

    #[test]
    fn emit_run_outputs_writes_non_stdout_targets_to_files() {
        let dir = TempDir::new().unwrap();
        let json_path = dir.path().join("reports").join("run.json");
        let html_path = dir.path().join("reports").join("run.html");
        let run_result = RunResult {
            file_results: vec![],
            duration_ms: 12,
        };

        emit_run_outputs(
            &run_result,
            &[
                OutputTarget {
                    format: OutputFormat::Json,
                    path: Some(json_path.clone()),
                },
                OutputTarget {
                    format: OutputFormat::Html,
                    path: Some(html_path.clone()),
                },
            ],
            JsonOutputMode::Verbose,
            RenderOptions::default(),
            false,
            false,
        )
        .unwrap();

        let json_output = std::fs::read_to_string(&json_path).unwrap();
        assert!(json_output.contains("\"schema_version\": 1"));

        let html_output = std::fs::read_to_string(&html_path).unwrap();
        assert!(html_output.starts_with("<!DOCTYPE html>"));
    }

    #[test]
    fn apply_project_defaults_fills_missing_timeout_and_retries() {
        let mut test_file = TestFile {
            version: None,
            name: "test".into(),
            description: None,
            tags: vec![],
            env: Default::default(),
            redaction: None,
            defaults: Some(Defaults {
                headers: Default::default(),
                auth: None,
                timeout: None,
                connect_timeout: None,
                follow_redirects: None,
                max_redirs: None,
                retries: None,
                delay: None,
            }),
            setup: vec![],
            teardown: vec![],
            tests: Default::default(),
            steps: vec![],
            cookies: None,
        };

        apply_project_defaults(
            &mut test_file,
            &TarnConfig {
                test_dir: "tests".into(),
                env_file: "tarn.env.yaml".into(),
                timeout: 1234,
                retries: 2,
                parallel: true,
                defaults: None,
                redaction: None,
                environments: Default::default(),
                proxy: None,
                no_proxy: None,
                cacert: None,
                cert: None,
                key: None,
                insecure: false,
                fail_fast_within_test: false,
            },
        );

        let defaults = test_file.defaults.unwrap();
        assert_eq!(defaults.timeout, Some(1234));
        assert_eq!(defaults.retries, Some(2));
    }

    #[test]
    fn apply_project_defaults_preserves_file_level_values() {
        let mut test_file = TestFile {
            version: None,
            name: "test".into(),
            description: None,
            tags: vec![],
            env: Default::default(),
            redaction: None,
            defaults: Some(Defaults {
                headers: Default::default(),
                auth: None,
                timeout: Some(5000),
                connect_timeout: None,
                follow_redirects: None,
                max_redirs: None,
                retries: Some(4),
                delay: None,
            }),
            setup: vec![],
            teardown: vec![],
            tests: Default::default(),
            steps: vec![],
            cookies: None,
        };

        apply_project_defaults(
            &mut test_file,
            &TarnConfig {
                test_dir: "tests".into(),
                env_file: "tarn.env.yaml".into(),
                timeout: 1234,
                retries: 2,
                parallel: false,
                defaults: Some(tarn::config::ProjectDefaults {
                    headers: [("X-Project".into(), "1".into())].into(),
                    connect_timeout: Some(250),
                    follow_redirects: Some(false),
                    max_redirs: Some(3),
                    delay: Some("100ms".into()),
                    ..Default::default()
                }),
                redaction: Some(tarn::model::RedactionConfig {
                    headers: vec!["authorization".into()],
                    replacement: "[hidden]".into(),
                    env_vars: vec![],
                    captures: vec![],
                }),
                environments: Default::default(),
                proxy: None,
                no_proxy: None,
                cacert: None,
                cert: None,
                key: None,
                insecure: false,
                fail_fast_within_test: false,
            },
        );

        let defaults = test_file.defaults.unwrap();
        assert_eq!(defaults.timeout, Some(5000));
        assert_eq!(defaults.retries, Some(4));
        assert_eq!(defaults.connect_timeout, Some(250));
        assert_eq!(defaults.follow_redirects, Some(false));
        assert_eq!(defaults.max_redirs, Some(3));
        assert_eq!(defaults.delay.as_deref(), Some("100ms"));
        assert_eq!(
            defaults.headers.get("X-Project").map(String::as_str),
            Some("1")
        );
        assert_eq!(test_file.redaction.unwrap().replacement, "[hidden]");
    }

    #[test]
    fn resolve_http_transport_config_prefers_cli_values() {
        let config = TarnConfig {
            test_dir: "tests".into(),
            env_file: "tarn.env.yaml".into(),
            timeout: 10000,
            retries: 0,
            parallel: false,
            defaults: None,
            redaction: None,
            environments: Default::default(),
            proxy: Some("http://project-proxy:8080".into()),
            no_proxy: Some("project.local".into()),
            cacert: Some("project-ca.pem".into()),
            cert: Some("project-cert.pem".into()),
            key: Some("project-key.pem".into()),
            insecure: false,
            fail_fast_within_test: false,
        };

        let transport = resolve_http_transport_config(
            &config,
            &HttpTransportConfig {
                proxy: Some("http://cli-proxy:9090".into()),
                no_proxy: Some("cli.local".into()),
                cacert: Some("cli-ca.pem".into()),
                cert: Some("cli-cert.pem".into()),
                key: Some("cli-key.pem".into()),
                insecure: true,
                http_version: None,
            },
        );

        assert_eq!(transport.proxy.as_deref(), Some("http://cli-proxy:9090"));
        assert_eq!(transport.no_proxy.as_deref(), Some("cli.local"));
        assert_eq!(transport.cacert.as_deref(), Some("cli-ca.pem"));
        assert_eq!(transport.cert.as_deref(), Some("cli-cert.pem"));
        assert_eq!(transport.key.as_deref(), Some("cli-key.pem"));
        assert!(transport.insecure);
    }

    #[test]
    fn resolve_http_transport_config_uses_project_defaults() {
        let config = TarnConfig {
            test_dir: "tests".into(),
            env_file: "tarn.env.yaml".into(),
            timeout: 10000,
            retries: 0,
            parallel: false,
            defaults: None,
            redaction: None,
            environments: Default::default(),
            proxy: Some("http://project-proxy:8080".into()),
            no_proxy: Some("localhost".into()),
            cacert: Some("project-ca.pem".into()),
            cert: Some("project-cert.pem".into()),
            key: Some("project-key.pem".into()),
            insecure: true,
            fail_fast_within_test: false,
        };

        let transport = resolve_http_transport_config(&config, &HttpTransportConfig::default());

        assert_eq!(
            transport.proxy.as_deref(),
            Some("http://project-proxy:8080")
        );
        assert_eq!(transport.no_proxy.as_deref(), Some("localhost"));
        assert_eq!(transport.cacert.as_deref(), Some("project-ca.pem"));
        assert_eq!(transport.cert.as_deref(), Some("project-cert.pem"));
        assert_eq!(transport.key.as_deref(), Some("project-key.pem"));
        assert!(transport.insecure);
    }

    #[test]
    fn cli_http_version_maps_flags() {
        assert_eq!(
            cli_http_version(true, false),
            Some(HttpVersionPreference::Http1_1)
        );
        assert_eq!(
            cli_http_version(false, true),
            Some(HttpVersionPreference::Http2)
        );
        assert_eq!(cli_http_version(false, false), None);
    }
}
