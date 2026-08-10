use serde::{Deserialize, Serialize};
use std::time::Duration;

// ── State Sync (cross-client) ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecord {
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub enabled: Option<bool>,
    #[serde(alias = "importedAt")]
    pub imported_at: Option<u64>,
}

/// Events received from the backend's `/v1/state/events` SSE endpoint.
/// Single source of truth: `contracts/state-sync-events.json` (verified by
/// `scripts/gen-state-sync-types.py`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StateSyncEvent {
    ModelsChanged {
        models: Vec<String>,
    },
    ConfigReloaded {
        changed_keys: Vec<String>,
    },
    AgentsChanged {
        added: Vec<String>,
        removed: Vec<String>,
    },
    BackendRestarting {
        reason: String,
        restart_in_ms: u64,
    },
    Heartbeat {
        timestamp: u64,
    },
}

/// Spawn a background task that listens to the backend's `/v1/state/events` SSE
/// endpoint and forwards parsed events into a provided channel.
///
/// The listener retries indefinitely on disconnect, using the unified
/// exponential backoff with ±30% jitter (contracts/cross-client-sync.md):
/// `delay = min(1000 × 2^attempt, 30000) × (0.7 + random() × 0.3)`. The
/// attempt counter resets after a clean (long-lived) connection.
///
/// Returns a `JoinHandle` that can be aborted to stop the listener (e.g. during
/// app shutdown or when the backend URL changes).
pub fn start_state_sync_listener(
    base_url: &str,
    event_tx: std::sync::mpsc::Sender<StateSyncEvent>,
    cancel: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let url = format!("{}/v1/state/events", base_url.trim_end_matches('/'));
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let mut attempt: u32 = 0;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    eprintln!("state sync listener: cancelled, shutting down");
                    return;
                }
                result = listen_sse_once(&client, &url, &event_tx) => {
                    match result {
                        Ok(()) => {
                            // Clean disconnect — the stream stayed connected,
                            // so reset the backoff counter per the unified
                            // contract before reconnecting.
                            attempt = 0;
                        }
                        Err(e) => {
                            eprintln!("state sync listener error: {}; retrying", e);
                        }
                    }
                    let delay = state_sync_backoff(attempt);
                    attempt = attempt.saturating_add(1);
                    tokio::time::sleep(delay).await;
                }
            }
        }
    })
}

/// Unified exponential backoff with ±30% jitter, capped at 30 s
/// (contracts/cross-client-sync.md), matching `gui/src/backend/rpc.rs`
/// `retry_backoff` and the VS Code addon.
fn state_sync_backoff(attempt: u32) -> Duration {
    let capped_ms = crate::backoff::exp_backoff_ms(1000, attempt, 30_000);
    let jitter_factor = 0.7 + fastrand::f64() * 0.3;
    Duration::from_secs_f64((capped_ms as f64 * jitter_factor) / 1000.0)
}

/// Connect to the SSE stream once, parse frames, and forward parsed events.
async fn listen_sse_once(
    client: &reqwest::Client,
    url: &str,
    event_tx: &std::sync::mpsc::Sender<StateSyncEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio_stream::StreamExt as _;

    let response = client
        .get(url)
        .timeout(Duration::from_secs(3600))
        .send()
        .await?;

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let text = String::from_utf8_lossy(&chunk);
        buffer.push_str(&text);

        // Process complete SSE frames (separated by \n\n)
        while let Some(frame_end) = buffer.find("\n\n") {
            let frame = buffer[..frame_end].to_string();
            buffer = buffer[frame_end + 2..].to_string();

            let (_, data_str) = crate::backend::state::parse_sse_frame_lines(
                &frame.split('\n').collect::<Vec<_>>(),
            );
            if let Some(data_str) = data_str {
                if data_str == "[DONE]" {
                    continue;
                }
                if let Ok(parsed) = serde_json::from_str::<StateSyncEvent>(&data_str) {
                    let _ = event_tx.send(parsed);
                }
            }
        }
    }

    Ok(())
}
