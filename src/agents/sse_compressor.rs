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

/// Cap for a single gzip decompression call (zip-bomb guard).
///
/// The compressed buffer feeding this is bounded (~threshold + one network
/// chunk), but the *decompressed* output was previously unbounded: a tiny
/// hostile gzip stream could expand to gigabytes and blow up memory before
/// the SSE parser's line/event caps (1 MiB / 4 MiB in agents/mod.rs) ever
/// saw the data. 16 MiB is a generous multiple of the parser caps for any
/// legitimate LLM stream chunk.
const MAX_DECOMPRESSED_BYTES: usize = 16 * 1024 * 1024;

/// Cap for the accumulated compressed input (a legitimate gzip-encoded LLM
/// stream is far below this; the cap only bounds hostile inputs).
const MAX_COMPRESSED_BUFFER: usize = 64 * 1024 * 1024;

/// gzip magic bytes (RFC 1952): `1f 8b`.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

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
    /// Accumulated compressed input (gzip path).
    buffer: Vec<u8>,
    enabled: bool,
    /// Whether the stream is gzip-encoded, decided from the first two bytes.
    /// `None` until enough bytes have arrived to decide. When the provider
    /// serves identity encoding (the norm when the client does not advertise
    /// `Accept-Encoding: gzip`), the stream passes through untouched.
    detected_gzip: Option<bool>,
}

impl SseDecompressor {
    /// Create a new decompressor with the given configuration.
    pub fn new(config: &StreamingConfig) -> Self {
        Self {
            buffer: Vec::new(),
            enabled: config.enable_compression,
            detected_gzip: None,
        }
    }

    /// Feed a raw (possibly gzip-compressed) data chunk into the decompressor.
    ///
    /// If decompression is enabled and the stream is gzip-encoded, the bytes
    /// are accumulated and decompressed in `flush()` at stream end — a
    /// partial gzip member must never be handed to `MultiGzDecoder` mid-stream
    /// (the former threshold-based path re-created the decoder on every
    /// crossing, losing the inflate position and corrupting multi-chunk
    /// streams). If the stream is identity-encoded, the data passes through
    /// as-is. Otherwise, the raw data is returned as-is (passthrough mode).
    pub fn decompress_chunk(&mut self, data: &[u8]) -> Vec<u8> {
        if !self.enabled {
            return data.to_vec();
        }

        // Decide gzip vs identity from the first bytes of the stream.
        if self.detected_gzip.is_none() {
            if data.is_empty() {
                return Vec::new();
            }
            let mut probe = std::mem::take(&mut self.buffer);
            probe.extend_from_slice(data);
            if probe.len() < 2 {
                // Not enough bytes to decide yet — hold and wait.
                self.buffer = probe;
                return Vec::new();
            }
            let is_gzip = probe[0] == GZIP_MAGIC[0] && probe[1] == GZIP_MAGIC[1];
            self.detected_gzip = Some(is_gzip);
            if !is_gzip {
                // Identity encoding: emit the held prefix and pass through
                // the rest of the stream untouched.
                return probe;
            }
            self.buffer = probe;
            return Vec::new();
        }

        match self.detected_gzip {
            Some(false) => data.to_vec(),
            Some(true) => {
                self.buffer.extend_from_slice(data);
                if self.buffer.len() > MAX_COMPRESSED_BUFFER {
                    tracing::warn!(
                        target: "sse_compressor",
                        bytes = self.buffer.len(),
                        "SSE gzip input exceeded {} byte cap — stream truncated",
                        MAX_COMPRESSED_BUFFER
                    );
                    self.buffer.truncate(MAX_COMPRESSED_BUFFER);
                }
                Vec::new()
            }
            None => unreachable!("handled above"),
        }
    }

    /// Flush any remaining buffered data.
    ///
    /// If decompression is enabled and the stream is gzip, the full
    /// accumulated member is decompressed here. Otherwise, returns the raw
    /// buffer contents.
    pub fn flush(&mut self) -> Vec<u8> {
        match self.detected_gzip {
            Some(true) => {
                let input = std::mem::take(&mut self.buffer);
                self.decompress_buffer(&input)
            }
            // Identity / undecided / disabled: pass through whatever was held.
            _ => std::mem::take(&mut self.buffer),
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

    /// Decompress a complete gzip byte slice.
    fn decompress_buffer(&self, input: &[u8]) -> Vec<u8> {
        let decoder = MultiGzDecoder::new(input);
        let mut result = Vec::new();
        // Read errors from a &[u8] are infallible for MultiGzDecoder
        // when the data is valid gzip. If invalid, we return empty data
        // rather than panicking.
        let read = decoder
            .take(MAX_DECOMPRESSED_BYTES as u64 + 1)
            .read_to_end(&mut result)
            .unwrap_or(0);
        if read > MAX_DECOMPRESSED_BYTES {
            // Zip-bomb: truncate (rather than allocate unboundedly) and make
            // the truncation visible instead of silently returning partial data.
            tracing::warn!(
                target: "sse_compressor",
                bytes = read,
                "SSE gzip decompression exceeded {} byte cap — stream truncated",
                MAX_DECOMPRESSED_BYTES
            );
            result.truncate(MAX_DECOMPRESSED_BYTES);
        }
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
    fn identity_stream_passes_through() {
        let config = StreamingConfig {
            enable_compression: true,
            compression_threshold: 1024,
        };
        let mut comp = SseDecompressor::new(&config);
        // Non-gzip (identity) input is passed through untouched once the
        // first two bytes rule out gzip magic — the provider did not send
        // compressed data.
        let result = comp.decompress_chunk(b"hello");
        assert_eq!(result, b"hello");
        // A single-byte first chunk is held until the encoding can be decided.
        let mut comp2 = SseDecompressor::new(&config);
        assert!(comp2.decompress_chunk(b"h").is_empty());
        let rest = comp2.decompress_chunk(b"i there");
        assert_eq!(rest, b"hi there");
        assert_eq!(comp2.flush(), b"");
    }

    #[test]
    fn decompresses_on_flush() {
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

        // Gzip input is accumulated; nothing is emitted mid-stream (a partial
        // gzip member must never be handed to MultiGzDecoder).
        let r1 = comp.decompress_chunk(&compressed[..5]);
        assert!(r1.is_empty());
        let r2 = comp.decompress_chunk(&compressed[5..]);
        assert!(r2.is_empty());
        // Decompression happens once at stream end.
        let flushed = comp.flush();
        let output = String::from_utf8_lossy(&flushed);
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

        // Feed compressed data to the decompressor; output arrives at flush.
        assert!(comp.decompress_chunk(&compressed).is_empty());
        let decompressed = comp.flush();
        assert_eq!(decompressed, original);
    }
}
