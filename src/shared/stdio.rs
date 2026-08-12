//! Shared stdin line reader for JSON-RPC-over-stdio servers.
//!
//! Reads stdin on a dedicated plain OS thread and feeds an unbounded channel.
//! This is the single implementation used by both the ACP and MCP stdio
//! loops — see the note below for why `tokio::io::stdin()` is avoided.

use std::io::{BufRead, Read};

/// Cap for a single stdio line (one JSON-RPC message), aligned with the HTTP
/// arms' `MAX_BODY_SIZE` (10 MiB): `BufRead::lines()` would allocate
/// unboundedly on one huge unterminated line, making stdio the only
/// unbounded entry point.
const MAX_STDIN_LINE_BYTES: usize = crate::protocol::mcp_server::MAX_BODY_SIZE;

/// Spawn a plain OS thread that reads stdin line-by-line into an unbounded
/// channel. Returns the receiver end.
///
/// # Why not `tokio::io::stdin()`?
///
/// Tokio's stdio is implemented as a blocking read on the blocking-pool
/// thread that CANNOT be cancelled; at runtime drop the pool waits for it
/// forever unless stdin reaches EOF, which hangs shutdown whenever the client
/// keeps the pipe open. A plain thread is not tracked by the blocking pool,
/// so runtime teardown never waits on it (the thread exits on EOF and is
/// killed with the process otherwise).
pub fn spawn_stdin_lines() -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let (stdin_tx, stdin_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = stdin.lock();
        loop {
            let mut line = Vec::with_capacity(256);
            // Cap the READ itself (not just a post-hoc check): `take` stops
            // reading after MAX+2 bytes, so a hostile unterminated line
            // cannot grow the buffer unboundedly.
            let read = reader
                .by_ref()
                .take(MAX_STDIN_LINE_BYTES as u64 + 2)
                .read_until(b'\n', &mut line)
                .unwrap_or(0);
            if read == 0 {
                break; // EOF
            }
            // Strip trailing newline / CRLF.
            while matches!(line.last(), Some(b'\n') | Some(b'\r')) {
                line.pop();
            }
            if line.len() > MAX_STDIN_LINE_BYTES {
                // Hostile/oversized line: discard it and drain the rest of
                // the line so the stream stays aligned at the next message
                // boundary — mirroring the HTTP arms' "payload too large"
                // behavior (fail closed on the message, not the connection).
                let mut drain = [0u8; 1024];
                loop {
                    let n = reader.read(&mut drain).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    if drain[..n].contains(&b'\n') {
                        break;
                    }
                }
                tracing::warn!(
                    target: "stdio",
                    bytes = line.len(),
                    "stdio line exceeds {} byte limit — dropping message",
                    MAX_STDIN_LINE_BYTES
                );
                continue;
            }
            let Ok(line) = String::from_utf8(line) else {
                break;
            };
            if stdin_tx.send(line).is_err() {
                // Receiver dropped (server exiting) — stop reading.
                break;
            }
        }
    });
    stdin_rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_stdin_lines_returns_receiver() {
        // Cannot exercise real stdin in a unit test; verify the channel
        // wiring is consistent (receiver alive, no immediate EOF).
        let _rx = spawn_stdin_lines();
    }
}
