use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error(
        "tarn binary not found (checked TARN_BIN env, PATH, workspace target/{{debug,release}})"
    )]
    BinaryNotFound,

    #[error("failed to spawn `{binary}`: {source}")]
    Spawn {
        binary: String,
        #[source]
        source: std::io::Error,
    },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("run `{0}` is not active")]
    RunNotFound(String),

    #[error("tarn exited with status {code:?}: {stderr}")]
    Exit { code: Option<i32>, stderr: String },

    #[error("{0}")]
    Other(String),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
