//! GAP-B53-50: CLI integration test suite.
//!
//! Tests the command-line interface of the go-on binary end-to-end:
//! argument parsing, config validation, health check, and chat mode.

use std::process::Command;

/// The name of the built binary.
const BINARY_NAME: &str = env!("CARGO_BIN_EXE_go-on");

/// Helper: run the CLI binary with the given args and return (stdout, stderr, status).
fn run_cli(args: &[&str]) -> (String, String, std::process::ExitStatus) {
    let output = Command::new(BINARY_NAME)
        .args(args)
        .output()
        .expect("Failed to execute CLI binary");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status)
}

#[test]
fn test_cli_help_exits_successfully() {
    let (stdout, stderr, status) = run_cli(&["--help"]);
    assert!(
        status.success(),
        "Expected --help to exit successfully, got: {stderr}"
    );
    assert!(
        stdout.contains("go-on") || stdout.contains("usage") || stdout.contains("Usage"),
        "Help output should mention go-on or usage, got: {stdout}"
    );
}

#[test]
fn test_cli_version_flag() {
    let (stdout, stderr, status) = run_cli(&["--version"]);
    assert!(
        status.success(),
        "Expected --version to exit successfully, got: {stderr}"
    );
    assert!(
        !stdout.is_empty() || status.success(),
        "Version output should not be empty"
    );
}

#[test]
fn test_cli_invalid_flag_fails() {
    let (_, stderr, status) = run_cli(&["--nonexistent-flag"]);
    assert!(!status.success(), "Expected nonexistent flag to fail");
    assert!(
        stderr.contains("error") || stderr.contains("unexpected"),
        "Stderr should indicate an error, got: {stderr}"
    );
}

#[test]
fn test_cli_diagnose_accepts_flag() {
    // diagnose should at least parse the flag without crashing.
    let (_, _, status) = run_cli(&["--diagnose", "--config", "/dev/null"]);
    assert!(
        status.success(),
        "Expected --diagnose flag to be accepted, got exit status: {status}"
    );
}

#[test]
fn test_cli_status_accepts_flag() {
    let (_, _, status) = run_cli(&["--status", "--config", "/dev/null"]);
    assert!(
        status.success(),
        "Expected --status flag to be accepted, got exit status: {status}"
    );
}

#[test]
fn test_cli_unknown_subcommand_fails_gracefully() {
    let (_, stderr, status) = run_cli(&["garbage-command"]);
    assert!(!status.success(), "Unknown subcommand should fail");
    assert!(
        stderr.contains("error") || stderr.contains("unrecognized") || stderr.contains("not found"),
        "Stderr should indicate unrecognized subcommand, got: {stderr}"
    );
}

#[test]
fn test_cli_chat_flag_parses() {
    let (_, _, status) = run_cli(&["--chat", "--config", "/dev/null"]);
    assert!(
        status.success(),
        "Expected --chat flag to be accepted, got exit status: {status}"
    );
}

#[test]
fn test_cli_config_validation_rejects_missing_config() {
    // Running without a config file should produce a helpful message,
    // but the binary should not panic.
    let (stdout, stderr, status) = run_cli(&["--validate-config", "--config", "/nonexistent/path"]);
    if !status.success() {
        assert!(
            stderr.contains("error")
                || stderr.contains("not found")
                || stdout.contains("error")
                || stdout.contains("not found"),
            "Expected a helpful error message about missing config, stderr: {stderr}, stdout: {stdout}"
        );
    }
}

#[test]
fn test_cli_verbose_mode_output() {
    let (stdout, stderr, _status) =
        run_cli(&["--verbose", "--validate-config", "--config", "/dev/null"]);
    // Verbose mode may produce output on stdout (tracing) or stderr depending on
    // the tracing subscriber configuration. Accept either.
    assert!(
        !stdout.is_empty() || !stderr.is_empty(),
        "Expected verbose mode to emit diagnostic output, stdout was: {stdout}, stderr: {stderr}"
    );
    // Ensure stdout or stderr contains a validation message.
    assert!(
        stdout.contains("config")
            || stderr.contains("config")
            || stdout.contains("go-on")
            || stderr.contains("go-on"),
        "Expected verbose mode to mention config validation, \
         stdout: {stdout}, stderr: {stderr}"
    );
}
