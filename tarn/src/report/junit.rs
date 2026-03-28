use crate::assert::types::{FileResult, RunResult, StepResult};

/// Render test results as JUnit XML.
pub fn render(result: &RunResult) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");

    let total = result.total_steps();
    let failures = result.failed_steps();
    let time_secs = result.duration_ms as f64 / 1000.0;

    xml.push_str(&format!(
        "<testsuites tests=\"{}\" failures=\"{}\" time=\"{:.3}\">\n",
        total, failures, time_secs
    ));

    for file in &result.file_results {
        render_file(&mut xml, file);
    }

    xml.push_str("</testsuites>\n");
    xml
}

fn render_file(xml: &mut String, file: &FileResult) {
    let total = file.total_steps();
    let failures = file.failed_steps();
    let time_secs = file.duration_ms as f64 / 1000.0;

    xml.push_str(&format!(
        "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" time=\"{:.3}\">\n",
        escape_xml(&file.name),
        total,
        failures,
        time_secs
    ));

    // Setup steps
    for step in &file.setup_results {
        render_test_case(xml, "setup", step);
    }

    // Test steps
    for test in &file.test_results {
        for step in &test.step_results {
            render_test_case(xml, &test.name, step);
        }
    }

    // Teardown steps
    for step in &file.teardown_results {
        render_test_case(xml, "teardown", step);
    }

    xml.push_str("  </testsuite>\n");
}

fn render_test_case(xml: &mut String, classname: &str, step: &StepResult) {
    let time_secs = step.duration_ms as f64 / 1000.0;

    if step.passed {
        xml.push_str(&format!(
            "    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\" />\n",
            escape_xml(classname),
            escape_xml(&step.name),
            time_secs
        ));
    } else {
        xml.push_str(&format!(
            "    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\">\n",
            escape_xml(classname),
            escape_xml(&step.name),
            time_secs
        ));

        for failure in step.failures() {
            xml.push_str(&format!(
                "      <failure message=\"{}\" type=\"AssertionFailure\">{}</failure>\n",
                escape_xml(&failure.message),
                escape_xml(&format!(
                    "Expected: {}\nActual: {}",
                    failure.expected, failure.actual
                ))
            ));
        }

        xml.push_str("    </testcase>\n");
    }
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::*;

    fn make_result(passed: bool) -> RunResult {
        RunResult {
            duration_ms: 500,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Test Suite".into(),
                passed,
                duration_ms: 500,
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "my_test".into(),
                    description: None,
                    passed,
                    duration_ms: 500,
                    step_results: vec![StepResult {
                        name: "Check status".into(),
                        passed,
                        duration_ms: 250,
                        assertion_results: if passed {
                            vec![AssertionResult::pass("status", "200", "200")]
                        } else {
                            vec![AssertionResult::fail(
                                "status",
                                "200",
                                "404",
                                "Expected 200, got 404",
                            )]
                        },
                        request_info: None,
                        response_info: None,
                        error_category: None,
                    }],
                }],
                teardown_results: vec![],
            }],
        }
    }

    #[test]
    fn junit_starts_with_xml_header() {
        let output = render(&make_result(true));
        assert!(output.starts_with("<?xml version=\"1.0\""));
    }

    #[test]
    fn junit_has_testsuites_root() {
        let output = render(&make_result(true));
        assert!(output.contains("<testsuites"));
        assert!(output.contains("</testsuites>"));
    }

    #[test]
    fn junit_has_testsuite_for_file() {
        let output = render(&make_result(true));
        assert!(output.contains("<testsuite name=\"Test Suite\""));
    }

    #[test]
    fn junit_passing_test_self_closing() {
        let output = render(&make_result(true));
        assert!(output.contains("testcase classname=\"my_test\" name=\"Check status\""));
        assert!(output.contains("/>"));
    }

    #[test]
    fn junit_failing_test_has_failure_element() {
        let output = render(&make_result(false));
        assert!(output.contains("<failure"));
        assert!(output.contains("Expected 200, got 404"));
    }

    #[test]
    fn junit_test_counts() {
        let output = render(&make_result(false));
        assert!(output.contains("tests=\"1\""));
        assert!(output.contains("failures=\"1\""));
    }

    #[test]
    fn junit_time_format() {
        let output = render(&make_result(true));
        assert!(output.contains("time=\"0.250\"") || output.contains("time=\"0.500\""));
    }

    #[test]
    fn escape_xml_special_chars() {
        assert_eq!(
            escape_xml("a<b>c&d\"e'f"),
            "a&lt;b&gt;c&amp;d&quot;e&apos;f"
        );
    }

    #[test]
    fn junit_with_setup_and_teardown() {
        let result = RunResult {
            duration_ms: 300,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Suite".into(),
                passed: true,
                duration_ms: 300,
                setup_results: vec![StepResult {
                    name: "Auth".into(),
                    passed: true,
                    duration_ms: 50,
                    assertion_results: vec![],
                    request_info: None,
                    response_info: None,
                    error_category: None,
                }],
                test_results: vec![],
                teardown_results: vec![StepResult {
                    name: "Cleanup".into(),
                    passed: true,
                    duration_ms: 30,
                    assertion_results: vec![],
                    request_info: None,
                    response_info: None,
                    error_category: None,
                }],
            }],
        };
        let output = render(&result);
        assert!(output.contains("classname=\"setup\""));
        assert!(output.contains("classname=\"teardown\""));
    }
}
