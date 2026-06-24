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

    assert_eq!(
        client.base_url(),
        "http://localhost:8090",
        "base_url should be set to the value passed to new()"
    );
    assert_eq!(client.max_retries(), 3, "default max_retries should be 3");
}

#[test]
fn test_client_builder_custom_timeout() {
    let client = GoOnClientBuilder::new("http://localhost:8090")
        .with_timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("building client with custom timeout should succeed");

    assert_eq!(
        client.timeout(),
        Some(std::time::Duration::from_secs(60)),
        "custom timeout should be applied"
    );
}

#[test]
fn test_client_builder_custom_retries() {
    let client = GoOnClientBuilder::new("http://localhost:8090")
        .with_max_retries(5)
        .with_retry_delay(std::time::Duration::from_secs(2))
        .build()
        .expect("building client with custom retries should succeed");

    assert_eq!(
        client.max_retries(),
        5,
        "custom max_retries should be applied"
    );
    assert_eq!(
        client.retry_delay(),
        std::time::Duration::from_secs(2),
        "custom retry_delay should be applied"
    );
}

#[test]
fn test_builder_chain_all_options() {
    let client = GoOnClientBuilder::new("http://localhost:8090")
        .with_timeout(std::time::Duration::from_secs(15))
        .with_max_retries(10)
        .with_retry_delay(std::time::Duration::from_millis(500))
        .build()
        .expect("building client with all options should succeed");

    assert_eq!(
        client.base_url(),
        "http://localhost:8090",
        "base_url should be preserved when chaining options"
    );
    assert_eq!(
        client.timeout(),
        Some(std::time::Duration::from_secs(15)),
        "timeout should be set when chaining options"
    );
    assert_eq!(
        client.max_retries(),
        10,
        "max_retries should be set when chaining options"
    );
    assert_eq!(
        client.retry_delay(),
        std::time::Duration::from_millis(500),
        "retry_delay should be set when chaining options"
    );
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
