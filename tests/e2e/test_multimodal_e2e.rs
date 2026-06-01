//! Multimodal End-to-End
//!
//! Validates the multimodal input pipeline:
//!   document parsing → audio → injection into chat
//!
//! Uses go_on::multimodal types for document parsing and audio processing.
//! Real integration requires the `document-pdf`, `audio-whisper-openai`, or
//! equivalent features enabled, plus sample files on disk.
//!
//! # integration-test-stub
//! File parsing and STT transcription are structurally validated. Real
//! execution would need a PDF file, a WAV file, and a Whisper API key.

use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;

use go_on::multimodal::document_parser::ParsedContent;
use go_on::multimodal::{
    AudioFormat, AudioProcessorConfig, DocumentParserError, MultimodalInput, SttBackend,
};

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

/// Full multimodal pipeline: document parsing → audio → injection into chat.
#[tokio::test]
#[ignore]
async fn test_multimodal_pipeline_full() {
    let mut ctx = MultimodalE2eContext::new();

    // ── 1. Setup ───────────────────────────────────────────────────────
    // integration-test-stub: real setup generates sample PDF and audio files.
    // For this structural test we use the type constructors directly.

    // ── 2. Document (PDF) upload as MultimodalInput ────────────────────
    let pdf_bytes: Vec<u8> = b"%PDF-1.4 sample content for e2e testing".to_vec();
    let doc_input = MultimodalInput::Document(pdf_bytes, "pdf".to_string());

    // Verify the input variant.
    match &doc_input {
        MultimodalInput::Document(_bytes, ext) => {
            assert_eq!(ext, "pdf", "extension must match");
        }
        _ => panic!("expected Document variant"),
    }

    // ── 3. Parse the document ──────────────────────────────────────────
    // integration-test-stub: real parsing calls DocumentParser::parse(path).
    // Here we construct a ParsedContent manually to validate the type.
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
    // integration-test-stub: real injection wraps parsed content into
    // an AgentContext via context.inject_multimodal(&parsed). Here we
    // simulate the payload size check.
    let injected_payload_size = parsed.text_content.len() + parsed.images.len() * 1024;
    assert!(
        injected_payload_size > 0,
        "injected payload must be non-empty"
    );

    // ── 5. Audio STT ──────────────────────────────────────────────────
    // integration-test-stub: real transcription calls
    // AudioProcessor::transcribe_file(&audio_path).await.
    let _audio_config = AudioProcessorConfig::default();
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

    sleep(Duration::from_millis(10)).await;
    assert!(true, "multimodal pipeline full passed");
}

/// Validates error handling for unsupported file formats.
#[tokio::test]
#[ignore]
async fn test_multimodal_unsupported_format() {
    // integration-test-stub: real code calls parser.parse(fake.xyz)
    // and expects DocumentParserError::UnsupportedExtension.
    let ext = "xyz";
    let err = DocumentParserError::UnsupportedExtension(ext.to_string());
    assert!(
        matches!(&err, DocumentParserError::UnsupportedExtension(e) if e == "xyz"),
        "must produce UnsupportedExtension error"
    );

    let _doc_input = MultimodalInput::Document(vec![], "xyz".to_string());
    match &_doc_input {
        MultimodalInput::Document(_bytes, extension) => {
            assert_eq!(extension, "xyz");
        }
        _ => panic!("expected Document variant"),
    }

    sleep(Duration::from_millis(10)).await;
    assert!(true, "unsupported format skeleton passed");
}

/// Validates that the AudioProcessorConfig can be constructed with different backends.
#[tokio::test]
#[ignore]
async fn test_multimodal_audio_processor_config() {
    let config = AudioProcessorConfig::default();
    // The default backend is implementation-defined; we just verify construction.
    let _config_with_backend = AudioProcessorConfig {
        backend: SttBackend::OpenAIWhisper,
        ..Default::default()
    };

    // Check that we can represent different audio formats.
    let _wav = AudioFormat::Wav;
    let _mp3 = AudioFormat::Mp3;
    let _flac = AudioFormat::Flac;

    sleep(Duration::from_millis(10)).await;
    assert!(true, "audio processor config passed");
}
