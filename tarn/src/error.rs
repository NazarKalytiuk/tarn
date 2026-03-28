use thiserror::Error;

#[derive(Error, Debug)]
pub enum TarnError {
    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Interpolation error: {0}")]
    Interpolation(String),

    #[error("Capture error: {0}")]
    Capture(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Script error: {0}")]
    Script(String),
}

impl TarnError {
    /// Map error to CLI exit code per spec:
    /// 2 = configuration/parse error
    /// 3 = runtime error (network, timeout)
    pub fn exit_code(&self) -> i32 {
        match self {
            TarnError::Parse(_) => 2,
            TarnError::Config(_) => 2,
            TarnError::Validation(_) => 2,
            TarnError::Http(_) => 3,
            TarnError::Io(_) => 3,
            TarnError::Interpolation(_) => 2,
            TarnError::Capture(_) => 3,
            TarnError::Script(_) => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_exit_code_is_2() {
        let err = TarnError::Parse("bad yaml".into());
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn config_error_exit_code_is_2() {
        let err = TarnError::Config("missing field".into());
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn validation_error_exit_code_is_2() {
        let err = TarnError::Validation("invalid schema".into());
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn http_error_exit_code_is_3() {
        let err = TarnError::Http("connection refused".into());
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn io_error_exit_code_is_3() {
        let err = TarnError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn interpolation_error_exit_code_is_2() {
        let err = TarnError::Interpolation("unknown var".into());
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn capture_error_exit_code_is_3() {
        let err = TarnError::Capture("jsonpath failed".into());
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn error_display_messages() {
        assert_eq!(
            TarnError::Parse("bad".into()).to_string(),
            "Parse error: bad"
        );
        assert_eq!(
            TarnError::Http("timeout".into()).to_string(),
            "HTTP error: timeout"
        );
    }
}
