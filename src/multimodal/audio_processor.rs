//! Audio processor — speech-to-text (STT) transcription with speaker diarization
//! support.
//!
//! # Architecture
//!
//! The [`AudioProcessor`] dispatches to one of several backends selected via
//! the [`SttBackend`] enum:
//!
//! | Backend | Feature | Description |
//! |---------|---------|-------------|
//! | `OpenAIWhisper` | (always available) | Remote OpenAI Whisper REST API |
//!
//! Local backends (`WhisperLocal`, `Vosk`) were removed as placeholder
//! implementations that never ran real inference.
//!
//! # Error handling
//!
//! All transcription methods return an [`AudioProcessorError`] on failure,
//! distinguishing config errors, API errors, and feature-gate errors.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Per-request timeout for OpenAI Whisper transcription (120s — long audio
/// takes a while; a stuck upstream must still not hang the pipeline).
const AUDIO_TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during audio transcription.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AudioProcessorError {
    /// The required API key was not provided.
    #[error("missing API key: {0}")]
    MissingApiKey(String),

    /// The HTTP request to the remote API failed.
    #[error("HTTP request failed: {0}")]
    HttpRequest(String),

    /// The remote API returned an error response.
    #[error("API error (status {status}): {body}")]
    ApiError {
        /// HTTP status code.
        status: u16,
        /// Response body (truncated).
        body: String,
    },

    /// The response from the API could not be parsed.
    #[error("response parse error: {0}")]
    ResponseParse(String),

    /// A required Cargo feature is not enabled.
    #[error("feature not enabled: {0}")]
    FeatureDisabled(String),

    /// I/O error (file read, etc.).
    #[error("I/O error: {0}")]
    Io(String),

    /// Audio format is not supported by the selected backend.
    #[error("unsupported audio format: {0:?}")]
    UnsupportedFormat(AudioFormat),

    /// Backend runtime error.
    #[error("backend error: {0}")]
    Backend(String),

    /// Generic/internal error.
    #[error("{0}")]
    Other(String),
}

impl AudioProcessorError {
    /// Create a `FeatureDisabled` error with a descriptive message.
    pub fn feature_disabled(name: &str) -> Self {
        Self::FeatureDisabled(format!(
            "{} transcription requires the corresponding Cargo feature to be enabled",
            name
        ))
    }

    /// Create an `Io` error from a `std::io::Error`.
    pub fn from_io(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Supported audio input formats for transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioFormat {
    /// WAV (PCM, typically 16-bit).
    Wav,
    /// MP3 (MPEG Audio Layer III).
    Mp3,
    /// FLAC (Free Lossless Audio Codec).
    Flac,
    /// Ogg Vorbis / Opus.
    Ogg,
    /// Raw PCM bytes (user must specify sample rate, channels, bit depth).
    RawPcm,
    /// Arbitrary / unknown format (processor will attempt auto-detection).
    Other,
}

impl AudioFormat {
    /// Infer the format from a file extension.
    pub fn from_extension(ext: &str) -> Self {
        match ext.trim().to_lowercase().as_str() {
            "wav" | "wave" => Self::Wav,
            "mp3" => Self::Mp3,
            "flac" => Self::Flac,
            "ogg" | "opus" => Self::Ogg,
            "pcm" | "raw" => Self::RawPcm,
            _ => Self::Other,
        }
    }

    /// Common MIME type for the format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
            Self::Flac => "audio/flac",
            Self::Ogg => "audio/ogg",
            Self::RawPcm => "audio/L16",
            Self::Other => "application/octet-stream",
        }
    }

    /// Returns an appropriate file name extension (without the dot).
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::RawPcm => "pcm",
            Self::Other => "bin",
        }
    }
}

/// A single transcribed time-aligned segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// Start time of the segment (seconds from beginning).
    pub start_sec: f64,
    /// End time of the segment (seconds from beginning).
    pub end_sec: f64,
    /// Transcribed text for this segment.
    pub text: String,
    /// Confidence score (0.0 – 1.0), if available.
    pub confidence: Option<f64>,
    /// Speaker label (e.g. `"SPEAKER_00"`, `"SPEAKER_01"`), if diarization was applied.
    pub speaker: Option<String>,
}

/// The full transcription result produced by an STT backend.
///
/// This type is `Serialize` + `Deserialize` for RPC injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcription {
    /// The entire transcribed text (concatenation of all segments).
    pub text: String,
    /// Time-aligned segments (may be empty if the backend does not provide them).
    #[serde(default)]
    pub segments: Vec<TranscriptSegment>,
    /// Detected or user-specified language (e.g. `"en"`, `"zh"`, `"fr"`).
    #[serde(default = "default_language")]
    pub language: String,
    /// Overall confidence estimate (0.0 – 1.0), if available.
    pub confidence: Option<f64>,
    /// Processing duration (end-to-end wall-clock time for the request).
    #[serde(skip)]
    pub processing_duration: Duration,
    /// Additional backend-specific metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

fn default_language() -> String {
    "en".to_string()
}

impl Transcription {
    /// Returns the word count of the transcribed text.
    pub fn word_count(&self) -> usize {
        self.text.split_whitespace().count()
    }

    /// Returns the error message from metadata, if any.
    pub fn error_message(&self) -> Option<&str> {
        self.metadata.get("error").map(|s| s.as_str())
    }
}

/// Supported speech-to-text backends.
///
/// - `OpenAIWhisper`: Fully production-ready. Calls the OpenAI Whisper REST API.
///   Always available (no feature gate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SttBackend {
    /// OpenAI Whisper API (`POST https://api.openai.com/v1/audio/transcriptions`).
    /// Fully implemented and production-ready.
    OpenAIWhisper,
}

impl SttBackend {
    /// Human-readable backend name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::OpenAIWhisper => "openai-whisper",
        }
    }
}

// ---------------------------------------------------------------------------
// AudioProcessor
// ---------------------------------------------------------------------------

/// Configuration for the audio processor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioProcessorConfig {
    /// Which STT backend to use.
    pub backend: SttBackend,
    /// Audio sample rate in Hz (e.g. 16000). Ignored for backends that
    /// auto-detect.
    pub sample_rate: u32,
    /// Number of audio channels (1 = mono, 2 = stereo).
    pub channels: u16,
    /// Whether to attempt speaker diarization (if the backend supports it).
    pub enable_diarization: bool,
    /// Maximum number of speakers to detect (if diarization is enabled).
    pub max_speakers: Option<usize>,
    /// OpenAI API key (only used for `OpenAIWhisper`).
    pub openai_api_key: Option<String>,
    /// OpenAI model name (e.g. `"whisper-1"`).
    pub openai_model: Option<String>,
    /// OpenAI-compatible base URL (only used for `OpenAIWhisper`). Defaults to
    /// `OPENAI_API_BASE` env, then `https://api.openai.com/v1` — same env the
    /// embedding provider honors, so proxy/gateway deployments can route both.
    pub openai_api_base: String,
    /// Language hint (ISO 639-1 code, e.g. `"en"`, `"fr"`). May be empty for
    /// auto-detection.
    pub language_hint: Option<String>,
    /// Optional prompt to guide the transcription (Whisper-style).
    pub prompt: Option<String>,
    /// Temperature for sampling (Whisper; default: 0.0).
    pub temperature: f64,
}

impl Default for AudioProcessorConfig {
    fn default() -> Self {
        Self {
            backend: SttBackend::OpenAIWhisper,
            sample_rate: 16000,
            channels: 1,
            enable_diarization: false,
            max_speakers: None,
            openai_api_key: None,
            openai_model: Some("whisper-1".to_string()),
            // Same default as the embedding provider's OpenAI base.
            openai_api_base: std::env::var("OPENAI_API_BASE").unwrap_or_else(|_| {
                crate::shared::http_client::OPENAI_DEFAULT_BASE_URL.to_string()
            }),
            language_hint: None,
            prompt: None,
            temperature: 0.0,
        }
    }
}

/// The main audio processor for speech-to-text transcription.
///
/// # Example
///
/// ```text
/// use go_on::multimodal::audio_processor::{
///     AudioProcessor, AudioProcessorConfig, AudioFormat, SttBackend,
/// };
///
/// let config = AudioProcessorConfig {
///     backend: SttBackend::OpenAIWhisper,
///     openai_api_key: Some("sk-...".into()),
///     ..Default::default()
/// };
/// let processor = AudioProcessor::new(config);
/// let audio = std::fs::read("speech.wav").unwrap();
/// let result = processor.transcribe(&audio, AudioFormat::Wav);
/// println!("{}", result.text);
/// ```
#[derive(Debug, Clone)]
pub struct AudioProcessor {
    config: AudioProcessorConfig,
}

impl AudioProcessor {
    /// Create a new audio processor with the given configuration.
    pub fn new(config: AudioProcessorConfig) -> Self {
        Self { config }
    }

    /// Returns a reference to the current configuration.
    pub fn config(&self) -> &AudioProcessorConfig {
        &self.config
    }

    /// Transcribe the provided audio bytes, returning a [`Transcription`] on
    /// success or an [`AudioProcessorError`] on failure.
    ///
    /// `audio` should contain the raw encoded bytes of the audio file (e.g.
    /// the contents of a `.wav` or `.mp3` file). The `format` hint helps the
    /// backend interpret the bytes correctly.
    pub fn transcribe(
        &self,
        audio: &[u8],
        format: AudioFormat,
    ) -> Result<Transcription, AudioProcessorError> {
        let start = std::time::Instant::now();

        let mut result = match self.config.backend {
            SttBackend::OpenAIWhisper => self.transcribe_openai_whisper(audio, format)?,
        };

        result.processing_duration = start.elapsed();

        // Diarization is requested in the SAME transcription call via the
        // `diarize_speaker_count` form field (see call_openai_whisper_api),
        // so speaker labels come back in the segments directly. Previously
        // this ran a second full transcription and zip-merged segments, which
        // doubled API cost and mismatched segment boundaries.
        if self.config.enable_diarization && result.segments.iter().all(|s| s.speaker.is_none()) {
            result.metadata.insert(
                "diarization".to_string(),
                "requested but no speaker labels returned by backend".to_string(),
            );
        }
        if self.config.enable_diarization {
            result.metadata.insert(
                "diarization_max_speakers".to_string(),
                self.config.max_speakers.unwrap_or(2).to_string(),
            );
        }

        result.metadata.insert(
            "backend".to_string(),
            self.config.backend.name().to_string(),
        );
        result
            .metadata
            .insert("format".to_string(), format!("{:?}", format));

        Ok(result)
    }

    // -----------------------------------------------------------------------
    // OpenAI Whisper backend  (always available — uses `reqwest`)
    // -----------------------------------------------------------------------

    fn transcribe_openai_whisper(
        &self,
        audio: &[u8],
        format: AudioFormat,
    ) -> Result<Transcription, AudioProcessorError> {
        let api_key = self
            .config
            .openai_api_key
            .as_ref()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                AudioProcessorError::MissingApiKey(
                    "OpenAIWhisper requires `openai_api_key` to be set".to_string(),
                )
            })?;

        let model = self
            .config
            .openai_model
            .clone()
            .unwrap_or_else(|| "whisper-1".to_string());

        let mime = format.mime_type();

        call_openai_whisper_api(api_key, &model, audio, mime, &self.config)
    }

    // -----------------------------------------------------------------------
    // Speaker diarization
    // -----------------------------------------------------------------------

    // Diarization is handled inside `call_openai_whisper_api`: when
    // `enable_diarization` is set, the single transcription request includes
    // `diarize_speaker_count`, so speaker labels arrive in the segments.
    // The former `diarize_via_openai` second-transcription path was removed
    // (it doubled API cost and zip-merged misaligned segments).
}

// ---------------------------------------------------------------------------
// Standalone helper: convenience transcription without a full AudioProcessor
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Send a transcription request to the OpenAI Whisper REST API using
/// `reqwest::blocking`.
///
/// This function is defined unconditionally so the `OpenAIWhisper` backend
/// works without any feature flag (it only depends on `reqwest`, which is
/// already a hard dependency of the project).
fn call_openai_whisper_api(
    api_key: &str,
    model: &str,
    audio: &[u8],
    mime: &str,
    config: &AudioProcessorConfig,
) -> Result<Transcription, AudioProcessorError> {
    // Build the multipart form using the shared process-global blocking
    // client (previously every transcription built a fresh client, paying a
    // full connection/TLS setup per call). Per-request timeout is applied on
    // the request builder so long transcriptions are not cut off.
    let client = crate::shared::http_client::blocking_http_client()
        .map_err(|e| AudioProcessorError::HttpRequest(e.to_string()))?;

    // Determine the file extension from the MIME type for the form part.
    let file_ext = match mime {
        "audio/wav" => "wav",
        "audio/mpeg" => "mp3",
        "audio/flac" => "flac",
        "audio/ogg" => "ogg",
        "audio/L16" => "pcm",
        _ => "bin",
    };
    let part_file_name = format!("audio.{file_ext}");

    let part = reqwest::blocking::multipart::Part::bytes(audio.to_vec())
        .file_name(part_file_name)
        .mime_str(mime)
        .map_err(|e| AudioProcessorError::HttpRequest(e.to_string()))?;

    let mut form = reqwest::blocking::multipart::Form::new()
        .part("file", part)
        .text("model", model.to_string())
        .text("response_format", "verbose_json".to_string());

    if let Some(ref lang) = config.language_hint {
        form = form.text("language", lang.clone());
    }
    if let Some(ref prompt) = config.prompt {
        form = form.text("prompt", prompt.clone());
    }
    form = form.text("temperature", config.temperature.to_string());

    // Diarization: request speaker labels in the SAME call. The OpenAI
    // Whisper API returns speaker tags per segment when `diarize_speaker_count`
    // is present with `response_format=verbose_json`. Previously diarization
    // was a separate second transcription (2x cost) and `max_speakers` was
    // never actually sent to the API.
    if config.enable_diarization {
        let speaker_count = config.max_speakers.unwrap_or(2);
        form = form.text("diarize_speaker_count", speaker_count.to_string());
    }

    let resp = client
        .post(crate::shared::url_join::join_url(
            &config.openai_api_base,
            "audio/transcriptions",
        ))
        .header("Authorization", format!("Bearer {api_key}"))
        .multipart(form)
        .timeout(AUDIO_TRANSCRIBE_TIMEOUT)
        .send()
        .map_err(|e| AudioProcessorError::HttpRequest(e.to_string()))?;

    let status = resp.status();
    let body_bytes = resp
        .bytes()
        .map_err(|e| AudioProcessorError::HttpRequest(e.to_string()))?;

    if !status.is_success() {
        let body_str = String::from_utf8_lossy(&body_bytes).to_string();
        return Err(AudioProcessorError::ApiError {
            status: status.as_u16(),
            body: body_str,
        });
    }

    // Parse the JSON response.
    let json: serde_json::Value = serde_json::from_slice(&body_bytes)
        .map_err(|e| AudioProcessorError::ResponseParse(format!("invalid JSON: {e}")))?;

    let text = json["text"]
        .as_str()
        .unwrap_or_else(|| {
            tracing::warn!(
                "OpenAI Whisper API response missing 'text' field, falling back to empty string"
            );
            ""
        })
        .to_string();

    let language = json["language"]
        .as_str()
        .unwrap_or_else(|| {
            tracing::warn!(
                "OpenAI Whisper API response missing 'language' field, falling back to 'en'"
            );
            "en"
        })
        .to_string();

    // Parse segments if available.
    let mut segments: Vec<TranscriptSegment> = Vec::new();
    if let Some(segments_arr) = json["segments"].as_array() {
        for seg_val in segments_arr {
            let start = seg_val["start"].as_f64().unwrap_or_else(|| {
                tracing::warn!("OpenAI Whisper API segment missing 'start', falling back to 0.0");
                0.0
            });
            let end = seg_val["end"].as_f64().unwrap_or_else(|| {
                tracing::warn!("OpenAI Whisper API segment missing 'end', falling back to 0.0");
                0.0
            });
            let seg_text = seg_val["text"]
                .as_str()
                .unwrap_or_else(|| {
                    tracing::warn!(
                        "OpenAI Whisper API segment missing 'text', falling back to empty string"
                    );
                    ""
                })
                .to_string();
            let conf = seg_val["confidence"].as_f64();
            let speaker = seg_val["speaker"].as_str().map(|s| s.to_string());

            segments.push(TranscriptSegment {
                start_sec: start,
                end_sec: end,
                text: seg_text,
                confidence: conf,
                speaker,
            });
        }
    }

    // Compute overall confidence as the average of segment confidences.
    let overall_confidence = if segments.is_empty() {
        json["confidence"].as_f64()
    } else {
        let sum: f64 = segments.iter().filter_map(|s| s.confidence).sum();
        let count = segments.iter().filter(|s| s.confidence.is_some()).count();
        if count > 0 {
            Some(sum / count as f64)
        } else {
            None
        }
    };

    Ok(Transcription {
        text,
        segments,
        language,
        confidence: overall_confidence,
        processing_duration: Duration::default(),
        metadata: {
            let mut m = HashMap::new();
            m.insert("model".to_string(), model.to_string());
            if let Some(duration) = json["duration"].as_f64() {
                m.insert("duration_sec".to_string(), duration.to_string());
            }
            m
        },
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_whisper_missing_key() {
        let config = AudioProcessorConfig {
            backend: SttBackend::OpenAIWhisper,
            openai_api_key: None,
            ..Default::default()
        };
        let processor = AudioProcessor::new(config);
        let result = processor.transcribe(b"dummy", AudioFormat::Wav);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AudioProcessorError::MissingApiKey(_)
        ));
    }

    #[test]
    fn test_diarization_heuristic() {
        let processor = AudioProcessor::new(AudioProcessorConfig {
            enable_diarization: true,
            max_speakers: Some(2),
            ..Default::default()
        });
        // Feed through the OpenAI backend; it will fail with missing key,
        // but the error will be surfaced via transcribe, so this test checks
        // that the pipeline doesn't panic.
        let result = processor.transcribe(b"dummy", AudioFormat::Wav);
        assert!(result.is_err());
    }

    #[test]
    fn test_transcribe_convenience_fn() {
        let processor = AudioProcessor::new(AudioProcessorConfig {
            backend: SttBackend::OpenAIWhisper,
            ..Default::default()
        });
        let result = processor.transcribe(b"test", AudioFormat::Wav);
        // Will fail with missing API key; just check it returns an error.
        assert!(result.is_err());
    }
}
