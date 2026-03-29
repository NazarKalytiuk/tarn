use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use std::path::Path;
use std::process;

use tarn::assert::types::RunResult;
use tarn::bench;
use tarn::env;
use tarn::error::TarnError;
use tarn::parser;
use tarn::report::{self, OutputFormat};
use tarn::runner;

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

        /// Output format: human, json, junit, tap, html
        #[arg(long, default_value = "human")]
        format: String,

        /// Filter by tag (comma-separated, AND logic)
        #[arg(long)]
        tag: Option<String>,

        /// Override environment variables (key=value)
        #[arg(long = "var", value_name = "KEY=VALUE")]
        vars: Vec<String>,

        /// Environment name (loads tarn.env.{name}.yaml)
        #[arg(long = "env")]
        env_name: Option<String>,

        /// Print full request/response for every step
        #[arg(short, long)]
        verbose: bool,

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
    },

    /// Validate test files without running
    Validate {
        /// Test file or directory to validate
        path: Option<String>,
    },

    /// List all tests (dry run)
    List {
        /// Filter by tag
        #[arg(long)]
        tag: Option<String>,
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
            tag,
            vars,
            env_name,
            verbose,
            dry_run,
            watch,
            parallel,
            jobs,
        } => run_command(
            path,
            &format,
            &vars,
            env_name.as_deref(),
            tag.as_deref(),
            verbose,
            dry_run,
            watch,
            parallel,
            jobs,
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
        } => bench_command(
            &path,
            requests,
            concurrency,
            step,
            ramp_up.as_deref(),
            &vars,
            env_name.as_deref(),
            &format,
        ),
        Commands::Validate { path } => validate_command(path),
        Commands::List { tag: _ } => list_command(),
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
    format: &str,
    vars: &[String],
    env_name: Option<&str>,
    tag: Option<&str>,
    verbose: bool,
    dry_run: bool,
    watch: bool,
    parallel: bool,
    jobs: Option<usize>,
) -> i32 {
    let tag_filter = tag.map(runner::parse_tag_filter).unwrap_or_default();
    let output_format = match format.parse::<OutputFormat>() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error: {}. Use: human, json, junit, tap, html", e);
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

    let run_opts = runner::RunOptions { verbose, dry_run };

    // Build the run closure (used by both normal and watch mode)
    let do_run = || {
        execute_run(
            &files,
            &cli_vars,
            env_name,
            &tag_filter,
            &run_opts,
            output_format,
            parallel,
            jobs,
        )
    };

    if watch {
        tarn::watch::run_watch_loop(&files, do_run);
    } else {
        do_run()
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_run(
    files: &[String],
    cli_vars: &[(String, String)],
    env_name: Option<&str>,
    tag_filter: &[String],
    run_opts: &runner::RunOptions,
    output_format: OutputFormat,
    parallel: bool,
    jobs: Option<usize>,
) -> i32 {
    let start = std::time::Instant::now();

    let file_results = if parallel {
        run_files_parallel(files, cli_vars, env_name, tag_filter, run_opts, jobs)
    } else {
        run_files_sequential(files, cli_vars, env_name, tag_filter, run_opts)
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

    let output = report::render(&run_result, output_format);

    if output_format == report::OutputFormat::Html {
        let report_path = std::env::temp_dir().join("tarn-report.html");
        if let Err(e) = std::fs::write(&report_path, &output) {
            eprintln!("Failed to write HTML report: {}", e);
            return 3;
        }
        eprintln!("Report saved to {}", report_path.display());
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(&report_path).spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(&report_path)
                .spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start"])
                .arg(&report_path)
                .spawn();
        }
    } else {
        print!("{}", output);
    }

    if run_result.passed() {
        0
    } else {
        1
    }
}

fn run_files_sequential(
    files: &[String],
    cli_vars: &[(String, String)],
    env_name: Option<&str>,
    tag_filter: &[String],
    run_opts: &runner::RunOptions,
) -> Result<Vec<tarn::assert::types::FileResult>, (i32, String)> {
    let mut results = Vec::new();
    for file_path in files {
        let path = Path::new(file_path);
        let test_file = parser::parse_file(path).map_err(|e| (e.exit_code(), e.to_string()))?;
        let base_dir = path.parent().unwrap_or(Path::new("."));
        let resolved_env = env::resolve_env(&test_file.env, env_name, cli_vars, base_dir)
            .map_err(|e| (e.exit_code(), e.to_string()))?;
        let result = runner::run_file(&test_file, file_path, &resolved_env, tag_filter, run_opts)
            .map_err(|e| (e.exit_code(), e.to_string()))?;
        results.push(result);
    }
    Ok(results)
}

fn run_files_parallel(
    files: &[String],
    cli_vars: &[(String, String)],
    env_name: Option<&str>,
    tag_filter: &[String],
    run_opts: &runner::RunOptions,
    jobs: Option<usize>,
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
            let test_file = parser::parse_file(path).map_err(|e| (e.exit_code(), e.to_string()))?;
            let base_dir = path.parent().unwrap_or(Path::new("."));
            let resolved_env = env::resolve_env(&test_file.env, env_name, cli_vars, base_dir)
                .map_err(|e| (e.exit_code(), e.to_string()))?;
            runner::run_file(&test_file, file_path, &resolved_env, tag_filter, run_opts)
                .map_err(|e| (e.exit_code(), e.to_string()))
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
) -> i32 {
    let cli_vars = match env::parse_cli_vars(vars) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            return e.exit_code();
        }
    };

    let file_path = Path::new(path);
    let test_file = match parser::parse_file(file_path) {
        Ok(tf) => tf,
        Err(e) => {
            eprintln!("Error: {}", e);
            return e.exit_code();
        }
    };

    let base_dir = file_path.parent().unwrap_or(Path::new("."));
    let resolved_env = match env::resolve_env(&test_file.env, env_name, &cli_vars, base_dir) {
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
    };

    match bench::run_bench(&test_file, step_index, &resolved_env, &opts) {
        Ok(result) => {
            let output = match format {
                "json" => bench::render_json(&result),
                _ => bench::render_human(&result),
            };
            print!("{}", output);
            if result.failed == 0 {
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

fn resolve_files(path: Option<String>) -> Result<Vec<String>, TarnError> {
    match path {
        Some(p) => {
            let path = Path::new(&p);
            if path.is_file() {
                Ok(vec![p])
            } else if path.is_dir() {
                runner::discover_test_files(path)
            } else {
                Err(TarnError::Config(format!("Path not found: {}", p)))
            }
        }
        None => {
            // Default: look for tests/ directory or current dir
            let tests_dir = Path::new("tests");
            if tests_dir.is_dir() {
                runner::discover_test_files(tests_dir)
            } else {
                runner::discover_test_files(Path::new("."))
            }
        }
    }
}

fn validate_command(path: Option<String>) -> i32 {
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

    let mut all_valid = true;
    for file_path in &files {
        let path = Path::new(file_path);
        match parser::parse_file(path) {
            Ok(_) => println!("  ✓ {}", file_path),
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

fn list_command() -> i32 {
    let files = match resolve_files(None) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error: {}", e);
            return e.exit_code();
        }
    };

    if files.is_empty() {
        println!("No test files found");
        return 0;
    }

    for file_path in &files {
        let path = Path::new(file_path);
        match parser::parse_file(path) {
            Ok(tf) => {
                println!("{}", file_path);
                println!("  \u{25cf} {}", tf.name);
                if !tf.tags.is_empty() {
                    println!("    tags: {}", tf.tags.join(", "));
                }
                if !tf.setup.is_empty() {
                    println!("    setup: {} step(s)", tf.setup.len());
                }
                for step in &tf.steps {
                    println!("    - {}", step.name);
                }
                for (name, group) in &tf.tests {
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
    let dirs = ["tests"];
    for d in &dirs {
        if let Err(e) = std::fs::create_dir_all(d) {
            eprintln!("Failed to create {}: {}", d, e);
            return 3;
        }
    }

    let example_test = r#"name: Health check
steps:
  - name: GET /health
    request:
      method: GET
      url: "{{ env.base_url }}/health"
    assert:
      status: 200
"#;

    let env_file = r#"base_url: "http://localhost:3000"
"#;

    let config_file = r#"test_dir: "tests"
timeout: 10000
"#;

    let files = [
        ("tests/health.tarn.yaml", example_test),
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

    println!("\nProject initialized! Run `tarn run` to execute tests.");
    0
}
