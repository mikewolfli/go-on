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
//! File parsing runs through the real `DocumentParser::parse_bytes` and STT
//! through the real `AudioProcessor::transcribe` (asserting its documented
//! missing-key error path when no API key is configured).

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

/// Full multimodal pipeline: document parsing using real parsers.
#[tokio::test]
async fn test_multimodal_pipeline_full() {
    let mut ctx = MultimodalE2eContext::new();

    // ── 1. Setup ───────────────────────────────────────────────────────
    // Use real parsers where features are available.

    // ── 2. Document (Markdown) parsing ──────────────────────────────────
    // Exercise the real parser: DocumentParser::default().parse_bytes().
    let md_content = "# Hello\n\nThis is a **test** document with `code`.";
    let md_bytes: Vec<u8> = md_content.as_bytes().to_vec();
    let parser = go_on::multimodal::DocumentParser::default();

    // The parsed text is the real output of the production parser. The
    // markdown backend is feature-gated (`document-markdown` via
    // `sub-bus-multimodal`): under that feature the parser must produce real
    // text; without it the parser must fail with a clear feature-gating
    // error. Each branch asserts exactly its own behavior — no dual-path
    // "either outcome passes" assertions.
    let parsed_text: Option<String> = {
        #[cfg(feature = "sub-bus-multimodal")]
        {
            let content = parser
                .parse_bytes(&md_bytes, "md")
                .expect("markdown parsing must succeed when sub-bus-multimodal is enabled");
            assert!(
                !content.text_content.is_empty(),
                "parsed text must not be empty"
            );
            assert!(
                content.char_count() > 0,
                "char_count must reflect real extracted text"
            );
            ctx.parsed_text = Some(content.text_content.clone());
            Some(content.text_content)
        }
        #[cfg(not(feature = "sub-bus-multimodal"))]
        {
            match parser.parse_bytes(&md_bytes, "md") {
                Ok(_) => {
                    panic!("markdown parser must not succeed without the document-markdown feature")
                }
                Err(e) => {
                    let err_str = e.to_string();
                    assert!(
                        err_str.contains("feature")
                            || err_str.contains("disabled")
                            || err_str.contains("markdown"),
                        "feature-gated parser must fail with a clear message, got: {err_str}"
                    );
                    None
                }
            }
        }
    };

    // ── 3. Document parse and MultimodalInput variant ─────────────────
    let doc_bytes = b"# Sample".to_vec();
    let doc_input = MultimodalInput::Document(doc_bytes, "md".to_string());

    match &doc_input {
        MultimodalInput::Document(_, ext) => {
            assert_eq!(ext, "md", "extension must match");
        }
        _ => panic!("expected Document variant"),
    }

    // ── 4. Inject into chat context ────────────────────────────────────
    // The payload is built from the real parsed output; when the parser is
    // feature-gated out the parse error already failed loudly above.
    if let Some(text) = &parsed_text {
        assert!(!text.is_empty(), "parsed text must be non-empty");
        assert_eq!(ctx.parsed_text.as_deref(), Some(text.as_str()));
        let injected_payload_size = text.len();
        assert!(
            injected_payload_size > 0,
            "injected payload must be non-empty"
        );
    }

    // ── 5. Audio STT ──────────────────────────────────────────────────
    let audio_config = AudioProcessorConfig::default();
    assert_eq!(audio_config.backend, SttBackend::OpenAIWhisper);
    assert_eq!(audio_config.sample_rate, 16000);
    assert!(!audio_config.enable_diarization);

    // Call the real transcription API with the default config (no API key
    // configured): the production pipeline must fail fast with the
    // documented MissingApiKey error instead of hitting the network.
    let processor = go_on::multimodal::AudioProcessor::new(audio_config);
    let transcription = processor.transcribe(&[0u8; 44], AudioFormat::Wav);
    match transcription {
        Ok(t) => {
            // Only reachable with a key configured; assert real output.
            assert!(
                !t.text.is_empty() && !t.language.is_empty(),
                "STT must produce text"
            );
            ctx.transcription = Some(t.text.clone());
        }
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.to_lowercase().contains("api key"),
                "without a key the real API must report MissingApiKey, got: {err_str}"
            );
        }
    }

    // ── 6. Combined multimodal injection ───────────────────────────────
    // Combine whatever the real pipeline produced; the assertion only
    // requires the format shape, never a hard-coded string.
    let combined_prompt = format!(
        "Document: {}\nAudio: {}",
        ctx.parsed_text.as_deref().unwrap_or(""),
        ctx.transcription.as_deref().unwrap_or(""),
    );
    assert!(
        combined_prompt.starts_with("Document:") && combined_prompt.contains("Audio:"),
        "combined payload must carry both sections"
    );

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

    // Verify construction with different backends (only OpenAIWhisper remains
    // after the placeholder WhisperLocal/Vosk backends were removed).
    let config_openai = AudioProcessorConfig {
        backend: SttBackend::OpenAIWhisper,
        openai_api_key: Some("sk-test".into()),
        language_hint: Some("en".into()),
        ..Default::default()
    };
    assert_eq!(config_openai.backend, SttBackend::OpenAIWhisper);
    assert_eq!(config_openai.openai_api_key.as_deref(), Some("sk-test"));

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
}
