//! Multimodal input module — unified types and processors for handling text, image,
//! audio, video, and document inputs within the go-on agent orchestration runtime.
//!
//! # Sub-modules
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`document_parser`] | Extracts text, images, tables, and metadata from PDF, DOCX, HTML, Markdown, Excel, PPT |
//! | `excel_processor` (feature `document-excel`) | Excel (.xlsx / .xls) workbook text extraction using `calamine` |
//! | `ppt_processor` (feature `document-ppt`) | PowerPoint (.pptx) slide text extraction using `quick-xml` |
//! | [`audio_processor`] | Speech-to-text transcription with speaker diarization support |
//!
//! # Re-exports
//!
//! Convenience re-exports of the principal types that the chat pipeline and
//! integration tests import directly from `go_on::multimodal`. The full type
//! surface remains available under each sub-module (e.g.
//! `go_on::multimodal::document_parser::ParsedContent`).

pub mod audio_processor;
pub mod code_repo_analyzer;
pub mod document_parser;
pub mod video_processor;

#[cfg(feature = "document-excel")]
pub mod excel_processor;
#[cfg(feature = "document-excel-write")]
pub mod excel_writer;
#[cfg(feature = "document-ppt")]
pub mod ppt_processor;

// Internal use of sub-module types (previously brought into scope by the
// removed re-exports; now explicit so the imports stay visible).
use code_repo_analyzer::RepoAnalyzer;
use video_processor::{VideoProcessor, VideoProcessorConfig};

// ── Re-exports from document_parser ────────────────────────────────────────
#[allow(unused_imports)] // public API surface — used by external consumers
pub use document_parser::DocumentParser;
#[allow(unused_imports)]
pub use document_parser::DocumentParserError;

// ── Re-exports from audio_processor ────────────────────────────────────────
#[allow(unused_imports)] // public API surface — used by external consumers
pub use audio_processor::AudioFormat;
#[allow(unused_imports)]
pub use audio_processor::AudioProcessor;
#[allow(unused_imports)]
pub use audio_processor::AudioProcessorConfig;
#[allow(unused_imports)]
pub use audio_processor::SttBackend;

// ── Re-exports from code_repo_analyzer ──────────────────────────────────────
#[allow(unused_imports)] // public API surface — used by external consumers
pub use code_repo_analyzer::REPO_PREFIX;

/// Represents a multimodal input payload that can be routed to an appropriate
/// processor (document parser, ASR pipeline, vision model, etc.).
///
/// Maximum allowed size for image payloads (10 MB). Enforced by the chat
/// pipeline's inline-`data:`-URI and `file://` extraction paths.
pub const MAX_IMAGE_SIZE: usize = 10 * 1024 * 1024;
/// Maximum allowed size for audio payloads (25 MB). Enforced by the chat
/// pipeline's inline-`data:`-URI and `file://` extraction paths.
pub const MAX_AUDIO_SIZE: usize = 25 * 1024 * 1024;
/// Maximum allowed size for document payloads (50 MB) — matches the cap
/// enforced by `DocumentParser::parse` on the path-based API.
pub const MAX_DOCUMENT_SIZE: usize = 50 * 1024 * 1024;
/// Maximum allowed size for video payloads (500 MB) — matches
/// `video_processor::MAX_FILE_SIZE_MB`.
pub const MAX_VIDEO_SIZE: usize = video_processor::MAX_FILE_SIZE_MB as usize * 1024 * 1024;

/// ## Variants
///
/// | Variant | Contents | Typical use |
/// |---------|----------|-------------|
/// | `Text` | UTF-8 string | Direct LLM prompt, chat message |
/// | `Image` | Raw image bytes (PNG, JPEG, WebP, etc.) | Vision model input |
/// | `Audio` | Raw audio bytes (WAV, MP3, FLAC, etc.) | ASR / STT pipeline |
/// | `Video` | Raw video bytes (MP4, AVI, etc.) | Video analysis pipeline |
/// | `Document` | Raw bytes + file extension | Document parsing pipeline |
#[derive(Debug, Clone)]
pub enum MultimodalInput {
    /// Plain text content.
    Text(String),
    /// Raw encoded image bytes (PNG, JPEG, WebP, etc.).
    Image(Vec<u8>),
    /// Raw encoded audio bytes (WAV, MP3, FLAC, etc.).
    Audio(Vec<u8>),
    /// Raw encoded video bytes (MP4, AVI, MKV, etc.).
    Video(Vec<u8>),
    /// Raw document bytes accompanied by a file-extension hint (e.g. `"pdf"`, `"docx"`).
    Document(Vec<u8>, String),
}

impl MultimodalInput {
    /// Returns a human-readable modality label.
    ///
    /// ```
    /// # use go_on::multimodal::MultimodalInput;
    /// assert_eq!(MultimodalInput::Text("hi".into()).modality(), "text");
    /// assert_eq!(MultimodalInput::Image(vec![0u8; 4]).modality(), "image");
    /// ```
    pub fn modality(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Image(_) => "image",
            Self::Audio(_) => "audio",
            Self::Video(_) => "video",
            Self::Document(_, _) => "document",
        }
    }

    /// Returns the approximate size in bytes of the contained payload.
    pub fn byte_size(&self) -> usize {
        match self {
            Self::Text(t) => t.len(),
            Self::Image(b) | Self::Audio(b) | Self::Video(b) | Self::Document(b, _) => b.len(),
        }
    }

    /// For `Document` variants, returns the file-extension hint (lowercased).
    pub fn document_extension(&self) -> Option<&str> {
        match self {
            Self::Document(_, ext) => Some(ext.as_str()),
            _ => None,
        }
    }

    /// Returns `true` if this input is a document type.
    pub fn is_document(&self) -> bool {
        matches!(self, Self::Document(_, _))
    }

    /// Returns `true` if this input is audio.
    pub fn is_audio(&self) -> bool {
        matches!(self, Self::Audio(_))
    }

    /// Returns `true` if this input is an image.
    pub fn is_image(&self) -> bool {
        matches!(self, Self::Image(_))
    }

    /// Returns `true` if this input is video.
    pub fn is_video(&self) -> bool {
        matches!(self, Self::Video(_))
    }

    /// Returns `true` if this input is plain text.
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }

    /// Provide the inner bytes (or string bytes) as a `&[u8]` slice, regardless
    /// of variant. Useful for hashing, serialisation, or size checks.
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Text(t) => t.as_bytes(),
            Self::Image(b) | Self::Audio(b) | Self::Video(b) | Self::Document(b, _) => b.as_slice(),
        }
    }

    /// Consume `self` and return the contained bytes together with any
    /// extension hint. Text is converted into UTF-8 bytes.
    pub fn into_bytes(self) -> (Vec<u8>, Option<String>) {
        match self {
            Self::Text(t) => (t.into_bytes(), None),
            Self::Image(b) => (b, None),
            Self::Audio(b) => (b, None),
            Self::Video(b) => (b, None),
            Self::Document(b, ext) => (b, Some(ext)),
        }
    }
}

// ---------------------------------------------------------------------------
// Conversions from common types
// ---------------------------------------------------------------------------

impl From<String> for MultimodalInput {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for MultimodalInput {
    fn from(s: &str) -> Self {
        Self::Text(s.to_owned())
    }
}

impl From<Vec<u8>> for MultimodalInput {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Image(bytes)
    }
}

// ── MultimodalProcessor orchestrator ───────────────────────────────────────

/// Unified result produced by processing any multimodal input.
///
/// This is consumed by the chat pipeline as the primary source of enriched
/// context. If no processing was needed, `text` carries the original input
/// verbatim.
#[derive(Debug, Clone, Default)]
pub struct ProcessedContent {
    /// The primary text extracted / transcribed / forwarded.
    pub text: String,
    /// Base64-encoded image data (PNG/JPEG) that should be injected into
    /// the LLM's vision context.
    pub images: Vec<String>,
    /// Audio transcription segments (aggregated into text for downstream).
    pub audio_transcriptions: Vec<String>,
}

impl ProcessedContent {
    /// True when absolutely nothing was produced (no text, no images, no audio).
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.images.is_empty() && self.audio_transcriptions.is_empty()
    }

    /// Returns the joined transcription text (empty string if none).
    pub fn joined_audio(&self) -> String {
        self.audio_transcriptions.join("\n")
    }
}

/// Central orchestrator for multimodal processing.
///
/// Holds sub-processors as `Option` so that each one can be omitted when its
/// backend feature is disabled. When a processor is `None`, the corresponding
/// input modality is passed through as plain text (or skipped entirely for
/// binary formats).
#[derive(Debug, Default)]
pub struct MultimodalProcessor {
    /// Document parser (PDF, DOCX, HTML, Markdown).
    pub document_parser: Option<DocumentParser>,
    /// Audio / speech-to-text processor.
    pub audio_processor: Option<AudioProcessor>,
    /// Video frame / scene extractor.
    pub video_processor: Option<VideoProcessor>,
    /// Code repository analyzer.
    pub repo_analyzer: Option<RepoAnalyzer>,
}

impl MultimodalProcessor {
    /// Create a new `MultimodalProcessor` with all sub-processors disabled
    /// (i.e. `None`). Use the builder-style setters to enable them.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a `MultimodalProcessor` with all sub-processors enabled using
    /// their default configuration.
    ///
    /// This is the recommended constructor when the multimodal feature set is
    /// active (feature `sub-bus-multimodal` or `full`).
    pub fn new_with_all_processors() -> Self {
        Self {
            document_parser: Some(DocumentParser::default()),
            audio_processor: Some(AudioProcessor::new(AudioProcessorConfig::default())),
            video_processor: Some(VideoProcessor::new(VideoProcessorConfig::default())),
            repo_analyzer: Some(RepoAnalyzer::default()),
        }
    }

    /// Convenience constructor that wires a pre-built `DocumentParser`.
    pub fn with_document_parser(mut self, parser: DocumentParser) -> Self {
        self.document_parser = Some(parser);
        self
    }

    /// Convenience constructor that wires a pre-built `AudioProcessor`.
    pub fn with_audio_processor(mut self, processor: AudioProcessor) -> Self {
        self.audio_processor = Some(processor);
        self
    }

    /// Convenience constructor that wires a pre-built `VideoProcessor`.
    pub fn with_video_processor(mut self, processor: VideoProcessor) -> Self {
        self.video_processor = Some(processor);
        self
    }

    /// Convenience constructor that wires a pre-built `RepoAnalyzer`.
    pub fn with_repo_analyzer(mut self, analyzer: RepoAnalyzer) -> Self {
        self.repo_analyzer = Some(analyzer);
        self
    }

    /// Returns `true` if any sub-processor is configured.
    pub fn is_configured(&self) -> bool {
        self.document_parser.is_some()
            || self.audio_processor.is_some()
            || self.video_processor.is_some()
            || self.repo_analyzer.is_some()
    }

    /// Route a single multimodal input to the appropriate sub-processor.
    ///
    /// When the relevant processor is `None` (or the modality is plain text
    /// without special prefixes), the input is passed through as-is in
    /// `ProcessedContent.text`.
    pub async fn process_input(&self, input: &MultimodalInput) -> ProcessedContent {
        match input {
            MultimodalInput::Text(text) => self.process_text(text).await,
            MultimodalInput::Image(data) => self.process_image(data).await,
            MultimodalInput::Audio(data) => self.process_audio(data).await,
            MultimodalInput::Video(data) => self.process_video(data).await,
            MultimodalInput::Document(data, ext) => self.process_document(data, ext).await,
        }
    }

    /// Process a text input — checks for `repo:` prefix and delegates to
    /// the `RepoAnalyzer` when present.
    async fn process_text(&self, text: &str) -> ProcessedContent {
        if RepoAnalyzer::has_repo_prefix(text) {
            if let Some(ref analyzer) = self.repo_analyzer {
                if let Some((url, question)) = RepoAnalyzer::parse_repo_input(text) {
                    tracing::info!(
                        repo_url = %url,
                        question = %question,
                        "MultimodalProcessor: delegating to RepoAnalyzer"
                    );
                    match analyzer.clone(&url).await {
                        Ok(repo) => {
                            let question = if question.is_empty() {
                                "Describe this repository.".to_string()
                            } else {
                                question
                            };
                            match analyzer.answer_code_question(&question, &repo).await {
                                Ok(answer) => {
                                    return ProcessedContent {
                                        text: answer.text,
                                        images: Vec::new(),
                                        audio_transcriptions: Vec::new(),
                                    };
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "MultimodalProcessor: RepoAnalyzer failed to answer"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "MultimodalProcessor: RepoAnalyzer failed to clone repo"
                            );
                        }
                    }
                }
            }
        }
        // Fall through — return the original text.
        ProcessedContent {
            text: text.to_owned(),
            images: Vec::new(),
            audio_transcriptions: Vec::new(),
        }
    }

    /// Process an image input — base64-encodes the raw bytes and stores them
    /// in `images` for downstream vision-model injection.
    async fn process_image(&self, data: &[u8]) -> ProcessedContent {
        let b64 = base64_encode(data);
        ProcessedContent {
            text: String::new(),
            images: vec![b64],
            audio_transcriptions: Vec::new(),
        }
    }

    /// Process an audio input — delegates to the `AudioProcessor` when
    /// configured, otherwise returns an empty result.
    async fn process_audio(&self, data: &[u8]) -> ProcessedContent {
        if let Some(ref processor) = self.audio_processor {
            let format = detect_audio_format(data);
            // AudioProcessor::transcribe is blocking I/O — offload via spawn_blocking.
            let data_owned = data.to_vec();
            let processor_clone = processor.clone();
            match tokio::task::spawn_blocking(move || {
                processor_clone.transcribe(&data_owned, format)
            })
            .await
            {
                Ok(Ok(transcription)) => {
                    return ProcessedContent {
                        text: transcription.text.clone(),
                        images: Vec::new(),
                        audio_transcriptions: vec![transcription.text],
                    };
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        error = %e,
                        "MultimodalProcessor: AudioProcessor failed to transcribe"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "MultimodalProcessor: spawn_blocking for audio transcription failed"
                    );
                }
            }
        }
        ProcessedContent {
            text: String::new(),
            images: Vec::new(),
            audio_transcriptions: Vec::new(),
        }
    }

    /// Detect video format from magic bytes and return the appropriate file extension.
    fn detect_video_ext(data: &[u8]) -> &'static str {
        if data.len() < 12 {
            return "mp4"; // fallback
        }
        // EBML header (MKV / WebM): 0x1A 0x45 0xDF 0xA3
        if data[0] == 0x1A && data[1] == 0x45 && data[2] == 0xDF && data[3] == 0xA3 {
            return "mkv";
        }
        // RIFF header (AVI): bytes 0-3 "RIFF", bytes 8-11 "AVI "
        if data[0] == 0x52
            && data[1] == 0x49
            && data[2] == 0x46
            && data[3] == 0x46
            && data[8] == 0x41
            && data[9] == 0x56
            && data[10] == 0x49
            && data[11] == 0x20
        {
            return "avi";
        }
        // ftyp box (MP4 / MOV / M4V): bytes 4-7 "ftyp"
        if data[4] == 0x66 && data[5] == 0x74 && data[6] == 0x79 && data[7] == 0x70 {
            // Check for QuickTime brand at bytes 8-11: "qt  "
            if data.len() >= 12
                && data[8] == 0x71
                && data[9] == 0x74
                && data[10] == 0x20
                && data[11] == 0x20
            {
                return "mov";
            }
            return "mp4";
        }
        // Fallback to mp4 (widest compatibility).
        "mp4"
    }

    /// Process a video input — delegates to the `VideoProcessor` when
    /// configured, otherwise returns an empty result.
    ///
    /// Raw video bytes are written to a temporary file (with the correct
    /// extension detected from magic bytes) so the `VideoProcessor` can
    /// read them (its API requires a `Path`).
    async fn process_video(&self, data: &[u8]) -> ProcessedContent {
        if let Some(ref processor) = self.video_processor {
            // Enforce the video size cap BEFORE writing the payload to a temp
            // file (the VideoProcessor re-checks after reading, but the whole
            // file must not be materialized on disk first).
            if data.len() as u64 > video_processor::MAX_FILE_SIZE_MB * 1024 * 1024 {
                tracing::warn!(
                    len = data.len(),
                    "MultimodalProcessor: video payload exceeds the size limit; skipping"
                );
                return ProcessedContent {
                    text: String::new(),
                    images: Vec::new(),
                    audio_transcriptions: Vec::new(),
                };
            }
            // Write bytes to a temp file because VideoProcessor works with paths.
            let tmp_dir = match tempfile::TempDir::new() {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "MultimodalProcessor: failed to create temp dir for video"
                    );
                    return ProcessedContent {
                        text: String::new(),
                        images: Vec::new(),
                        audio_transcriptions: Vec::new(),
                    };
                }
            };
            let ext = Self::detect_video_ext(data);
            let video_path = tmp_dir.path().join(format!("input.{ext}"));
            if let Err(e) = tokio::fs::write(&video_path, data).await {
                tracing::warn!(
                    error = %e,
                    "MultimodalProcessor: failed to write video temp file"
                );
                return ProcessedContent {
                    text: String::new(),
                    images: Vec::new(),
                    audio_transcriptions: Vec::new(),
                };
            }

            let interval = processor.config().frame_interval_secs;
            match processor.process_full(&video_path, interval).await {
                Ok(full_result) => {
                    let text = full_result
                        .scenes
                        .iter()
                        .map(|s| {
                            format!(
                                "[{}s-{}s] {} (conf: {:.2})",
                                s.start_sec, s.end_sec, s.label, s.confidence
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let images: Vec<String> = full_result
                        .frames
                        .iter()
                        .map(|f| base64_encode(&f.data))
                        .collect();
                    return ProcessedContent {
                        text,
                        images,
                        audio_transcriptions: Vec::new(),
                    };
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "MultimodalProcessor: VideoProcessor failed to process video"
                    );
                }
            }
        }
        ProcessedContent {
            text: String::new(),
            images: Vec::new(),
            audio_transcriptions: Vec::new(),
        }
    }

    /// Process a document input — delegates to the `DocumentParser` when
    /// configured.
    async fn process_document(&self, data: &[u8], ext: &str) -> ProcessedContent {
        if let Some(ref parser) = self.document_parser {
            // Document parsing (PDF/DOCX/HTML/…) is blocking CPU work — offload
            // via spawn_blocking so the tokio worker isn't stalled, mirroring
            // the audio transcription path above.
            let data_owned = data.to_vec();
            let ext_owned = ext.to_string();
            let parser_clone = parser.clone();
            match tokio::task::spawn_blocking(move || {
                parser_clone.parse_bytes(&data_owned, &ext_owned)
            })
            .await
            {
                Ok(Ok(parsed)) => {
                    // `parsed.images` already holds base64-encoded strings
                    // (document_parser encodes embedded images); passing them
                    // through here unchanged avoids double encoding.
                    let images = parsed.images.clone();
                    return ProcessedContent {
                        text: parsed.text_content,
                        images,
                        audio_transcriptions: Vec::new(),
                    };
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        error = %e,
                        "MultimodalProcessor: DocumentParser failed to parse"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "MultimodalProcessor: spawn_blocking for document parsing failed"
                    );
                }
            }
        }
        ProcessedContent {
            text: String::new(),
            images: Vec::new(),
            audio_transcriptions: Vec::new(),
        }
    }
}

// ProcessedContent and MultimodalProcessor are defined publicly above;
// no separate `pub use` needed since they're already `pub struct`.

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Detect audio format from magic bytes for broader compatibility
/// with common audio codecs.
fn detect_audio_format(bytes: &[u8]) -> crate::multimodal::audio_processor::AudioFormat {
    use crate::multimodal::audio_processor::AudioFormat;
    if bytes.len() < 12 {
        return AudioFormat::Wav; // fallback (RIFF header needs at least 12 bytes)
    }
    if &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        AudioFormat::Wav
    } else if &bytes[0..4] == b"fLaC" {
        AudioFormat::Flac
    } else if &bytes[0..3] == b"ID3" || (bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0) {
        AudioFormat::Mp3
    } else if &bytes[0..4] == b"OggS" {
        AudioFormat::Ogg
    } else {
        AudioFormat::Wav // fallback
    }
}

/// Decode a base64 string (standard or URL-safe) into raw bytes.
pub fn base64_decode(input: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    // Try standard padding first, then URL-safe.
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(input))
}

/// Base64-encode raw bytes (standard alphabet).
pub fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Map a MIME type string to a file extension for document processing.
///
/// Falls back to `"bin"` when the MIME type is unrecognised.
pub fn mime_to_extension(mime: &str) -> String {
    match mime {
        // Specific text subtypes must be checked BEFORE the catch-all m.contains("text").
        m if m.contains("pdf") => "pdf".to_string(),
        m if m.contains("word") || m.contains("docx") || m.contains("doc") => "docx".to_string(),
        m if m.contains("html") || m.contains("xhtml") => "html".to_string(),
        m if m.contains("markdown") || m.contains("md") => "md".to_string(),
        m if m.contains("json") => "json".to_string(),
        m if m.contains("csv") => "csv".to_string(),
        m if m.contains("xml") => "xml".to_string(),
        m if m.contains("yaml") || m.contains("yml") => "yaml".to_string(),
        // Catch-all text match — keep last so more-specific text/* subtypes
        // (html, markdown, json, csv, xml) are matched above first.
        m if m.contains("text/plain") || m.contains("text") => "txt".to_string(),
        _ => "bin".to_string(),
    }
}
