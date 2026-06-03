//! Shared HTTP client singleton for connection-pool reuse across the process.
//!
//! GAP-B58-C11: Replace per-call `reqwest::Client::new()` creations with a
//! single lazily-initialized `OnceLock` static so all subsystems (alert
//! manager webhooks, security advisor digests, vault rotator, proxy-detecting
//! GitHub client, etc.) share a common connection pool and default timeout.

use std::sync::OnceLock;
use std::time::Duration;

/// Inner storage that separates initialization success from the public API.
///
/// [`OnceLock::get_or_init`] always runs the initializer, but `build()` can
/// fail (e.g. missing TLS backend). By storing a `Result` we propagate the
/// error on first access instead of panicking with `.expect()`.
static CLIENT: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();

/// Returns a reference to the process-global `reqwest::Client`.
///
/// The client is created once on first access and configured with a 30-second
/// default timeout. All callers share the same connection pool.
///
/// # Errors
///
/// Returns an error if the underlying `reqwest::Client` cannot be built
/// (e.g. TLS backend initialization failure).
pub fn http_client() -> Result<&'static reqwest::Client, &'static reqwest::Error> {
    let result = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
    });
    result.as_ref()
}
