use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One NDJSON event emitted by `tarn run --ndjson`. The schema mirrors
/// the upstream stream exactly; new event kinds added in tarn appear
/// here as `TarnEvent::Other` until the desktop app explicitly handles
/// them. This keeps forward-compat painless.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TarnEvent {
    FileStarted(FileStarted),
    StepFinished(StepFinished),
    TestFinished(TestFinished),
    FileFinished(FileFinished),
    Done(RunDone),

    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStarted {
    pub file: String,
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepFinished {
    pub file: String,
    pub test: Option<String>,
    pub step: String,
    pub step_index: u32,
    pub status: String,
    pub duration_ms: u64,
    pub phase: Option<String>,
    pub progress: Option<Progress>,
    #[serde(default)]
    pub assertion_failures: Vec<Value>,
    pub error_code: Option<String>,
    pub failure_category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub index: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFinished {
    pub file: String,
    pub test: String,
    pub status: String,
    pub duration_ms: u64,
    pub steps: StepCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepCounts {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileFinished {
    pub file: String,
    pub file_name: Option<String>,
    pub status: String,
    pub duration_ms: u64,
    pub summary: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDone {
    pub duration_ms: u64,
    pub summary: Value,
}
