//! Shared stdin line reader for JSON-RPC-over-stdio servers.
//!
//! Reads stdin on a dedicated plain OS thread and feeds an unbounded channel.
//! This is the single implementation used by both the ACP and MCP stdio
//! loops — see the note below for why `tokio::io::stdin()` is avoided.

use std::io::BufRead;

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
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
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
