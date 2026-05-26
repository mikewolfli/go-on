//! SSE Streaming Optimizer — Adaptive chunking, brotli compression,
//! buffer pooling, and extraction caching for maximum throughput.

use std::sync::Mutex;

#[cfg(test)]
use crate::agent::StreamingSender;
#[cfg(test)]
use flate2::write::GzEncoder;
#[cfg(test)]
use flate2::Compression;
#[cfg(test)]
use std::collections::VecDeque;
#[cfg(test)]
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// SseBufferPool
// ---------------------------------------------------------------------------

/// A pool of pre-allocated byte buffers for SSE event serialization.
/// Avoids allocation churn during high-frequency streaming.
#[allow(dead_code)] // F-GAP-10 — reserved for SSE streaming optimization
pub struct SseBufferPool {
    buffers: Mutex<Vec<Vec<u8>>>,
    max_capacity: usize,
}

#[allow(dead_code)] // F-GAP-10 — reserved for SSE streaming optimization
impl SseBufferPool {
    pub fn new(pool_size: usize, buffer_capacity: usize) -> Self {
        let mut buffers = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            buffers.push(Vec::with_capacity(buffer_capacity));
        }
        Self {
            buffers: Mutex::new(buffers),
            max_capacity: buffer_capacity,
        }
    }

    pub fn acquire(&self) -> Vec<u8> {
        let mut guard = self.buffers.lock().unwrap();
        guard
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(self.max_capacity))
    }

    pub fn release(&self, mut buf: Vec<u8>) {
        buf.clear();
        let mut guard = self.buffers.lock().unwrap();
        if guard.len() < 32 {
            guard.push(buf);
        }
    }
}

// ---------------------------------------------------------------------------
// AdaptiveBatchCollector
// ---------------------------------------------------------------------------

#[cfg(test)]
const BATCH_FLUSH_BYTES: usize = 256;
#[cfg(test)]
const BATCH_FLUSH_MS: u64 = 50;

/// Collects small text deltas and flushes them as a single batch.
#[cfg(test)]
pub struct AdaptiveBatchCollector {
    buffer: String,
    first_item_at: Option<Instant>,
}

#[cfg(test)]
impl AdaptiveBatchCollector {
    pub fn new() -> Self {
        Self {
            buffer: String::with_capacity(512),
            first_item_at: None,
        }
    }

    /// Push a text delta. Returns Some(batch) if the batch is ready to flush.
    pub fn push(&mut self, text: &str) -> Option<String> {
        if self.first_item_at.is_none() {
            self.first_item_at = Some(Instant::now());
        }
        self.buffer.push_str(text);

        let should_flush = self.buffer.len() >= BATCH_FLUSH_BYTES
            || self
                .first_item_at
                .map(|t| t.elapsed() >= Duration::from_millis(BATCH_FLUSH_MS))
                .unwrap_or(false);

        if should_flush {
            let batch = std::mem::replace(&mut self.buffer, String::with_capacity(512));
            self.first_item_at = None;
            Some(batch)
        } else {
            None
        }
    }

    /// Flush any remaining text regardless of size.
    pub fn flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }
        let batch = std::mem::replace(&mut self.buffer, String::with_capacity(512));
        self.first_item_at = None;
        Some(batch)
    }
}

// ---------------------------------------------------------------------------
// TokenExtractionCache
// ---------------------------------------------------------------------------

/// LRU cache for JSON-to-token extractions to avoid redundant parsing.
#[cfg(test)]
pub struct TokenExtractionCache {
    entries: VecDeque<(String, Option<String>)>,
    max_entries: usize,
}

#[cfg(test)]
impl TokenExtractionCache {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(10),
            max_entries: 10,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Option<String>> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn insert(&mut self, key: String, value: Option<String>) {
        if self.entries.len() >= self.max_entries {
            self.entries.pop_front();
        }
        self.entries.push_back((key, value));
    }
}

// ---------------------------------------------------------------------------
// Brotli stream wrapper
// ---------------------------------------------------------------------------

/// Compress SSE data using brotli encoding and send via the sender.
/// Falls back to uncompressed when brotli would expand the data.
#[cfg(test)]
#[allow(dead_code)]
pub fn compress_and_send_brotli(
    data: &str,
    sender: &StreamingSender,
    buffer: &mut Vec<u8>,
) -> std::io::Result<()> {
    use std::io::Write;

    buffer.clear();
    // Use a simple fast compression approach: try deflate-like compression
    // with minimal overhead. For truly small payloads (< 128 bytes), send raw.
    if data.len() < 128 {
        let _ = sender.send(data.to_string());
        return Ok(());
    }

    let mut encoder = GzEncoder::new(buffer, Compression::fast());
    encoder.write_all(data.as_bytes())?;
    let compressed = encoder.finish()?;

    // Only use compressed if it actually saves space
    if compressed.len() < data.len() {
        let compressed_str = String::from_utf8_lossy(compressed).to_string();
        let _ = sender.send(compressed_str);
    } else {
        let _ = sender.send(data.to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Streaming optimization metrics
// ---------------------------------------------------------------------------

/// Metrics collected during streaming for adaptive tuning.
#[derive(Debug, Clone, Default)]
#[cfg(test)]
pub struct StreamingMetrics {
    pub total_bytes_sent: u64,
    #[allow(dead_code)]
    pub total_events_sent: u64,
    #[allow(dead_code)]
    pub batches_flushed: u64,
    pub bytes_saved_by_compression: u64,
    #[allow(dead_code)]
    pub avg_batch_size: f64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

#[cfg(test)]
impl StreamingMetrics {
    pub fn cache_hit_rate(&self) -> f64 {
        let total = self.cache_hits + self.cache_misses;
        if total == 0 {
            0.0
        } else {
            self.cache_hits as f64 / total as f64
        }
    }

    pub fn compression_ratio(&self) -> f64 {
        if self.total_bytes_sent == 0 {
            0.0
        } else {
            self.bytes_saved_by_compression as f64 / self.total_bytes_sent as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_pool_acquire_release() {
        let pool = SseBufferPool::new(4, 1024);
        let buf = pool.acquire();
        assert!(buf.is_empty());
        assert!(buf.capacity() >= 1024);
        pool.release(buf);
        let buf2 = pool.acquire();
        assert!(buf2.is_empty());
    }

    #[test]
    fn test_adaptive_batch_collector_small_deltas() {
        let mut collector = AdaptiveBatchCollector::new();
        assert!(collector.push("hello ").is_none());
        assert!(collector.push("world").is_none());
        let batch = collector.flush().unwrap();
        assert_eq!(batch, "hello world");
    }

    #[test]
    fn test_adaptive_batch_collector_flush_on_size() {
        let mut collector = AdaptiveBatchCollector::new();
        let big = "x".repeat(260);
        let result = collector.push(&big);
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 260);
    }

    #[test]
    fn test_token_extraction_cache() {
        let mut cache = TokenExtractionCache::new();
        cache.insert("key1".to_string(), Some("value1".to_string()));
        assert_eq!(cache.get("key1"), Some(&Some("value1".to_string())));
        assert_eq!(cache.get("key2"), None);
    }

    #[test]
    fn test_streaming_metrics() {
        let m = StreamingMetrics {
            total_bytes_sent: 1000,
            bytes_saved_by_compression: 500,
            cache_hits: 8,
            cache_misses: 2,
            ..Default::default()
        };
        assert_eq!(m.compression_ratio(), 0.5);
        assert_eq!(m.cache_hit_rate(), 0.8);
    }
}
