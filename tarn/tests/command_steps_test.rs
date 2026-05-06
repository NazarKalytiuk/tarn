//! End-to-end tests for `command:` shell-step support (NAZ-464).
//!
//! These tests exercise the CLI surface — parser validation, the
//! `--allow-exec` gate, env-scrubbing semantics, `pass_env` allowlist,
//! capture from stdout regex / exit code, and the `command_failed`
//! failure path. They run a real `tarn run` subprocess so the
//! interaction between the CLI flag, the `tarn.config.yaml` field, and
//! the runner stays end-to-end coherent.
//!
//! All tests are gated to Unix because they exercise `sh -c` directly.
//! Windows uses `cmd /C` and has its own quoting/escape semantics; the
//! cross-platform contract is covered by the executor's unit tests in
//! `src/command.rs`. Adding a Windows e2e harness requires a separate
//! fixture set and is tracked outside this PR.

#![cfg(unix)]

use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;

fn tarn() -> Command {
    Command::cargo_bin("tarn").unwrap()
}

fn write_test(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    path
}

#[test]
fn validate_rejects_step_with_both_request_and_command() {
    let dir = TempDir::new().unwrap();
    let path = write_test(
        dir.path(),
        "bad.tarn.yaml",
        r#"
name: Bad step
steps:
  - name: Conflicting
    request:
      method: GET
      url: "http://example.com"
    command:
      run: "echo hi"
"#,
    );

    tarn()
        .arg("validate")
        .arg(&path)
        .assert()
        .failure()
        .stdout(predicates::str::contains(
            "either an HTTP request or a shell command",
        ));
}

#[test]
fn validate_rejects_step_with_neither_request_nor_command() {
    let dir = TempDir::new().unwrap();
    let path = write_test(
        dir.path(),
        "bare.tarn.yaml",
        r#"
name: Bare step
steps:
  - name: Empty
"#,
    );

    tarn()
        .arg("validate")
        .arg(&path)
        .assert()
        .failure()
        .stdout(predicates::str::contains("must define either"));
}

#[test]
fn validate_rejects_command_with_assert() {
    let dir = TempDir::new().unwrap();
    let path = write_test(
        dir.path(),
        "assert-on-command.tarn.yaml",
        r#"
name: Assert on command
steps:
  - name: With assert
    command:
      run: "echo hi"
    assert:
      status: 200
"#,
    );

    tarn()
        .arg("validate")
        .arg(&path)
        .assert()
        .failure()
        .stdout(predicates::str::contains("only valid on `request:` steps"));
}

#[test]
fn validate_rejects_command_with_both_capture_sources() {
    let dir = TempDir::new().unwrap();
    let path = write_test(
        dir.path(),
        "double-capture.tarn.yaml",
        r#"
name: Double capture source
steps:
  - name: Bad capture
    command:
      run: "echo hi"
      capture:
        v:
          stdout_regex: "(.+)"
          exit_code: true
"#,
    );

    tarn()
        .arg("validate")
        .arg(&path)
        .assert()
        .failure()
        .stdout(predicates::str::contains("pick one source per capture"));
}

#[test]
fn run_skips_command_step_without_allow_exec_and_stays_green() {
    let dir = TempDir::new().unwrap();
    let path = write_test(
        dir.path(),
        "skipped.tarn.yaml",
        r#"
name: Inert by default
steps:
  - name: would echo
    command:
      run: "echo this-should-be-skipped"
"#,
    );

    let assertion = tarn()
        .arg("run")
        .arg(&path)
        .arg("--format")
        .arg("json")
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("\"failure_category\": \"skipped_by_policy\""),
        "expected skipped_by_policy in JSON report, got: {stdout}"
    );
    // Step must still have passed: the skip is benign.
    assert!(
        !stdout.contains("\"this-should-be-skipped\""),
        "command must NOT execute without --allow-exec; got: {stdout}"
    );
}

#[test]
fn run_with_allow_exec_executes_command_and_captures_stdout() {
    let dir = TempDir::new().unwrap();
    let path = write_test(
        dir.path(),
        "exec.tarn.yaml",
        r#"
name: Capture stdout
tests:
  capture:
    steps:
      - name: emit token
        command:
          run: "echo token=ABC123"
          capture:
            extracted:
              stdout_regex: "token=([A-Z0-9]+)"
      - name: see token
        command:
          run: "echo got {{ capture.extracted }}"
          capture:
            mirror:
              stdout_regex: "got ([A-Z0-9]+)"
"#,
    );

    let assertion = tarn()
        .arg("run")
        .arg(&path)
        .arg("--allow-exec")
        .arg("--format")
        .arg("json")
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("\"extracted\": \"ABC123\""),
        "expected captured value to be threaded into the JSON report, got: {stdout}"
    );
    assert!(
        stdout.contains("\"mirror\": \"ABC123\""),
        "expected interpolation of capture into the next command, got: {stdout}"
    );
}

#[test]
fn run_with_allow_exec_marks_command_failed_on_nonzero_exit() {
    let dir = TempDir::new().unwrap();
    let path = write_test(
        dir.path(),
        "fail.tarn.yaml",
        r#"
name: Non-zero exit
steps:
  - name: explode
    command:
      run: "exit 42"
"#,
    );

    let assertion = tarn()
        .arg("run")
        .arg(&path)
        .arg("--allow-exec")
        .arg("--format")
        .arg("json")
        .assert()
        .failure()
        .code(1);

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("\"failure_category\": \"command_failed\""),
        "expected command_failed category, got: {stdout}"
    );
}

#[test]
fn pass_env_does_not_forward_unlisted_variables() {
    let dir = TempDir::new().unwrap();
    let path = write_test(
        dir.path(),
        "env.tarn.yaml",
        r#"
name: Env scrubbing
steps:
  - name: probe
    command:
      run: "sh -c 'echo SECRET=${TARN_E2E_SECRET:-MISSING}'"
      capture:
        leaked:
          stdout_regex: "SECRET=(\\S+)"
"#,
    );

    // Set a fake secret in the parent env. Without `pass_env`,
    // the child must observe the placeholder `MISSING`.
    let assertion = tarn()
        .arg("run")
        .arg(&path)
        .arg("--allow-exec")
        .arg("--format")
        .arg("json")
        .env("TARN_E2E_SECRET", "shhhh")
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("\"leaked\": \"MISSING\""),
        "child must NOT inherit unlisted env var, got: {stdout}"
    );
    assert!(
        !stdout.contains("\"shhhh\""),
        "secret leaked into child env: {stdout}"
    );
}

#[test]
fn pass_env_allowlist_forwards_named_variable() {
    let dir = TempDir::new().unwrap();
    let path = write_test(
        dir.path(),
        "env-allow.tarn.yaml",
        r#"
name: Env allowlist
steps:
  - name: probe
    command:
      run: "sh -c 'echo SECRET=${TARN_E2E_TOKEN:-MISSING}'"
      pass_env: ["TARN_E2E_TOKEN"]
      capture:
        leaked:
          stdout_regex: "SECRET=(\\S+)"
"#,
    );

    let assertion = tarn()
        .arg("run")
        .arg(&path)
        .arg("--allow-exec")
        .arg("--format")
        .arg("json")
        .env("TARN_E2E_TOKEN", "letmein")
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("\"leaked\": \"letmein\""),
        "expected explicitly-allowlisted env var to reach the child, got: {stdout}"
    );
}

#[test]
fn config_allow_exec_authorizes_command_steps() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("tarn.config.yaml"),
        "test_dir: .\nallow_exec: true\n",
    )
    .unwrap();
    write_test(
        dir.path(),
        "exec.tarn.yaml",
        r#"
name: Project-level opt-in
steps:
  - name: emit
    command:
      run: "echo HELLO"
      capture:
        msg:
          stdout_regex: "(HELLO)"
"#,
    );

    let assertion = tarn()
        .arg("run")
        .arg("exec.tarn.yaml")
        .arg("--format")
        .arg("json")
        .current_dir(dir.path())
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("\"msg\": \"HELLO\""),
        "expected allow_exec config to authorize the command, got: {stdout}"
    );
}

#[test]
fn list_marks_command_steps_with_distinct_marker() {
    let dir = TempDir::new().unwrap();
    let path = write_test(
        dir.path(),
        "mixed.tarn.yaml",
        r#"
name: Mixed
steps:
  - name: ping
    request:
      method: GET
      url: "http://example.com"
  - name: shell-prep
    command:
      run: "true"
"#,
    );

    let assertion = tarn()
        .arg("list")
        .arg("--file")
        .arg(&path)
        .arg("--format")
        .arg("json")
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("\"kind\": \"command\""),
        "command kind missing in JSON list output: {stdout}"
    );
    assert!(
        stdout.contains("\"kind\": \"request\""),
        "request kind missing in JSON list output: {stdout}"
    );
}
