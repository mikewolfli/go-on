//! Shared HTTP client singleton for connection-pool reuse across the process.
//!
//! GAP-B58-C11: Replace per-call `reqwest::Client::new()` creations with a
//! single lazily-initialized `OnceLock` static so all subsystems (alert
//! manager webhooks, security advisor digests, vault rotator, proxy-detecting
//! GitHub client, etc.) share a common connection pool and default timeout.

use std::sync::OnceLock;
use std::time::Duration;

/// Returns a reference to the process-global `reqwest::Client`.
///
/// The client is created once on first access and configured with a 30-second
/// default timeout. All callers share the same connection pool.
pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create shared HTTP client")
    })
}
