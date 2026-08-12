//! Shared HTTP/SSE timeout constants.
//!
//! Single source for the 30s socket/stream timeouts used by both HTTP arms
//! (ACP runtime and MCP server) — each previously inlined `Duration::from_secs(30)`
//! in 5+ places with no shared definition, so the two servers could drift.

use std::time::Duration;

/// Timeout for reading an HTTP request header (30s).
pub const HTTP_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Timeout for reading an HTTP request body (30s).
pub const HTTP_BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Timeout for a single socket write (30s).
pub const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Timeout for flushing SSE buffered data (30s).
pub const SSE_FLUSH_TIMEOUT: Duration = Duration::from_secs(30);
/// SSE heartbeat interval (30s) — clients use it to detect dead connections.
pub const SSE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// Maximum HTTP request header size (64 KiB) — bounded read guard shared by
/// the ACP runtime, MCP server, and the hub hand-written HTTP server.
pub const MAX_HTTP_HEADER_SIZE: usize = 64 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_timeouts_are_30s() {
        assert_eq!(HTTP_HEADER_READ_TIMEOUT, Duration::from_secs(30));
        assert_eq!(HTTP_BODY_READ_TIMEOUT, Duration::from_secs(30));
        assert_eq!(SOCKET_WRITE_TIMEOUT, Duration::from_secs(30));
        assert_eq!(SSE_FLUSH_TIMEOUT, Duration::from_secs(30));
        assert_eq!(SSE_HEARTBEAT_INTERVAL, Duration::from_secs(30));
    }

    #[test]
    fn max_header_size_is_64k() {
        assert_eq!(MAX_HTTP_HEADER_SIZE, 64 * 1024);
    }
}
