//! SSE streaming compression using gzip/deflate.
//!
//! Provides optional gzip compression for SSE event streams to reduce
//! bandwidth consumption for large streaming responses from LLM APIs.
//!
//! Uses `flate2` for gzip compression with a configurable buffer threshold.
//! When the internal buffer exceeds the threshold, the accumulated data is
//! compressed and emitted. Any remaining data can be flushed at stream end.

use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;

/// Configuration for SSE streaming behavior
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// Enable gzip compression on the SSE stream
    pub enable_compression: bool,
    /// Minimum number of uncompressed bytes to buffer before compressing
    pub compression_threshold: usize,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            enable_compression: false,
            compression_threshold: 1024,
        }
    }
}

/// Buffers raw bytes and emits gzip-compressed chunks when the
/// accumulated size reaches or exceeds the configured threshold.
pub struct SseCompressor {
    buffer: Vec<u8>,
    threshold: usize,
    enabled: bool,
}

impl SseCompressor {
    /// Create a new compressor with the given configuration.
    pub fn new(config: &StreamingConfig) -> Self {
        Self {
            buffer: Vec::new(),
            threshold: config.compression_threshold,
            enabled: config.enable_compression,
        }
    }

    /// Feed a raw data chunk into the compressor.
    ///
    /// If compression is enabled and the internal buffer reaches or exceeds
    /// the threshold, the buffered data is gzip-compressed and returned
    /// as a single compressed chunk. Otherwise, the uncompressed data is
    /// returned as-is (passthrough mode).
    pub fn compress_chunk(&mut self, data: &[u8]) -> Vec<u8> {
        if !self.enabled {
            return data.to_vec();
        }

        self.buffer.extend_from_slice(data);

        if self.buffer.len() >= self.threshold {
            let compressed = self.compress_buffer();
            self.buffer.clear();
            compressed
        } else {
            // Hold in buffer, nothing emitted yet
            Vec::new()
        }
    }

    /// Flush any remaining buffered data.
    ///
    /// If compression is enabled, the remaining buffer is compressed.
    /// Otherwise, returns the raw buffer contents.
    pub fn flush(&mut self) -> Vec<u8> {
        if self.enabled && !self.buffer.is_empty() {
            let compressed = self.compress_buffer();
            self.buffer.clear();
            compressed
        } else if !self.enabled {
            std::mem::take(&mut self.buffer)
        } else {
            Vec::new()
        }
    }

    /// Whether compression is enabled on this compressor.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Number of bytes currently held in the internal buffer.
    pub fn buffered_bytes(&self) -> usize {
        self.buffer.len()
    }

    /// Compress the internal buffer using gzip at the default level.
    fn compress_buffer(&self) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        // Write errors to a Vec are infallible.
        // Writing to a Vec<T> is infallible per std::io::Write contract.
        encoder.write_all(&self.buffer).expect("gzip write to vec");
        encoder.finish().expect("gzip finish to vec")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_compression_disabled() {
        let config = StreamingConfig {
            enable_compression: false,
            compression_threshold: 64,
        };
        let mut comp = SseCompressor::new(&config);
        let data = b"data: hello\n\n";
        let result = comp.compress_chunk(data);
        // Returns uncompressed data as-is
        assert_eq!(result, data);
    }

    #[test]
    fn buffers_below_threshold() {
        let config = StreamingConfig {
            enable_compression: true,
            compression_threshold: 1024,
        };
        let mut comp = SseCompressor::new(&config);
        let result = comp.compress_chunk(b"hello");
        // Below threshold, nothing emitted yet
        assert!(result.is_empty());
        assert_eq!(comp.buffered_bytes(), 5);
    }

    #[test]
    fn compresses_when_threshold_exceeded() {
        let config = StreamingConfig {
            enable_compression: true,
            compression_threshold: 10,
        };
        let mut comp = SseCompressor::new(&config);
        // First chunk below threshold
        let r1 = comp.compress_chunk(b"hello");
        assert!(r1.is_empty());
        // Second chunk pushes past threshold
        let r2 = comp.compress_chunk(b" world!");
        assert!(!r2.is_empty());
        // Should be compressed (gzip header magic bytes: 0x1f 0x8b)
        assert_eq!(r2[0], 0x1f);
        assert_eq!(r2[1], 0x8b);
        assert_eq!(comp.buffered_bytes(), 0);
    }

    #[test]
    fn flush_emits_remaining() {
        let config = StreamingConfig {
            enable_compression: true,
            compression_threshold: 1024,
        };
        let mut comp = SseCompressor::new(&config);
        comp.compress_chunk(b"small data");
        let flushed = comp.flush();
        assert!(!flushed.is_empty());
        assert_eq!(flushed[0], 0x1f);
        assert_eq!(flushed[1], 0x8b);
        assert_eq!(comp.buffered_bytes(), 0);
    }

    #[test]
    fn flush_passthrough_when_disabled() {
        let config = StreamingConfig {
            enable_compression: false,
            compression_threshold: 1024,
        };
        let mut comp = SseCompressor::new(&config);
        // When disabled, compress_chunk returns data immediately
        let chunk = comp.compress_chunk(b"raw bytes");
        assert_eq!(chunk, b"raw bytes");
        // Flush has nothing left
        let flushed = comp.flush();
        assert!(flushed.is_empty());
    }

    #[test]
    fn roundtrip_decompress() {
        let config = StreamingConfig {
            enable_compression: true,
            compression_threshold: 5,
        };
        let mut comp = SseCompressor::new(&config);
        let original = b"Hello, World! This is a test of SSE compression.";
        let compressed = comp.compress_chunk(original);
        // Decompress with flate2
        use flate2::read::GzDecoder;
        use std::io::Read;
        let mut decoder = GzDecoder::new(&compressed[..]);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();
        assert_eq!(decompressed, original);
    }
}
