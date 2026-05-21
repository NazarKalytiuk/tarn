use crate::error::{AppError, AppResult};
use crate::sidecar::binary;
use crate::sidecar::event::TarnEvent;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;

/// Handle returned to callers of `run_tests`. Only carries the cancel
/// channel — the child process and report-reading lifecycle are owned
/// by the background streaming task that this call spawned.
pub struct RunHandle {
    pub cancel: Option<oneshot::Sender<()>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunStarted<'a> {
    pub run_id: &'a str,
    pub project: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub struct WrappedEvent<'a> {
    pub run_id: &'a str,
    #[serde(flatten)]
    pub event: TarnEvent,
}

#[derive(Debug, Clone, Serialize)]
pub struct SidecarLog<'a> {
    pub run_id: &'a str,
    pub level: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunReportReady<'a> {
    pub run_id: &'a str,
    pub report: serde_json::Value,
}

/// Run `tarn list --format json` in `project_dir` and return the raw
/// stdout. Schema mirrors `tarn list --format json` 1:1.
pub async fn list_tests(project_dir: &Path) -> AppResult<String> {
    let binary_path = binary::resolve_tarn()?;
    let output = Command::new(&binary_path)
        .args(["list", "--format", "json"])
        .current_dir(project_dir)
        .output()
        .await
        .map_err(|source| AppError::Spawn {
            binary: binary_path.display().to_string(),
            source,
        })?;

    if !output.status.success() {
        return Err(AppError::Exit {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Read the always-on `.tarn/last-run.json` artifact for `project_dir`.
/// Returns `Ok(None)` when the file does not exist yet (no run has
/// produced an artifact, or `--no-last-run-json` was passed).
pub async fn read_last_run_report(project_dir: &Path) -> AppResult<Option<serde_json::Value>> {
    let path = project_dir.join(".tarn").join("last-run.json");
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(AppError::Io(err)),
    }
}

/// Spawn `tarn run --ndjson --verbose-responses [selectors...]`. The
/// child process and its lifecycle (streaming → child wait → read
/// `.tarn/last-run.json`) are owned by a background tokio task.
///
/// `--verbose-responses` is required so the JSON artifact contains
/// full request/response bodies for every step. The NDJSON stream
/// itself does not carry payload bodies — see ADR 0002 section 7.
pub async fn run_tests(
    app: AppHandle,
    project_dir: &Path,
    run_id: String,
    selectors: Vec<String>,
    env_name: Option<String>,
    tag: Option<String>,
    vars: Vec<(String, String)>,
) -> AppResult<RunHandle> {
    let binary_path = binary::resolve_tarn()?;

    let mut cmd = Command::new(&binary_path);
    cmd.arg("run").arg("--ndjson").arg("--verbose-responses");
    for selector in &selectors {
        cmd.arg("--select").arg(selector);
    }
    if let Some(env) = env_name.as_deref() {
        cmd.arg("--env").arg(env);
    }
    if let Some(tag) = tag.as_deref() {
        cmd.arg("--tag").arg(tag);
    }
    for (k, v) in &vars {
        cmd.arg("--var").arg(format!("{k}={v}"));
    }
    cmd.current_dir(project_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|source| AppError::Spawn {
        binary: binary_path.display().to_string(),
        source,
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Other("tarn child has no stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::Other("tarn child has no stderr".into()))?;

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let app_for_stream = app.clone();
    let run_id_for_stream = run_id.clone();
    let project_for_stream = project_dir.to_path_buf();

    tokio::spawn(stream_lifecycle(
        app_for_stream,
        run_id_for_stream,
        project_for_stream,
        child,
        stdout,
        stderr,
        cancel_rx,
    ));

    let _ = app.emit(
        "test:run-started",
        RunStarted {
            run_id: &run_id,
            project: &project_dir.display().to_string(),
        },
    );

    Ok(RunHandle {
        cancel: Some(cancel_tx),
    })
}

#[allow(clippy::too_many_arguments)]
async fn stream_lifecycle(
    app: AppHandle,
    run_id: String,
    project_dir: PathBuf,
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    mut cancel: oneshot::Receiver<()>,
) {
    let mut reader = BufReader::new(stdout).lines();
    let mut err_reader = BufReader::new(stderr).lines();
    let mut cancelled = false;

    loop {
        tokio::select! {
            biased;

            _ = &mut cancel => {
                cancelled = true;
                let _ = child.start_kill();
                break;
            }

            line = reader.next_line() => {
                match line {
                    Ok(Some(text)) => {
                        if text.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<TarnEvent>(&text) {
                            Ok(event) => {
                                let topic = match &event {
                                    TarnEvent::FileStarted(_) => "test:file-started",
                                    TarnEvent::StepFinished(_) => "test:step-finished",
                                    TarnEvent::TestFinished(_) => "test:test-finished",
                                    TarnEvent::FileFinished(_) => "test:file-finished",
                                    TarnEvent::Done(_) => "test:run-done",
                                    TarnEvent::Other => "test:other",
                                };
                                let _ = app.emit(topic, WrappedEvent { run_id: &run_id, event });
                            }
                            Err(err) => {
                                let _ = app.emit(
                                    "sidecar:log",
                                    SidecarLog {
                                        run_id: &run_id,
                                        level: "warn",
                                        message: format!("ndjson parse error: {err} — line: {text}"),
                                    },
                                );
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        let _ = app.emit(
                            "sidecar:log",
                            SidecarLog {
                                run_id: &run_id,
                                level: "error",
                                message: format!("stdout read error: {err}"),
                            },
                        );
                        break;
                    }
                }
            }

            err_line = err_reader.next_line() => {
                if let Ok(Some(text)) = err_line {
                    if !text.is_empty() {
                        let _ = app.emit(
                            "sidecar:log",
                            SidecarLog {
                                run_id: &run_id,
                                level: "info",
                                message: text,
                            },
                        );
                    }
                }
            }
        }
    }

    // Drain any remaining stderr lines until the child fully exits, so
    // panic backtraces or shutdown logs don't get silently dropped.
    let _ = child.wait().await;
    while let Ok(Some(text)) = err_reader.next_line().await {
        if text.is_empty() {
            continue;
        }
        let _ = app.emit(
            "sidecar:log",
            SidecarLog {
                run_id: &run_id,
                level: "info",
                message: text,
            },
        );
    }

    if cancelled {
        return;
    }

    match read_last_run_report(&project_dir).await {
        Ok(Some(report)) => {
            let _ = app.emit(
                "test:run-report-ready",
                RunReportReady {
                    run_id: &run_id,
                    report,
                },
            );
        }
        Ok(None) => {
            let _ = app.emit(
                "sidecar:log",
                SidecarLog {
                    run_id: &run_id,
                    level: "warn",
                    message: ".tarn/last-run.json not found after run finished".into(),
                },
            );
        }
        Err(err) => {
            let _ = app.emit(
                "sidecar:log",
                SidecarLog {
                    run_id: &run_id,
                    level: "error",
                    message: format!("failed to read last-run.json: {err}"),
                },
            );
        }
    }
}
