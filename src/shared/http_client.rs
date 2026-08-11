//! Shared HTTP client singleton for connection-pool reuse across the process.
//!
//! GAP-B58-C11: Replace per-call `reqwest::Client::new()` creations with a
//! single lazily-initialized `OnceLock` static so all subsystems (alert
//! manager webhooks, security advisor digests, vault rotator, proxy-detecting
//! GitHub client, etc.) share a common connection pool and default timeout.
//!
//! # Intentional exceptions (do NOT fold into these singletons)
//!
//! A handful of call sites deliberately build their own clients because the
//! shared 30s timeout ceiling / `http1_only()` / redirect policy / proxy
//! probing would break them — these are config isolation, not drift:
//!   - `cli/chat.rs` and `main/server.rs`: SSE streams need a long timeout
//!     (300s / 120s) and `http1_only()` (avoids DeepSeek HTTP/2 stream reset).
//!   - `tool/extended/http.rs`: `redirect(10)` + no timeout (tool semantics).
//!   - `runtime_pack.rs`: proxy-probing GitHub client (env-key cached).
//!   - `skill_market.rs` / `skill_import.rs`: dedicated timeouts/UAs.
//!   - `crates/go-on-web-search` and the GUI build their own per-crate clients.
//!
//! See docs/log/log-20260811-6.md (debt #4 verdict: keep).

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
            .user_agent("go-on/1.0")
            .build()
    });
    result.as_ref()
}

/// Process-global blocking `reqwest::Client` for sync contexts (tools that
/// run on `spawn_blocking` and cannot use the async client).
///
/// Like [`http_client`], the client is created once and shared for connection
/// pooling; per-request timeouts must be applied by the caller on the request
/// builder (no fixed timeout is baked in so long-running calls like audio
/// transcription are not cut off).
static BLOCKING_CLIENT: OnceLock<Result<reqwest::blocking::Client, reqwest::Error>> =
    OnceLock::new();

/// Returns a reference to the process-global blocking `reqwest::Client`.
///
/// # Errors
///
/// Returns an error if the underlying client cannot be built.
pub fn blocking_http_client() -> Result<&'static reqwest::blocking::Client, &'static reqwest::Error>
{
    let result = BLOCKING_CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .user_agent("go-on/1.0")
            .build()
    });
    result.as_ref()
}
