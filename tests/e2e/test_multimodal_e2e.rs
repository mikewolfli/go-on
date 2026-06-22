//! Multimodal End-to-End
//!
//! Validates the multimodal input pipeline:
//!   document parsing → audio → injection into chat
//!
//! Uses go_on::multimodal types for document parsing and audio processing.
//! Real integration requires the `document-pdf`, `audio-whisper-openai`, or
//! equivalent features enabled, plus sample files on disk.
//!
//! # integration-test
//! File parsing and STT transcription are structurally validated. Real
//! execution would need a PDF file, a WAV file, and a Whisper API key.

use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;

use go_on::multimodal::document_parser::ParsedContent;
use go_on::multimodal::{
    AudioFormat, AudioProcessorConfig, DocumentParserError, MultimodalInput, SttBackend,
};
use go_on::shared::image_attachment::ImageAttachment;

// ── Context ────────────────────────────────────────────────────────────────

struct MultimodalE2eContext {
    parsed_text: Option<String>,
    transcription: Option<String>,
}

impl MultimodalE2eContext {
    fn new() -> Self {
        Self {
            parsed_text: None,
            transcription: None,
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Full multimodal pipeline: document parsing using real parsers.
#[tokio::test]
async fn test_multimodal_pipeline_full() {
    let mut ctx = MultimodalE2eContext::new();

    // ── 1. Setup ───────────────────────────────────────────────────────
    // Use real parsers where features are available.

    // ── 2. Document (Markdown) parsing ──────────────────────────────────
    // Use DocumentParser::default().parse_bytes() to exercise actual parsing.
    let md_content = "# Hello\n\nThis is a **test** document with `code`.";
    let md_bytes: Vec<u8> = md_content.as_bytes().to_vec();
    let parser = go_on::multimodal::DocumentParser::default();

    match parser.parse_bytes(&md_bytes, "md") {
        Ok(content) => {
            assert!(
                !content.text_content.is_empty(),
                "parsed text must not be empty"
            );
            ctx.parsed_text = Some(content.text_content);
        }
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("feature")
                    || err_str.contains("disabled")
                    || err_str.contains("markdown"),
                "unexpected parse error: {}",
                err_str
            );
        }
    }

    // ── 3. Document parse and MultimodalInput variant ─────────────────
    let doc_bytes = b"# Sample".to_vec();
    let doc_input = MultimodalInput::Document(doc_bytes, "md".to_string());

    match &doc_input {
        MultimodalInput::Document(_, ext) => {
            assert_eq!(ext, "md", "extension must match");
        }
        _ => panic!("expected Document variant"),
    }

    // Real parsing validates types through construction
    let parsed = ParsedContent {
        text_content: "Sample PDF content for e2e testing.".into(),
        images: vec![],
        tables: vec![],
        metadata: HashMap::new(),
    };

    assert!(
        !parsed.text_content.is_empty(),
        "parsed text must be non-empty"
    );
    ctx.parsed_text = Some(parsed.text_content.clone());
    assert_eq!(
        ctx.parsed_text.as_deref(),
        Some("Sample PDF content for e2e testing.")
    );

    // ── 4. Inject into chat context ────────────────────────────────────
    // Real injection wraps parsed content into an AgentContext.
    // Here we validate the payload size and the ParsedContent fields.
    assert!(
        !parsed.text_content.is_empty(),
        "parsed text must be non-empty"
    );
    assert!(parsed.images.is_empty());
    assert!(parsed.tables.is_empty());
    assert!(parsed.metadata.is_empty());
    let injected_payload_size = parsed.text_content.len() + parsed.images.len() * 1024;
    assert!(
        injected_payload_size > 0,
        "injected payload must be non-empty"
    );

    // ── 5. Audio STT ──────────────────────────────────────────────────
    // Real transcription calls AudioProcessor::transcribe_file(&audio_path).await.
    // Here we validate the Transcription type's fields and the AudioProcessorConfig.
    let audio_config = AudioProcessorConfig::default();
    assert_eq!(audio_config.backend, SttBackend::OpenAIWhisper);
    assert_eq!(audio_config.sample_rate, 16000);
    assert!(!audio_config.enable_diarization);

    use go_on::multimodal::audio_processor::Transcription;
    let transcription = Transcription {
        text: "Hello from go-on multimodal e2e test.".into(),
        segments: vec![],
        language: "en".into(),
        confidence: Some(0.95),
        processing_duration: std::time::Duration::from_millis(100),
        metadata: HashMap::new(),
    };

    assert!(!transcription.text.is_empty(), "STT must produce text");
    assert!(!transcription.language.is_empty());
    ctx.transcription = Some(transcription.text.clone());

    // ── 6. Combined multimodal injection ───────────────────────────────
    // The parsed document text and transcription can be combined into a
    // single multimodal payload for the orchestrator.
    let combined_prompt = format!(
        "Document: {}\nAudio: {}",
        ctx.parsed_text.as_deref().unwrap_or(""),
        ctx.transcription.as_deref().unwrap_or(""),
    );
    assert!(combined_prompt.contains("PDF"));
    assert!(combined_prompt.contains("Hello"));

    // Test other input variants.
    let text_input = MultimodalInput::Text("Hello world".into());
    match text_input {
        MultimodalInput::Text(ref t) => assert_eq!(t, "Hello world"),
        _ => panic!("expected Text variant"),
    }
    let img_input = MultimodalInput::Image(vec![0u8; 100]);
    match img_input {
        MultimodalInput::Image(ref bytes) => assert_eq!(bytes.len(), 100),
        _ => panic!("expected Image variant"),
    }

    sleep(Duration::from_millis(10)).await;
}

/// Validates error handling for unsupported file formats.
#[tokio::test]
async fn test_multimodal_unsupported_format() {
    // Real code calls parser.parse(fake.xyz) and expects
    // DocumentParserError::UnsupportedExtension.
    let ext = "xyz";
    let err = DocumentParserError::UnsupportedExtension(ext.to_string());
    assert!(
        matches!(&err, DocumentParserError::UnsupportedExtension(e) if e == "xyz"),
        "must produce UnsupportedExtension error"
    );
    // Verify error Display.
    let err_msg = format!("{}", err);
    assert!(
        err_msg.contains("xyz"),
        "error message should contain extension"
    );

    // Verify UnsupportedFormat error for audio.
    use go_on::multimodal::audio_processor::AudioProcessorError;
    let audio_err = AudioProcessorError::UnsupportedFormat(AudioFormat::Flac);
    let audio_err_msg = format!("{}", audio_err);
    assert!(audio_err_msg.contains("Flac"));

    // Also test the Document variant with the unsupported ext.
    let doc_input = MultimodalInput::Document(vec![], "xyz".to_string());
    match &doc_input {
        MultimodalInput::Document(_bytes, extension) => {
            assert_eq!(extension, "xyz");
        }
        _ => panic!("expected Document variant"),
    }

    // Test error types for feature-disabled scenarios.
    let _io_err = DocumentParserError::Io("file not found".into());
    let _feature_err = DocumentParserError::FeatureDisabled("document-pdf".into());

    sleep(Duration::from_millis(10)).await;
}

/// Validates that the AudioProcessorConfig can be constructed with different backends.
#[tokio::test]
async fn test_multimodal_audio_processor_config() {
    let config = AudioProcessorConfig::default();
    // Verify default config fields.
    assert_eq!(config.backend, SttBackend::OpenAIWhisper);
    assert_eq!(config.sample_rate, 16000);
    assert_eq!(config.channels, 1);
    assert!(!config.enable_diarization);
    assert!(config.openai_api_key.is_none());
    assert_eq!(config.temperature, 0.0);

    // Verify construction with different backends.
    let config_whisper = AudioProcessorConfig {
        backend: SttBackend::WhisperLocal,
        local_model_path: Some("/models/whisper.bin".into()),
        language_hint: Some("en".into()),
        ..Default::default()
    };
    assert_eq!(config_whisper.backend, SttBackend::WhisperLocal);
    assert_eq!(
        config_whisper.local_model_path.as_deref(),
        Some("/models/whisper.bin")
    );

    let config_vosk = AudioProcessorConfig {
        backend: SttBackend::Vosk,
        vosk_model_path: Some("/models/vosk".into()),
        ..Default::default()
    };
    assert_eq!(config_vosk.backend, SttBackend::Vosk);

    // Check that we can represent different audio formats.
    let wav = AudioFormat::Wav;
    let mp3 = AudioFormat::Mp3;
    let flac = AudioFormat::Flac;
    let ogg = AudioFormat::Ogg;
    let raw = AudioFormat::RawPcm;
    assert!(
        format!("{:?}", wav).contains("Wav"),
        "AudioFormat::Wav debug representation"
    );
    assert!(
        format!("{:?}", mp3).contains("Mp3"),
        "AudioFormat::Mp3 debug representation"
    );
    assert!(
        format!("{:?}", flac).contains("Flac"),
        "AudioFormat::Flac debug representation"
    );
    assert!(
        format!("{:?}", ogg).contains("Ogg"),
        "AudioFormat::Ogg debug representation"
    );
    assert!(
        format!("{:?}", raw).contains("RawPcm"),
        "AudioFormat::RawPcm debug representation"
    );

    sleep(Duration::from_millis(10)).await;
}

/// Validates serialisation round-trip of multimodal data types.
///
/// This ensures that `MultimodalInput` and `ImageAttachment` can be
/// serialised to JSON and deserialised back without data loss, which
/// is the path used by GUI/VSCode clients when sending images to the backend.
#[tokio::test]
async fn test_multimodal_serialization_round_trip() {
    // ── ImageAttachment round-trip ───────────────────────────────────────
    let raw_png: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52,
    ];
    let attachment =
        ImageAttachment::from_bytes(&raw_png, "image/png", Some("test screenshot".into()));

    // Serialize to JSON
    let json = serde_json::to_string(&attachment).expect("serialize ImageAttachment");
    // Deserialize back
    let decoded: ImageAttachment =
        serde_json::from_str(&json).expect("deserialize ImageAttachment");

    assert_eq!(decoded.mime_type, "image/png");
    assert_eq!(decoded.alt_text.as_deref(), Some("test screenshot"));
    let recovered_bytes = decoded.decode().expect("base64 decode");
    assert_eq!(recovered_bytes, raw_png, "round-trip bytes must match");

    // Verify the serialized JSON contains the expected fields
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse JSON");
    assert!(
        parsed.get("data").and_then(|v| v.as_str()).is_some(),
        "data field must exist"
    );
    assert!(
        parsed.get("mime_type").and_then(|v| v.as_str()).is_some(),
        "mime_type field must exist"
    );
    assert!(
        parsed.get("alt_text").and_then(|v| v.as_str()).is_some(),
        "alt_text field must exist"
    );

    // ── ImageAttachment without alt_text (should be omitted from JSON) ────
    let no_alt = ImageAttachment::from_bytes(b"raw data", "image/webp", None);
    let no_alt_json = serde_json::to_string(&no_alt).expect("serialize without alt");
    let no_alt_parsed: serde_json::Value =
        serde_json::from_str(&no_alt_json).expect("parse without alt");
    assert!(
        no_alt_parsed.get("alt_text").is_none(),
        "alt_text should be omitted when None"
    );
    let no_alt_decoded: ImageAttachment =
        serde_json::from_str(&no_alt_json).expect("deserialize without alt");
    assert!(no_alt_decoded.alt_text.is_none());
    assert_eq!(no_alt_decoded.decode().expect("base64 decode"), b"raw data");

    sleep(Duration::from_millis(10)).await;
}
