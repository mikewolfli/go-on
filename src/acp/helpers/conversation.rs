//! Conversation helper functions for ACP server
//!
//! This module provides utility functions for managing streaming limits,
//! latency monitoring, pipeline gates, and storage validation.

/// Maximum stream chunks
pub const MAX_STREAM_CHUNKS: usize = 4_096;

/// Maximum stream characters
pub const MAX_STREAM_CHARS: usize = 256_000;

/// Drain an `UnboundedReceiver` of stream tokens into a `String`, capped at
/// the shared stream limits (see [`stream_would_exceed_limits`]). This is the
/// plain-drain counterpart to [`collect_chat_output_capped`] for callers that
/// run the chat themselves (spawn/await on their side) and only need to
/// collect the resulting stream. Truncation warns — explicit, never silent.
pub async fn drain_channel_capped(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
) -> String {
    let mut full = String::new();
    let mut chunks = 0usize;
    let mut total_chars = 0usize;
    while let Some(token) = receiver.recv().await {
        let next_chars = token.chars().count();
        if stream_would_exceed_limits(chunks, total_chars, next_chars) {
            tracing::warn!("stream output truncated at {total_chars} chars (chunks {chunks})");
            break;
        }
        full.push_str(&token);
        chunks += 1;
        total_chars += next_chars;
    }
    full
}

/// Check if streaming would exceed limits
pub fn stream_would_exceed_limits(
    current_chunks: usize,
    current_chars: usize,
    next_token_chars: usize,
) -> bool {
    current_chunks.saturating_add(1) > MAX_STREAM_CHUNKS
        || current_chars.saturating_add(next_token_chars) > MAX_STREAM_CHARS
}

/// Run an agent chat and collect its streamed tokens into a `String`,
/// bounded by the shared stream caps (see [`stream_would_exceed_limits`]).
///
/// The chat future and the channel are polled CONCURRENTLY: a "await chat,
/// then drain" order would buffer the entire stream in the unbounded channel
/// before any cap applied. On truncation or `timeout` the loop breaks — the
/// pinned chat future is dropped, which cancels the in-flight model request.
pub async fn collect_chat_output_capped<F, E>(
    chat_future: F,
    receiver: tokio::sync::mpsc::UnboundedReceiver<String>,
    timeout: Option<std::time::Duration>,
) -> Result<String, anyhow::Error>
where
    F: std::future::Future<Output = Result<(), E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    let mut receiver = receiver;
    tokio::pin!(chat_future);
    let deadline = timeout.map(|d| tokio::time::Instant::now() + d);
    let mut full_response = String::new();
    let mut chunks = 0usize;
    let mut total_chars = 0usize;

    macro_rules! append_capped {
        ($token:expr) => {{
            let next_chars = $token.chars().count();
            if stream_would_exceed_limits(chunks, total_chars, next_chars) {
                tracing::warn!(
                    "agent chat output truncated at {total_chars} chars (chunks {chunks})"
                );
                break;
            }
            full_response.push_str(&$token);
            chunks += 1;
            total_chars += next_chars;
        }};
    }

    loop {
        tokio::select! {
            chat_res = &mut chat_future => {
                chat_res.map_err(|e| anyhow::anyhow!("agent chat failed: {e}"))?;
                // Chat finished — drain any tokens still buffered in the channel.
                while let Some(token) = receiver.recv().await {
                    append_capped!(token);
                }
                break;
            }
            token = receiver.recv() => {
                match token {
                    Some(token) => append_capped!(token),
                    None => break, // sender dropped — chat ended
                }
            }
            _ = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                anyhow::bail!("agent chat timed out after {:?}", timeout);
            }
        }
    }
    Ok(full_response)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── stream limits ─────────────────────────────────────────────────

    #[test]
    fn stream_would_exceed_limits_chunk_boundary() {
        assert!(stream_would_exceed_limits(MAX_STREAM_CHUNKS, 0, 0));
        assert!(!stream_would_exceed_limits(MAX_STREAM_CHUNKS - 1, 0, 10));
    }

    #[test]
    fn stream_would_exceed_limits_char_boundary() {
        assert!(stream_would_exceed_limits(0, MAX_STREAM_CHARS, 1));
        assert!(!stream_would_exceed_limits(0, MAX_STREAM_CHARS - 100, 50));
    }

    #[tokio::test]
    async fn collect_chat_output_capped_collects_full_small_stream() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let chat = async move {
            tx.send("hello".to_string()).unwrap();
            tx.send(" world".to_string()).unwrap();
            drop(tx);
            Ok::<(), std::io::Error>(())
        };
        let out = super::collect_chat_output_capped(chat, rx, None)
            .await
            .expect("collect");
        assert_eq!(out, "hello world");
    }

    #[tokio::test]
    async fn collect_chat_output_capped_truncates_oversized_stream() {
        // 10 × 30k chars = 300k chars > MAX_STREAM_CHARS (256k): the stream
        // must be capped at the limit and the partial output returned.
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let chat = async move {
            for _ in 0..10 {
                tx.send("x".repeat(30_000)).unwrap();
            }
            drop(tx);
            Ok::<(), std::io::Error>(())
        };
        let out = super::collect_chat_output_capped(chat, rx, None)
            .await
            .expect("collect");
        assert!(
            out.chars().count() <= MAX_STREAM_CHARS,
            "must be capped, got {} chars",
            out.chars().count()
        );
        assert!(!out.is_empty(), "partial output is returned, never silent");
    }

    #[tokio::test]
    async fn collect_chat_output_capped_enforces_timeout() {
        // A chat that never sends nor completes must be cut off by `timeout`.
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let chat = async move {
            std::future::pending::<()>().await;
            drop(tx);
            Ok::<(), std::io::Error>(())
        };
        let err =
            super::collect_chat_output_capped(chat, rx, Some(std::time::Duration::from_millis(50)))
                .await
                .expect_err("must time out");
        assert!(err.to_string().contains("timed out"), "got: {err}");
    }

    #[tokio::test]
    async fn drain_channel_capped_caps_oversized_stream() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for _ in 0..10 {
            tx.send("x".repeat(30_000)).unwrap();
        }
        drop(tx);
        let out = super::drain_channel_capped(&mut rx).await;
        assert!(
            out.chars().count() <= MAX_STREAM_CHARS,
            "must be capped, got {} chars",
            out.chars().count()
        );
        assert!(!out.is_empty(), "partial output is returned, never silent");
    }
}
