pub mod html;
pub mod human;
pub mod json;
pub mod junit;
pub mod tap;

use crate::assert::types::RunResult;
use std::str::FromStr;

/// Output format for test results.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    Human,
    Json,
    Junit,
    Tap,
    Html,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "human" => Ok(OutputFormat::Human),
            "json" => Ok(OutputFormat::Json),
            "junit" => Ok(OutputFormat::Junit),
            "tap" => Ok(OutputFormat::Tap),
            "html" => Ok(OutputFormat::Html),
            other => Err(format!("Unknown output format: '{}'", other)),
        }
    }
}

/// Render test results in the specified format.
pub fn render(result: &RunResult, format: OutputFormat) -> String {
    match format {
        OutputFormat::Human => human::render(result),
        OutputFormat::Json => json::render(result),
        OutputFormat::Junit => junit::render(result),
        OutputFormat::Tap => tap::render(result),
        OutputFormat::Html => html::render(result),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_format_from_str() {
        assert_eq!("human".parse::<OutputFormat>(), Ok(OutputFormat::Human));
        assert_eq!("json".parse::<OutputFormat>(), Ok(OutputFormat::Json));
        assert_eq!("junit".parse::<OutputFormat>(), Ok(OutputFormat::Junit));
        assert_eq!("tap".parse::<OutputFormat>(), Ok(OutputFormat::Tap));
        assert_eq!("html".parse::<OutputFormat>(), Ok(OutputFormat::Html));
        assert_eq!("JSON".parse::<OutputFormat>(), Ok(OutputFormat::Json));
        assert_eq!("HTML".parse::<OutputFormat>(), Ok(OutputFormat::Html));
        assert!("unknown".parse::<OutputFormat>().is_err());
    }
}
