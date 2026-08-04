//! SSE streaming decompression for gzip/deflate-encoded event streams.
//!
//! Provides optional gzip decompression for SSE event streams to handle
//! gzip-compressed streaming responses from LLM APIs.
//!
//! Uses `flate2` for gzip decompression with a configurable buffer threshold.
//! When the internal buffer exceeds the threshold, the accumulated data is
//! decompressed and emitted. Any remaining data can be flushed at stream end.
//! (Despite the file name, this module is a **decompressor** — outgoing SSE
//! payloads are not compressed here.)

use flate2::read::MultiGzDecoder;
use std::io::Read;

/// Configuration for SSE streaming behavior
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// Enable gzip **decompression** of the incoming SSE stream
    /// (set when the provider may send gzip-encoded chunks).
    pub enable_compression: bool,
    /// Minimum number of uncompressed bytes to buffer before decompressing
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

/// Buffers raw bytes and emits gzip-decompressed chunks when the
/// accumulated size reaches or exceeds the configured threshold.
///
/// Despite its name, this is a **decompressor** — it decompresses
/// gzip-encoded streaming data.
pub struct SseDecompressor {
    buffer: Vec<u8>,
    threshold: usize,
    enabled: bool,
}

impl SseDecompressor {
    /// Create a new decompressor with the given configuration.
    pub fn new(config: &StreamingConfig) -> Self {
        Self {
            buffer: Vec::new(),
            threshold: config.compression_threshold,
            enabled: config.enable_compression,
        }
    }

    /// Feed a raw (possibly gzip-compressed) data chunk into the decompressor.
    ///
    /// If decompression is enabled and the internal buffer reaches or exceeds
    /// the threshold, the buffered data is gzip-decompressed and returned
    /// as a single decompressed chunk. Otherwise, the raw data is
    /// returned as-is (passthrough mode).
    pub fn decompress_chunk(&mut self, data: &[u8]) -> Vec<u8> {
        if !self.enabled {
            return data.to_vec();
        }

        self.buffer.extend_from_slice(data);

        if self.buffer.len() >= self.threshold {
            let decompressed = self.decompress_buffer();
            self.buffer.clear();
            decompressed
        } else {
            // Hold in buffer, nothing emitted yet
            Vec::new()
        }
    }

    /// Flush any remaining buffered data.
    ///
    /// If decompression is enabled, the remaining buffer is decompressed.
    /// Otherwise, returns the raw buffer contents.
    pub fn flush(&mut self) -> Vec<u8> {
        if self.enabled && !self.buffer.is_empty() {
            let decompressed = self.decompress_buffer();
            self.buffer.clear();
            decompressed
        } else if !self.enabled {
            std::mem::take(&mut self.buffer)
        } else {
            Vec::new()
        }
    }

    /// Whether decompression is enabled on this decompressor.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Number of bytes currently held in the internal buffer.
    pub fn buffered_bytes(&self) -> usize {
        self.buffer.len()
    }

    /// Decompress the internal buffer using gzip.
    fn decompress_buffer(&self) -> Vec<u8> {
        let mut decoder = MultiGzDecoder::new(&self.buffer[..]);
        let mut result = Vec::new();
        // Read errors from a &[u8] are infallible for MultiGzDecoder
        // when the data is valid gzip. If invalid, we return empty data
        // rather than panicking.
        let _ = decoder.read_to_end(&mut result);
        result
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
        let mut comp = SseDecompressor::new(&config);
        let data = b"data: hello\n\n";
        let result = comp.decompress_chunk(data);
        // Returns uncompressed data as-is
        assert_eq!(result, data);
    }

    #[test]
    fn buffers_below_threshold() {
        let config = StreamingConfig {
            enable_compression: true,
            compression_threshold: 1024,
        };
        let mut comp = SseDecompressor::new(&config);
        let result = comp.decompress_chunk(b"hello");
        // Below threshold, nothing emitted yet
        assert!(result.is_empty());
        assert_eq!(comp.buffered_bytes(), 5);
    }

    #[test]
    fn decompresses_when_threshold_exceeded() {
        let config = StreamingConfig {
            enable_compression: true,
            compression_threshold: 10,
        };
        let mut comp = SseDecompressor::new(&config);
        // First we manually compress some data to feed to the decompressor
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        let data = b"Hello world from gzip!";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("write_all should succeed");
        let compressed = encoder.finish().expect("finish should succeed");

        // First chunk below threshold
        let r1 = comp.decompress_chunk(&compressed[..5]);
        assert!(r1.is_empty());
        // Second chunk pushes past threshold
        let r2 = comp.decompress_chunk(&compressed[5..]);
        assert!(!r2.is_empty());
        // Should contain the decompressed text
        let output = String::from_utf8_lossy(&r2);
        assert!(output.contains("Hello world"));
        assert_eq!(comp.buffered_bytes(), 0);
    }

    #[test]
    fn flush_emits_remaining() {
        let config = StreamingConfig {
            enable_compression: true,
            compression_threshold: 1024,
        };
        let mut comp = SseDecompressor::new(&config);
        // Small gzip data below threshold, flush should decompress
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        let data = b"small data";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("write_all should succeed");
        let compressed = encoder.finish().expect("finish should succeed");

        comp.decompress_chunk(&compressed);
        let flushed = comp.flush();
        assert!(!flushed.is_empty());
        let output = String::from_utf8_lossy(&flushed);
        assert!(output.contains("small data"));
        assert_eq!(comp.buffered_bytes(), 0);
    }

    #[test]
    fn flush_passthrough_when_disabled() {
        let config = StreamingConfig {
            enable_compression: false,
            compression_threshold: 1024,
        };
        let mut comp = SseDecompressor::new(&config);
        // When disabled, decompress_chunk returns data immediately
        let chunk = comp.decompress_chunk(b"raw bytes");
        assert_eq!(chunk, b"raw bytes");
        // Flush has nothing left
        let flushed = comp.flush();
        assert!(flushed.is_empty());
    }

    #[test]
    fn roundtrip_compress_decompress() {
        let config = StreamingConfig {
            enable_compression: true,
            compression_threshold: 5,
        };
        let mut comp = SseDecompressor::new(&config);
        let original = b"Hello, World! This is a test of SSE decompression.";
        // Manually compress the original
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(original)
            .expect("write_all should succeed");
        let compressed = encoder.finish().expect("finish should succeed");

        // Feed compressed data to the decompressor
        let decompressed = comp.decompress_chunk(&compressed);
        assert_eq!(decompressed, original);
    }
}
