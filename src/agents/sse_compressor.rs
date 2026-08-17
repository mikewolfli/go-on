//! SSE streaming decompression for gzip/deflate-encoded event streams.
//!
//! Provides optional gzip decompression for SSE event streams to handle
//! gzip-compressed streaming responses from LLM APIs.
//!
//! Uses `flate2` for gzip decompression. A single persistent decoder is fed
//! compressed bytes as each chunk arrives and emits the decompressed bytes
//! immediately, so a gzip-encoded stream stays streaming end-to-end. (An
//! earlier whole-body-buffering design delivered no tokens until `flush()` at
//! stream end, which made streaming responses non-streaming.) Any remaining
//! data can be flushed at stream end.
//! (Despite the file name, this module is a **decompressor** — outgoing SSE
//! payloads are not compressed here.)

use flate2::read::MultiGzDecoder;
use std::io::{self, Read};

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

/// Size of the scratch buffer used to pull decompressed bytes out of the
/// persistent decoder per `read` call.
const PULL_BUFFER_SIZE: usize = 64 * 1024;

/// gzip magic bytes (RFC 1952): `1f 8b`.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Configuration for SSE streaming behavior
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// Enable gzip **decompression** of the incoming SSE stream
    /// (set when the provider may send gzip-encoded chunks).
    pub enable_compression: bool,
    /// Retained for API compatibility — decompression is emitted
    /// incrementally per chunk, so no buffering threshold applies.
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

/// Incrementally-fed compressed-input source for the persistent gzip decoder.
///
/// A `flate2::read::MultiGzDecoder` treats an empty read as end-of-stream,
/// which is wrong mid-stream: the next compressed chunk may still be in
/// flight. To distinguish "no data yet" from "stream over", this reader
/// returns `Err(WouldBlock)` while input is pending and only returns `Ok(0)`
/// once [`IncrementalReader::finish`] has been called (by `flush()`). flate2
/// propagates `WouldBlock` out of `read` without advancing its state, so
/// pulling simply retries once more input arrives.
struct IncrementalReader {
    /// Compressed bytes received but not yet consumed by the decoder.
    buffer: Vec<u8>,
    /// Read cursor into `buffer`; consumed prefixes are drained on push.
    pos: usize,
    /// Set by `finish()` — no further input will ever arrive.
    finished: bool,
}

impl IncrementalReader {
    fn new(initial: Vec<u8>) -> Self {
        Self {
            buffer: initial,
            pos: 0,
            finished: false,
        }
    }

    /// Feed a raw compressed chunk to the reader.
    fn push(&mut self, data: &[u8]) {
        if self.pos > 0 {
            self.buffer.drain(..self.pos);
            self.pos = 0;
        }
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
    }

    /// Declare the end of the stream; subsequent reads report EOF.
    fn finish(&mut self) {
        self.finished = true;
    }

    /// Number of compressed bytes not yet handed to the decoder.
    fn buffered(&self) -> usize {
        self.buffer.len() - self.pos
    }
}

impl Read for IncrementalReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let available = self.buffer.len() - self.pos;
        if available == 0 {
            if self.finished {
                return Ok(0);
            }
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "gzip input not yet available",
            ));
        }
        let n = available.min(buf.len());
        buf[..n].copy_from_slice(&self.buffer[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Feeds raw bytes to a persistent gzip decoder and emits decompressed
/// chunks as soon as they are available.
///
/// Despite its name, this is a **decompressor** — it decompresses
/// gzip-encoded streaming data.
pub struct SseDecompressor {
    /// Compressed bytes held while the gzip/identity decision is pending;
    /// handed to the incremental decoder once gzip is detected.
    buffer: Vec<u8>,
    enabled: bool,
    /// Whether the stream is gzip-encoded, decided from the first two bytes.
    /// `None` until enough bytes have arrived to decide. When the provider
    /// serves identity encoding (the norm when the client does not advertise
    /// `Accept-Encoding: gzip`), the stream passes through untouched.
    detected_gzip: Option<bool>,
    /// Persistent incremental gzip decoder, created once gzip is detected and
    /// fed compressed bytes as they arrive. Keeping one decoder alive across
    /// chunks preserves the inflate position (re-creating it per chunk would
    /// corrupt multi-chunk streams).
    decoder: Option<MultiGzDecoder<IncrementalReader>>,
}

impl SseDecompressor {
    /// Create a new decompressor with the given configuration.
    pub fn new(config: &StreamingConfig) -> Self {
        Self {
            buffer: Vec::new(),
            enabled: config.enable_compression,
            detected_gzip: None,
            decoder: None,
        }
    }

    /// Feed a raw (possibly gzip-compressed) data chunk into the decompressor.
    ///
    /// If decompression is enabled and the stream is gzip-encoded, the bytes
    /// are fed to the persistent decoder and the decompressed bytes produced
    /// from them are returned immediately (possibly empty when the decoder
    /// needs more input to make progress). If the stream is identity-encoded,
    /// the data passes through as-is. Otherwise, the raw data is returned
    /// as-is (passthrough mode).
    pub fn decompress_chunk(&mut self, data: &[u8]) -> Vec<u8> {
        if !self.enabled {
            return data.to_vec();
        }

        match self.detected_gzip {
            None => {
                if data.is_empty() {
                    return Vec::new();
                }
                self.buffer.extend_from_slice(data);
                if self.buffer.len() < 2 {
                    // Not enough bytes to decide yet — hold and wait.
                    return Vec::new();
                }
                let is_gzip = self.buffer[0] == GZIP_MAGIC[0] && self.buffer[1] == GZIP_MAGIC[1];
                self.detected_gzip = Some(is_gzip);
                if !is_gzip {
                    // Identity encoding: emit the held prefix and pass through
                    // the rest of the stream untouched.
                    return std::mem::take(&mut self.buffer);
                }
                // Gzip encoding: hand the held prefix to a persistent decoder
                // and start emitting decompressed bytes per chunk.
                let reader = IncrementalReader::new(std::mem::take(&mut self.buffer));
                self.decoder = Some(MultiGzDecoder::new(reader));
                self.pull_decoder()
            }
            Some(false) => data.to_vec(),
            Some(true) => {
                if let Some(decoder) = self.decoder.as_mut() {
                    decoder.get_mut().push(data);
                }
                self.pull_decoder()
            }
        }
    }

    /// Flush any remaining buffered data.
    ///
    /// If decompression is enabled and the stream is gzip, marks the
    /// compressed input as complete so the decoder can finalize the last
    /// member, then returns whatever remains. Otherwise, returns the raw
    /// buffer contents.
    pub fn flush(&mut self) -> Vec<u8> {
        match self.detected_gzip {
            Some(true) => {
                if let Some(decoder) = self.decoder.as_mut() {
                    decoder.get_mut().finish();
                }
                self.pull_decoder()
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
        self.decoder
            .as_ref()
            .map(|d| d.get_ref().buffered())
            .unwrap_or(self.buffer.len())
    }

    /// Pull decompressed bytes out of the persistent decoder while input is
    /// available. Returns whatever was produced in this call (possibly empty).
    ///
    /// Stops on `WouldBlock` (no more compressed input yet — resume on the
    /// next chunk or flush), on EOF, and on decode errors (truncated or
    /// corrupt stream: emit whatever was decoded rather than panicking).
    fn pull_decoder(&mut self) -> Vec<u8> {
        let Some(decoder) = self.decoder.as_mut() else {
            return Vec::new();
        };
        let mut output = Vec::new();
        let mut buf = [0u8; PULL_BUFFER_SIZE];
        loop {
            match decoder.read(&mut buf) {
                Ok(0) => break, // EOF: stream fully decoded.
                Ok(n) => {
                    output.extend_from_slice(&buf[..n]);
                    if output.len() > MAX_DECOMPRESSED_BYTES {
                        // Zip-bomb: truncate (rather than allocate unboundedly)
                        // and make the truncation visible instead of silently
                        // returning partial data.
                        tracing::warn!(
                            target: "sse_compressor",
                            bytes = output.len(),
                            "SSE gzip decompression exceeded {} byte cap — stream truncated",
                            MAX_DECOMPRESSED_BYTES
                        );
                        output.truncate(MAX_DECOMPRESSED_BYTES);
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // No more compressed input available yet — resume on the
                    // next chunk (or flush).
                    break;
                }
                Err(e) => {
                    tracing::warn!(
                        target: "sse_compressor",
                        error = %e,
                        "SSE gzip decode error, returning partial data"
                    );
                    break;
                }
            }
        }
        output
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
    fn decompresses_incrementally() {
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

        // A partial first chunk yields nothing (the decoder needs more input).
        let r1 = comp.decompress_chunk(&compressed[..5]);
        assert!(r1.is_empty());
        // The rest of the stream decompresses immediately, per chunk — it is
        // not held back until flush().
        let r2 = comp.decompress_chunk(&compressed[5..]);
        let output = String::from_utf8_lossy(&r2);
        assert!(output.contains("Hello world"));
        // Nothing remains buffered at stream end.
        assert!(comp.flush().is_empty());
        assert_eq!(comp.buffered_bytes(), 0);
    }

    #[test]
    fn chunk_emits_decompressed_output() {
        let config = StreamingConfig {
            enable_compression: true,
            compression_threshold: 1024,
        };
        let mut comp = SseDecompressor::new(&config);
        // Small gzip data: the whole stream fits in one chunk, so the output
        // is returned from decompress_chunk itself rather than flush().
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        let data = b"small data";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("write_all should succeed");
        let compressed = encoder.finish().expect("finish should succeed");

        let out = comp.decompress_chunk(&compressed);
        assert!(!out.is_empty());
        let output = String::from_utf8_lossy(&out);
        assert!(output.contains("small data"));
        assert!(comp.flush().is_empty());
        assert_eq!(comp.buffered_bytes(), 0);
    }

    #[test]
    fn streams_decompressed_output_across_chunks() {
        let config = StreamingConfig {
            enable_compression: true,
            compression_threshold: 1024,
        };
        let mut comp = SseDecompressor::new(&config);
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        // Large enough that the deflate encoder emits several blocks, so
        // decompressed output becomes available mid-stream.
        let original: Vec<u8> = (0..256 * 1024)
            .map(|i| b"lorem ipsum dolor sit amet, consectetur adipiscing elit. "[i % 56])
            .collect();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&original)
            .expect("write_all should succeed");
        let compressed = encoder.finish().expect("finish should succeed");

        // Feed the compressed stream in fixed-size pieces, as a network
        // response would deliver it.
        let mut decoded = Vec::new();
        let mut output_chunks = 0;
        for piece in compressed.chunks(1024) {
            let out = comp.decompress_chunk(piece);
            if !out.is_empty() {
                output_chunks += 1;
            }
            decoded.extend_from_slice(&out);
        }
        // Streaming: decompressed bytes flowed before flush()…
        assert!(output_chunks > 1, "expected output across multiple chunks");
        // …and nothing was left buffered at stream end.
        assert!(comp.flush().is_empty());
        assert_eq!(decoded, original);
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

        // Feed compressed data to the decompressor; the output arrives from
        // decompress_chunk immediately.
        let decompressed = comp.decompress_chunk(&compressed);
        assert_eq!(decompressed, original);
        assert!(comp.flush().is_empty());
    }
}
