//! Basic integration test for go-on Rust SDK.
//!
//! These tests verify client construction and configuration.
//! They do not require a running backend — they only test that the
//! SDK types and builders are wired correctly.

use go_on_sdk::GoOnClientBuilder;

#[test]
fn test_client_builder_defaults() {
    let client = GoOnClientBuilder::new("http://localhost:8090")
        .build()
        .expect("building client with defaults should succeed");

    // Verify the client is constructible; base_url is private so we
    // just assert the builder doesn't panic or return an error.
    let _ = client;
}

#[test]
fn test_client_builder_custom_timeout() {
    let client = GoOnClientBuilder::new("http://localhost:8090")
        .with_timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("building client with custom timeout should succeed");

    let _ = client;
}

#[test]
fn test_client_builder_custom_retries() {
    let client = GoOnClientBuilder::new("http://localhost:8090")
        .with_max_retries(5)
        .with_retry_delay(std::time::Duration::from_secs(2))
        .build()
        .expect("building client with custom retries should succeed");

    let _ = client;
}

#[test]
fn test_builder_chain_all_options() {
    let client = GoOnClientBuilder::new("http://localhost:8090")
        .with_timeout(std::time::Duration::from_secs(15))
        .with_max_retries(10)
        .with_retry_delay(std::time::Duration::from_millis(500))
        .build()
        .expect("building client with all options should succeed");

    let _ = client;
}

#[test]
fn test_error_sdk_display() {
    use go_on_sdk::SdkError;

    let err = SdkError::Timeout { elapsed_secs: 30 };
    let msg = err.to_string();
    assert!(
        msg.contains("30"),
        "Timeout error should include elapsed seconds"
    );
}
