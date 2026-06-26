//! SSE buffer pool — pre-allocated byte buffers to reduce allocation churn
//! during SSE event serialization.

use std::sync::Mutex;

// ---------------------------------------------------------------------------
// SseBufferPool
// ---------------------------------------------------------------------------

/// A pool of pre-allocated byte buffers for SSE event serialization.
/// Avoids allocation churn during high-frequency streaming.
pub struct SseBufferPool {
    buffers: Mutex<Vec<Vec<u8>>>,
    max_capacity: usize,
}

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
        let mut guard = self.buffers.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "sse_optimizer", "SseBufferPool buffers Mutex poisoned – recovering");
            poisoned.into_inner()
        });
        guard
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(self.max_capacity))
    }

    pub fn release(&self, mut buf: Vec<u8>) {
        buf.clear();
        let mut guard = self.buffers.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "sse_optimizer", "SseBufferPool buffers Mutex poisoned – recovering");
            poisoned.into_inner()
        });
        if guard.len() < 32 {
            guard.push(buf);
        }
    }
}
