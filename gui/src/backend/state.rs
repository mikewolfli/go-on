use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::validate_input_size;

// ── StreamProcessor ────────────────────────────────────────────────────────

const MAX_SSE_LINE_LENGTH: usize = 1024 * 1024; // 1 MB per SSE line
const MAX_SSE_CHUNK_SIZE: usize = 16 * 1024 * 1024; // 16 MB total per push call

/// Incrementally parses an SSE byte stream frame-by-frame, extracting `event:`
/// and `data:` fields. Returns parsed JSON values with the event type attached
/// (either as a top-level `"_event_type"` field, or through the existing structure).
/// Tracks token count and total bytes processed for progress reporting in the UI.
pub struct StreamProcessor {
    buffer: String,
    max_buffer_size: usize,
    /// Number of JSON tokens (events) parsed so far.
    pub token_count: usize,
    /// Total bytes consumed from the wire.
    pub total_bytes_processed: usize,
}

impl StreamProcessor {
    pub fn new() -> Self {
        Self {
            buffer: String::with_capacity(16_384),
            max_buffer_size: MAX_SSE_LINE_LENGTH,
            token_count: 0,
            total_bytes_processed: 0,
        }
    }

    /// Feed a chunk of raw bytes into the processor.
    /// Returns a batch of parsed results (Ok(values) or Err(errors)).
    /// Each parsed value now includes an `"_event_type"` field extracted from
    /// the SSE `event:` line, allowing the caller to distinguish between chunk,
    /// done, telemetry, and other event types emitted by the backend.
    pub fn push_chunk(&mut self, chunk: &[u8]) -> Vec<Result<Value, String>> {
        let mut events: Vec<Result<Value, String>> = Vec::new();

        // Validate input size before processing
        if let Err(e) = validate_input_size(chunk, MAX_SSE_CHUNK_SIZE) {
            events.push(Err(e));
            return events;
        }

        // Overflow guard
        if self.buffer.len() + chunk.len() > self.max_buffer_size {
            events.push(Err("SSE buffer overflow (exceeded 1 MB)".to_string()));
            return events;
        }

        self.total_bytes_processed += chunk.len();

        // Normalise CRLF → LF for consistent frame splitting
        let part = String::from_utf8_lossy(chunk);
        self.buffer.push_str(&part.replace('\r', ""));

        // Consume complete SSE frames (delimited by \n\n, fallback \n)
        loop {
            let (delim, delim_len) = if self.buffer.contains("\n\n") {
                ("\n\n", 2usize)
            } else {
                ("\n", 1usize)
            };

            let pos = match self.buffer.find(delim) {
                Some(p) => p,
                None => break,
            };

            let segment = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + delim_len..].to_string();

            // Safety: reject unbounded lines
            if segment.len() > MAX_SSE_LINE_LENGTH {
                events.push(Err("SSE line exceeds maximum length (1 MB)".to_string()));
                return events;
            }

            // Collect lines from the segment.
            // When using \n\n delimiter the segment may contain embedded \n
            // (multi-line SSE data), so split further on single \n.
            let sub_lines: Vec<&str> = if delim_len == 2 {
                segment.split('\n').collect()
            } else {
                vec![&segment]
            };

            let mut current_event_type: Option<String> = None;
            let mut current_data: Option<String> = None;

            for line in &sub_lines {
                if let Some(event) = line.strip_prefix("event: ") {
                    current_event_type = Some(event.trim().to_string());
                } else if let Some(data) = line.strip_prefix("data: ") {
                    current_data = Some(data.trim().to_string());
                } else if let Some(data) = line.strip_prefix("data:") {
                    // Handle "data: {json}" without space after colon
                    current_data = Some(data.trim().to_string());
                }
            }

            // Emit a single event per frame, combining event type + data
            if let Some(data_str) = current_data {
                if data_str == "[DONE]" {
                    let mut val = Value::String("[DONE]".to_string());
                    if let Some(ev) = current_event_type {
                        // Wrap [DONE] in an object with event type
                        val = serde_json::json!({
                            "_event_type": ev,
                            "data": "[DONE]",
                        });
                    }
                    events.push(Ok(val));
                    continue;
                }

                match serde_json::from_str::<Value>(&data_str) {
                    Ok(mut val) => {
                        self.token_count += 1;
                        // Inject the event type so callers can distinguish
                        // "chunk", "done", "telemetry", etc.
                        if let Some(ev) = current_event_type {
                            if let Some(obj) = val.as_object_mut() {
                                obj.insert("_event_type".to_string(), Value::String(ev));
                            }
                        }
                        events.push(Ok(val));
                    }
                    Err(e) => {
                        events.push(Err(format!("JSON parse error: {}", e)));
                    }
                }
            } else if let Some(ev) = current_event_type {
                // Event with no data payload — emit a minimal object
                events.push(Ok(serde_json::json!({
                    "_event_type": ev,
                    "_no_data": true,
                })));
            }
        }

        events
    }
}

impl Default for StreamProcessor {
    fn default() -> Self {
        Self::new()
    }
}

// ── AbortController ────────────────────────────────────────────────────────

/// Shared cancellation signal for in-progress SSE streams.
/// Cloning produces another handle to the same underlying signal.
/// Uses a `tokio::sync::Notify` so callers can `tokio::select!` on the
/// abort signal and cancel the actual in-flight HTTP request.
#[derive(Clone)]
pub struct AbortController {
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl AbortController {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Signal abort.  Idempotent — safe to call multiple times.
    pub fn abort(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Returns `true` if abort has been signalled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Returns a future that resolves when `abort()` is called.
    /// Use with `tokio::select!` to cancel in-flight HTTP requests:
    ///
    /// ```ignore
    /// tokio::select! {
    ///     result = http_request => { … },
    ///     _ = abort_ctrl.wait_for_abort() => { … },
    /// }
    /// ```
    pub async fn wait_for_abort(&self) {
        self.notify.notified().await;
    }

    /// Reset the signal for reuse.
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }
}

impl Default for AbortController {
    fn default() -> Self {
        Self::new()
    }
}

// ── TokenProgress ───────────────────────────────────────────────────────────

/// Lightweight snapshot of streaming progress for the UI.
#[derive(Debug, Clone, Default)]
pub struct TokenProgress {
    /// Number of tokens (SSE events) received so far.
    pub tokens_received: usize,
    /// Total bytes processed from the wire.
    pub bytes_processed: usize,
    /// Input token count reported by telemetry.
    pub input_tokens: usize,
    /// Output token count reported by telemetry.
    pub output_tokens: usize,
    /// Total token count reported by telemetry.
    pub total_tokens: usize,
}
