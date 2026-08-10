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
    assert!(!stdout.is_empty(), "Version output should not be empty");
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
fn test_cli_config_validation_bootstraps_missing_config() {
    // The CLI does not reject a missing config file: it writes non-AI
    // bootstrap defaults to the path and validates them (see
    // `defaults::ensure_bootstrap_config`). Assert that real behavior
    // unconditionally — no conditional assertion that can vacate itself.
    let missing =
        std::env::temp_dir().join(format!("go-on-missing-config-{}.toml", std::process::id()));
    let _ = std::fs::remove_file(&missing);
    let missing_str = missing.to_str().expect("temp path is valid UTF-8");
    let (stdout, stderr, status) = run_cli(&["--validate-config", "--config", missing_str]);

    assert!(
        status.success(),
        "missing config must be bootstrapped and validate cleanly, stderr: {stderr}"
    );
    assert!(
        stdout.contains("Valid: true"),
        "validation report must declare the bootstrapped config valid, stdout: {stdout}"
    );
    assert!(
        missing.exists(),
        "the missing config path must be populated with bootstrap defaults"
    );
    let _ = std::fs::remove_file(&missing);
}

#[test]
fn test_cli_help_lists_known_flags() {
    // Regression guard: the CLI flag surface must stay explicit. Previously a
    // `--verbose` test exercised a flag that never existed — the assertion
    // passed only because clap's unknown-argument error happened to satisfy
    // "output non-empty". Verify real flags instead.
    let (stdout, stderr, status) = run_cli(&["--help"]);
    let output = format!("{stdout}{stderr}");
    assert!(status.success());
    for flag in ["--config", "--validate-config", "--setup", "--diagnose"] {
        assert!(output.contains(flag), "--help must list {flag}");
    }
    assert!(
        !output.contains("--verbose"),
        "--help must not advertise a non-existent --verbose flag"
    );
}
