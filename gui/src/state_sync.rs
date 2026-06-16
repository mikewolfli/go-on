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
/// Mirrors `src/protocol/state_sync.rs::StateSyncEvent`.
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
/// The listener retries indefinitely with exponential backoff on disconnect.
pub fn start_state_sync_listener(
    base_url: &str,
    event_tx: std::sync::mpsc::Sender<StateSyncEvent>,
) {
    let url = format!("{}/v1/state/events", base_url.trim_end_matches('/'));
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            match listen_sse_once(&client, &url, &event_tx).await {
                Ok(()) => {
                    // Clean disconnect — retry after a brief pause
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                Err(e) => {
                    eprintln!("state sync listener error: {}; retrying in 10s", e);
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }
        }
    });
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

            if let Some(data_str) = extract_sse_data(&frame) {
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

/// Extract the `data:` field value from a single SSE frame.
fn extract_sse_data(frame: &str) -> Option<String> {
    for line in frame.lines() {
        if let Some(value) = line.strip_prefix("data: ") {
            return Some(value.trim().to_string());
        }
    }
    None
}
