pub mod compact;
pub mod curl;
pub mod diff;
pub mod failure;
pub mod failures_command;
pub mod fixture_writer;
pub mod html;
pub mod human;
pub mod inspect;
pub mod json;
pub mod json_parse;
pub mod junit;
pub mod llm;
pub mod progress;
pub mod redaction;
pub mod rerun;
pub mod run_dir;
pub mod state_writer;
pub mod summary;
pub mod tap;

use crate::assert::types::RunResult;
use std::path::PathBuf;
use std::str::FromStr;

/// Options that tweak how test results are rendered.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOptions {
    /// Show only failed tests/steps in the output. Summary counts stay accurate.
    pub only_failed: bool,
    /// Verbose rendering: e.g. compact format shows captured values per test.
    pub verbose: bool,
    /// When true, skip ANSI color escapes in the output. Used by the
    /// llm format whenever stdout is not a TTY (pipes, files, CI logs).
    pub no_color: bool,
    /// When true, include request/response payloads for passing steps
    /// (in addition to failing steps). Mirrors the `--verbose-responses`
    /// CLI flag (NAZ-244). Step-level `debug: true` overrides this at
    /// the per-step level.
    pub verbose_responses: bool,
}

/// Output format for test results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Human,
    Json,
    Junit,
    Tap,
    Html,
    Curl,
    CurlAll,
    Compact,
    Llm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputTarget {
    pub format: OutputFormat,
    pub path: Option<PathBuf>,
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
            "curl" => Ok(OutputFormat::Curl),
            "curl-all" => Ok(OutputFormat::CurlAll),
            "compact" => Ok(OutputFormat::Compact),
            "llm" => Ok(OutputFormat::Llm),
            other => Err(format!("Unknown output format: '{}'", other)),
        }
    }
}

impl FromStr for OutputTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (format_raw, path) = match s.split_once('=') {
            Some((format, path)) => (format, Some(PathBuf::from(path))),
            None => (s, None),
        };

        let format = format_raw.parse::<OutputFormat>()?;
        Ok(Self { format, path })
    }
}

impl OutputTarget {
    pub fn writes_to_stdout(&self) -> bool {
        self.path.is_none() && self.format != OutputFormat::Html
    }
}

/// Render test results in the specified format.
pub fn render(result: &RunResult, format: OutputFormat) -> String {
    render_with_options(result, format, RenderOptions::default())
}

/// Render test results in the specified format with rendering options.
pub fn render_with_options(
    result: &RunResult,
    format: OutputFormat,
    opts: RenderOptions,
) -> String {
    match format {
        OutputFormat::Human => human::render_with_options(result, opts),
        OutputFormat::Json => {
            json::render_with_options(result, json::JsonOutputMode::Verbose, opts)
        }
        OutputFormat::Junit => junit::render(result),
        OutputFormat::Tap => tap::render(result),
        OutputFormat::Html => html::render(result),
        OutputFormat::Curl => curl::render_failures(result),
        OutputFormat::CurlAll => curl::render_all(result),
        OutputFormat::Compact => compact::render_with_options(result, opts),
        OutputFormat::Llm => llm::render_with_options(result, opts),
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
        assert_eq!("curl".parse::<OutputFormat>(), Ok(OutputFormat::Curl));
        assert_eq!(
            "curl-all".parse::<OutputFormat>(),
            Ok(OutputFormat::CurlAll)
        );
        assert_eq!("JSON".parse::<OutputFormat>(), Ok(OutputFormat::Json));
        assert_eq!("HTML".parse::<OutputFormat>(), Ok(OutputFormat::Html));
        assert_eq!("compact".parse::<OutputFormat>(), Ok(OutputFormat::Compact));
        assert_eq!("llm".parse::<OutputFormat>(), Ok(OutputFormat::Llm));
        assert_eq!("LLM".parse::<OutputFormat>(), Ok(OutputFormat::Llm));
        assert!("unknown".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn output_target_from_format_only() {
        assert_eq!(
            "json".parse::<OutputTarget>(),
            Ok(OutputTarget {
                format: OutputFormat::Json,
                path: None,
            })
        );
    }

    #[test]
    fn output_target_from_format_and_path() {
        assert_eq!(
            "junit=reports/junit.xml".parse::<OutputTarget>(),
            Ok(OutputTarget {
                format: OutputFormat::Junit,
                path: Some(PathBuf::from("reports/junit.xml")),
            })
        );
    }

    #[test]
    fn html_without_path_does_not_write_to_stdout() {
        let target = "html".parse::<OutputTarget>().unwrap();
        assert!(!target.writes_to_stdout());
    }
}
