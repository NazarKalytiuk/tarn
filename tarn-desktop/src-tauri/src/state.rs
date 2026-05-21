use crate::sidecar::RunHandle;
use dashmap::DashMap;
use std::sync::Arc;

/// Process-wide map of `run_id` → live run. Inserts on `run_tests`,
/// removes on `cancel_run` or when the streaming task notices the
/// child has exited.
#[derive(Clone, Default)]
pub struct AppState {
    pub runs: Arc<DashMap<String, RunHandle>>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }
}
