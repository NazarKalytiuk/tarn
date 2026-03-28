use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Run in watch mode: execute, then re-execute on file changes.
/// The `run_fn` closure is called for each run and returns the exit code.
pub fn run_watch_loop(watch_paths: &[String], run_fn: impl Fn() -> i32) -> ! {
    // Initial run
    clear_screen();
    run_fn();

    let (tx, rx) = mpsc::channel();
    let mut watcher =
        RecommendedWatcher::new(tx, Config::default()).expect("Failed to create file watcher");

    // Watch directories containing test files
    let mut watched = std::collections::HashSet::new();
    for file_path in watch_paths {
        if let Some(dir) = Path::new(file_path).parent() {
            if watched.insert(dir.to_path_buf()) {
                let _ = watcher.watch(dir, RecursiveMode::Recursive);
            }
        }
    }
    // Also watch cwd for env/config files
    let _ = watcher.watch(Path::new("."), RecursiveMode::NonRecursive);

    eprintln!("\n  Watching for changes... (Ctrl+C to stop)\n");

    let debounce = Duration::from_millis(300);
    let mut last_run = Instant::now();

    loop {
        match rx.recv() {
            Ok(Ok(event)) => {
                let dominated = event.paths.iter().any(|p| {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    name.ends_with(".tarn.yaml") || name.starts_with("tarn.env")
                });
                if dominated && last_run.elapsed() > debounce {
                    last_run = Instant::now();
                    clear_screen();
                    run_fn();
                    eprintln!("\n  Watching for changes... (Ctrl+C to stop)\n");
                }
            }
            Ok(Err(e)) => eprintln!("Watch error: {}", e),
            Err(_) => {
                std::process::exit(3);
            }
        }
    }
}

fn clear_screen() {
    eprint!("\x1B[2J\x1B[1;1H");
}
