use crate::error::{AppError, AppResult};
use crate::sidecar;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use tauri::{AppHandle, State};
use uuid::Uuid;

/// Result of `discover_tests` — the raw `tarn list --format json`
/// payload, parsed once on the backend and forwarded as a typed value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverResult {
    pub files: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunSelector {
    /// `FILE[::TEST[::STEP]]` selectors, repeated; mirrors `tarn run --select`.
    #[serde(default)]
    pub selectors: Vec<String>,
    pub env: Option<String>,
    pub tag: Option<String>,
    #[serde(default)]
    pub vars: Vec<KeyValue>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunStartedResponse {
    pub run_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OkResponse {
    pub ok: bool,
}

/// Available environment, as exposed in the toolbar Env picker.
/// `name` is the suffix after `tarn.env.` in the filename — e.g.
/// `tarn.env.staging.yaml` → `"staging"`. The base file `tarn.env.yaml`
/// is represented separately as `is_base = true` with `name = "default"`.
#[derive(Debug, Clone, Serialize)]
pub struct Environment {
    pub name: String,
    pub file: String,
    pub is_base: bool,
}

#[tauri::command]
pub async fn discover_tests(project: String) -> AppResult<DiscoverResult> {
    let dir = PathBuf::from(&project);
    let raw = sidecar::list_tests(&dir).await?;
    let value: DiscoverResult = serde_json::from_str(&raw)?;
    Ok(value)
}

#[tauri::command]
pub async fn run_tests(
    app: AppHandle,
    state: State<'_, AppState>,
    project: String,
    selector: RunSelector,
) -> AppResult<RunStartedResponse> {
    let dir = PathBuf::from(&project);
    let run_id = Uuid::new_v4().to_string();
    let vars: Vec<(String, String)> = selector
        .vars
        .into_iter()
        .map(|kv| (kv.key, kv.value))
        .collect();

    let handle = sidecar::run_tests(
        app,
        &dir,
        run_id.clone(),
        selector.selectors,
        selector.env,
        selector.tag,
        vars,
    )
    .await?;

    state.runs.insert(run_id.clone(), handle);

    Ok(RunStartedResponse { run_id })
}

#[tauri::command]
pub async fn cancel_run(state: State<'_, AppState>, run_id: String) -> AppResult<OkResponse> {
    let (_, mut handle) = state
        .runs
        .remove(&run_id)
        .ok_or_else(|| AppError::RunNotFound(run_id.clone()))?;

    if let Some(cancel) = handle.cancel.take() {
        let _ = cancel.send(());
    }

    Ok(OkResponse { ok: true })
}

#[tauri::command]
pub async fn get_run_report(project: String) -> AppResult<Option<Value>> {
    let dir = PathBuf::from(&project);
    sidecar::read_last_run_report(&dir).await
}

#[tauri::command]
pub async fn list_environments(project: String) -> AppResult<Vec<Environment>> {
    let dir = PathBuf::from(&project);
    let mut entries = tokio::fs::read_dir(&dir).await.map_err(AppError::Io)?;
    let mut out = Vec::new();

    while let Some(entry) = entries.next_entry().await.map_err(AppError::Io)? {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };

        // `tarn.env.local.yaml` is intentionally excluded from the
        // picker — it is a developer's per-machine override, not a
        // named environment. Tarn's env resolution already overlays it
        // on top of any chosen env, so surfacing it would mislead.
        if file_name == "tarn.env.local.yaml" {
            continue;
        }

        if file_name == "tarn.env.yaml" {
            out.push(Environment {
                name: "default".into(),
                file: file_name.into(),
                is_base: true,
            });
            continue;
        }

        if let Some(rest) = file_name
            .strip_prefix("tarn.env.")
            .and_then(|r| r.strip_suffix(".yaml"))
        {
            if !rest.is_empty() {
                out.push(Environment {
                    name: rest.into(),
                    file: file_name.into(),
                    is_base: false,
                });
            }
        }
    }

    out.sort_by(|a, b| {
        // Base env first, then named envs alphabetically.
        match (a.is_base, b.is_base) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        }
    });

    Ok(out)
}
