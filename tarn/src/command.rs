//! Shell-command step executor (NAZ-464).
//!
//! `command:` steps run a child process via the platform shell so test
//! authors can prep / stamp / cleanup fixture files inline with their
//! HTTP suite, without dropping out to a wrapper script. The
//! executor is deliberately minimal:
//!
//! * the shell is fixed per platform (`sh -c` on Unix, `cmd /C` on
//!   Windows) — the input string is whatever the user wrote,
//!   post-interpolation;
//! * the child env is scrubbed by default. Only `PATH`, `HOME`
//!   (`USERPROFILE` on Windows), and the platform's temp dir variable
//!   pass through unconditionally; anything else must be allowlisted
//!   via `pass_env:`. Tarn's own `{{ env.x }}` chain is *never*
//!   implicitly forwarded — secrets in `tarn.env.local.yaml` stay
//!   scoped to template interpolation;
//! * captures support stdout regex (capture group 1, or full match)
//!   and the literal exit code. Other shapes are rejected at parse
//!   time.
//!
//! The runner-level policy gate (`--allow-exec` /
//! `tarn.config.yaml: allow_exec`) is enforced before this module
//! runs — once we get here, execution is authorized.

use crate::interpolation::{self, Context};
use crate::model::{CommandCaptureSpec, CommandStep};
use crate::regex_cache;
use indexmap::IndexMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of executing a single `command:` step.
#[derive(Debug, Clone)]
pub struct CommandRunResult {
    /// Exit code reported by the child process. `None` only when the
    /// child was killed by a signal (Unix) — the executor surfaces
    /// that as a structured failure, so report consumers always see
    /// either a code or a `signal_terminated` flag.
    pub exit_code: Option<i32>,
    /// Whether the child was terminated by a signal (Unix only).
    pub signal_terminated: bool,
    /// Captured stdout (utf-8 lossy decode — see note below).
    pub stdout: String,
    /// Captured stderr (utf-8 lossy decode).
    pub stderr: String,
    /// Effective command string after template interpolation. Used by
    /// the runner to populate the report's "what we actually ran"
    /// breadcrumb.
    pub interpolated_run: String,
    /// Capture results extracted from the child output, in declaration
    /// order. `None` values mean the source missed *and* the capture
    /// was declared `optional: true` — caller treats that as
    /// optional-unset, matching HTTP-step semantics.
    pub captures: IndexMap<String, Option<serde_json::Value>>,
    /// Capture names that fell back to optional-unset because the
    /// regex did not match. The runner threads these into the
    /// shared `optional_unset` set so downstream `{{ capture.x }}`
    /// references surface a precise diagnostic.
    pub optional_unset: Vec<String>,
    /// Wallclock duration of the child process.
    pub duration_ms: u64,
}

/// Why a `command:` step ultimately failed. The runner converts these
/// into a single `AssertionResult` so the existing per-step report
/// shape carries the failure reason without inventing a new field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandFailureReason {
    /// Child process exited with a non-zero status.
    NonZeroExit { code: i32 },
    /// Child was terminated by a signal (Unix only).
    SignalTerminated,
    /// `stdout_regex:` capture missed and `optional:` was not set.
    CaptureMissed { name: String, pattern: String },
    /// We could not even spawn the shell — this is a runner-level
    /// failure, distinct from a normal non-zero exit.
    SpawnFailed { message: String },
    /// User-supplied regex did not compile. Caught here rather than at
    /// parse time so the diagnostic carries the step's location.
    InvalidRegex { name: String, error: String },
}

/// Top-level executor entry point. Interpolates the command spec
/// against `ctx`, spawns the child via the platform shell, applies
/// captures, and returns either a success result or a structured
/// failure.
pub fn run_command(
    spec: &CommandStep,
    ctx: &Context,
    base_dir: &Path,
    parent_env: &dyn ParentEnv,
) -> Result<CommandRunResult, CommandFailureReason> {
    let interpolated_run = interpolation::interpolate(&spec.run, ctx);

    let workdir: PathBuf = match spec.workdir.as_deref() {
        Some(raw) => {
            let interpolated = interpolation::interpolate(raw, ctx);
            let path = Path::new(&interpolated);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                base_dir.join(path)
            }
        }
        None => base_dir.to_path_buf(),
    };
    // Path::new("file.yaml").parent() returns `""` rather than `.`, and
    // `Command::current_dir("")` makes the spawn fail with ENOENT on
    // Unix. Fall back to the runner's current directory whenever the
    // base path lands empty so a relative invocation
    // (`tarn run exec.tarn.yaml`) keeps working.
    let workdir: PathBuf = if workdir.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        workdir
    };

    let allowlist: Vec<String> = spec
        .pass_env
        .iter()
        .map(|name| interpolation::interpolate(name, ctx))
        .collect();
    let scrubbed_env = build_child_env(&allowlist, parent_env);

    let (program, shell_arg) = shell_invocation();
    let started = std::time::Instant::now();
    let output = Command::new(program)
        .arg(shell_arg)
        .arg(&interpolated_run)
        .current_dir(&workdir)
        .env_clear()
        .envs(scrubbed_env)
        .output()
        .map_err(|err| CommandFailureReason::SpawnFailed {
            message: err.to_string(),
        })?;
    let duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    let (exit_code, signal_terminated) = decode_exit_status(&output.status);

    let mut captures: IndexMap<String, Option<serde_json::Value>> = IndexMap::new();
    let mut optional_unset: Vec<String> = Vec::new();
    for (name, capture_spec) in &spec.capture {
        match extract_capture(name, capture_spec, &stdout, exit_code) {
            CaptureOutcome::Value(value) => {
                captures.insert(name.clone(), Some(value));
            }
            CaptureOutcome::OptionalUnset => {
                captures.insert(name.clone(), None);
                optional_unset.push(name.clone());
            }
            CaptureOutcome::Missed { pattern } => {
                return Err(CommandFailureReason::CaptureMissed {
                    name: name.clone(),
                    pattern,
                });
            }
            CaptureOutcome::InvalidRegex { error } => {
                return Err(CommandFailureReason::InvalidRegex {
                    name: name.clone(),
                    error,
                });
            }
        }
    }

    if signal_terminated {
        return Err(CommandFailureReason::SignalTerminated);
    }
    if let Some(code) = exit_code {
        if code != 0 {
            return Err(CommandFailureReason::NonZeroExit { code });
        }
    }

    Ok(CommandRunResult {
        exit_code,
        signal_terminated,
        stdout,
        stderr,
        interpolated_run,
        captures,
        optional_unset,
        duration_ms,
    })
}

/// Abstraction over the parent process environment so tests can swap
/// in a deterministic map without poking at the global env.
pub trait ParentEnv {
    fn lookup(&self, name: &str) -> Option<String>;
}

/// Default implementation reading from `std::env::var`.
pub struct ProcessEnv;

impl ParentEnv for ProcessEnv {
    fn lookup(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

#[cfg(unix)]
fn shell_invocation() -> (&'static str, &'static str) {
    // Absolute path so the spawn does not depend on PATH being
    // present in the scrubbed child env (which `env_clear()` removes
    // before our `envs()` writes the baseline back).
    ("/bin/sh", "-c")
}

#[cfg(windows)]
fn shell_invocation() -> (&'static str, &'static str) {
    ("cmd", "/C")
}

/// Build the child's environment: a tiny baseline (`PATH`, `HOME`,
/// `TMPDIR`/`TEMP`/`TMP`) plus everything the user explicitly listed
/// in `pass_env:`. Names not present in the parent process are
/// silently skipped so the test author can list "preferred but
/// optional" knobs without forcing the runner to fail when they are
/// absent.
fn build_child_env(allowlist: &[String], parent: &dyn ParentEnv) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let baseline = baseline_env_keys();
    let mut visited = std::collections::BTreeSet::new();
    for key in baseline.iter().copied() {
        if visited.insert(key.to_string()) {
            if let Some(value) = parent.lookup(key) {
                out.push((key.to_string(), value));
            }
        }
    }
    for raw in allowlist {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !visited.insert(trimmed.to_string()) {
            continue;
        }
        if let Some(value) = parent.lookup(trimmed) {
            out.push((trimmed.to_string(), value));
        }
    }
    out
}

#[cfg(unix)]
fn baseline_env_keys() -> &'static [&'static str] {
    &["PATH", "HOME", "TMPDIR"]
}

#[cfg(windows)]
fn baseline_env_keys() -> &'static [&'static str] {
    &[
        "PATH",
        "PATHEXT",
        "USERPROFILE",
        "SYSTEMROOT",
        "TMP",
        "TEMP",
        "COMSPEC",
    ]
}

#[cfg(unix)]
fn decode_exit_status(status: &std::process::ExitStatus) -> (Option<i32>, bool) {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = status.code() {
        (Some(code), false)
    } else {
        // Killed by a signal: report `signal_terminated: true`.
        let _ = status.signal();
        (None, true)
    }
}

#[cfg(windows)]
fn decode_exit_status(status: &std::process::ExitStatus) -> (Option<i32>, bool) {
    (status.code(), false)
}

#[derive(Debug)]
enum CaptureOutcome {
    Value(serde_json::Value),
    OptionalUnset,
    Missed { pattern: String },
    InvalidRegex { error: String },
}

fn extract_capture(
    _name: &str,
    spec: &CommandCaptureSpec,
    stdout: &str,
    exit_code: Option<i32>,
) -> CaptureOutcome {
    if spec.exit_code {
        return match exit_code {
            Some(code) => CaptureOutcome::Value(serde_json::Value::from(code)),
            None => {
                if spec.optional {
                    CaptureOutcome::OptionalUnset
                } else {
                    CaptureOutcome::Missed {
                        pattern: "exit_code".to_string(),
                    }
                }
            }
        };
    }
    if let Some(pattern) = spec.stdout_regex.as_ref() {
        let regex = match regex_cache::get(pattern) {
            Ok(re) => re,
            Err(err) => {
                return CaptureOutcome::InvalidRegex {
                    error: err.to_string(),
                };
            }
        };
        match regex.captures(stdout) {
            Some(caps) => {
                let value = caps
                    .get(1)
                    .or_else(|| caps.get(0))
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                CaptureOutcome::Value(serde_json::Value::String(value))
            }
            None => {
                if spec.optional {
                    CaptureOutcome::OptionalUnset
                } else {
                    CaptureOutcome::Missed {
                        pattern: pattern.clone(),
                    }
                }
            }
        }
    } else {
        // Parser already enforces exactly one source; this branch is
        // unreachable in practice but keeps the function total.
        CaptureOutcome::Missed {
            pattern: "<no source>".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CommandCaptureSpec, CommandStep};
    use std::collections::HashMap;

    struct FakeEnv(HashMap<String, String>);
    impl ParentEnv for FakeEnv {
        fn lookup(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    fn ctx_empty() -> Context {
        Context::default()
    }

    #[test]
    fn child_env_is_minimal_by_default() {
        let parent = FakeEnv(
            [
                ("PATH".to_string(), "/usr/bin".to_string()),
                ("HOME".to_string(), "/root".to_string()),
                ("API_KEY".to_string(), "secret".to_string()),
                ("TMPDIR".to_string(), "/tmp".to_string()),
            ]
            .into_iter()
            .collect(),
        );
        let env = build_child_env(&[], &parent);
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"PATH"));
        assert!(!keys.contains(&"API_KEY"), "unlisted vars must not leak");
    }

    #[test]
    fn pass_env_allowlist_forwards_named_variables() {
        let parent = FakeEnv(
            [
                ("PATH".to_string(), "/usr/bin".to_string()),
                ("API_KEY".to_string(), "secret".to_string()),
                ("OTHER".to_string(), "x".to_string()),
            ]
            .into_iter()
            .collect(),
        );
        let env = build_child_env(&["API_KEY".into()], &parent);
        let api_key = env.iter().find(|(k, _)| k == "API_KEY");
        assert!(api_key.is_some());
        assert_eq!(api_key.unwrap().1, "secret");
        let other = env.iter().find(|(k, _)| k == "OTHER");
        assert!(other.is_none());
    }

    #[test]
    fn pass_env_unknown_variable_is_silently_skipped() {
        let parent = FakeEnv(HashMap::new());
        let env = build_child_env(&["DOES_NOT_EXIST".into()], &parent);
        assert!(env.iter().all(|(k, _)| k != "DOES_NOT_EXIST"));
    }

    #[test]
    #[cfg(unix)]
    fn run_echo_succeeds_and_captures_stdout() {
        let spec = CommandStep {
            run: "echo hello".into(),
            pass_env: Vec::new(),
            workdir: None,
            capture: {
                let mut c = IndexMap::new();
                c.insert(
                    "greeting".to_string(),
                    CommandCaptureSpec {
                        stdout_regex: Some("(hello)".into()),
                        exit_code: false,
                        optional: false,
                    },
                );
                c
            },
        };
        let result = run_command(&spec, &ctx_empty(), Path::new("."), &ProcessEnv).unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(
            result.captures.get("greeting").cloned().flatten(),
            Some(serde_json::Value::String("hello".into()))
        );
    }

    #[test]
    #[cfg(unix)]
    fn run_nonzero_exit_returns_command_failed() {
        let spec = CommandStep {
            run: "exit 7".into(),
            pass_env: Vec::new(),
            workdir: None,
            capture: IndexMap::new(),
        };
        let err = run_command(&spec, &ctx_empty(), Path::new("."), &ProcessEnv).unwrap_err();
        assert!(matches!(err, CommandFailureReason::NonZeroExit { code: 7 }));
    }

    #[test]
    #[cfg(unix)]
    fn run_capture_missed_returns_failure() {
        let spec = CommandStep {
            run: "echo nothing".into(),
            pass_env: Vec::new(),
            workdir: None,
            capture: {
                let mut c = IndexMap::new();
                c.insert(
                    "value".to_string(),
                    CommandCaptureSpec {
                        stdout_regex: Some("FOO=([0-9]+)".into()),
                        exit_code: false,
                        optional: false,
                    },
                );
                c
            },
        };
        let err = run_command(&spec, &ctx_empty(), Path::new("."), &ProcessEnv).unwrap_err();
        match err {
            CommandFailureReason::CaptureMissed { name, .. } => assert_eq!(name, "value"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    #[cfg(unix)]
    fn run_optional_capture_miss_is_not_failure() {
        let spec = CommandStep {
            run: "echo nothing".into(),
            pass_env: Vec::new(),
            workdir: None,
            capture: {
                let mut c = IndexMap::new();
                c.insert(
                    "value".to_string(),
                    CommandCaptureSpec {
                        stdout_regex: Some("FOO=([0-9]+)".into()),
                        exit_code: false,
                        optional: true,
                    },
                );
                c
            },
        };
        let result = run_command(&spec, &ctx_empty(), Path::new("."), &ProcessEnv).unwrap();
        assert_eq!(result.captures.get("value").cloned().flatten(), None);
        assert_eq!(result.optional_unset, vec!["value".to_string()]);
    }

    #[test]
    #[cfg(unix)]
    fn run_exit_code_capture_returns_integer_value() {
        let spec = CommandStep {
            run: "exit 0".into(),
            pass_env: Vec::new(),
            workdir: None,
            capture: {
                let mut c = IndexMap::new();
                c.insert(
                    "code".to_string(),
                    CommandCaptureSpec {
                        stdout_regex: None,
                        exit_code: true,
                        optional: false,
                    },
                );
                c
            },
        };
        let result = run_command(&spec, &ctx_empty(), Path::new("."), &ProcessEnv).unwrap();
        assert_eq!(
            result.captures.get("code").cloned().flatten(),
            Some(serde_json::Value::from(0_i64))
        );
    }
}
