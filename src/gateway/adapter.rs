//! Platform adapter trait (M3.4 multi-platform gateway).
//!
//! A platform adapter knows how to translate the platform's inbound webhook
//! payload into a neutral [`InboundMessage`] and how to render a reply back
//! into the platform's response format. The rest of the gateway — registry,
//! per-chat turn lease, delivery ledger, HTTP listener — is platform-agnostic,
//! so adding a second platform only means implementing this trait and
//! registering it (the acceptance criterion for M3.4).

use anyhow::Result;
use serde_json::Value;

/// A single inbound message from a platform user, in neutral form.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    /// Platform-scoped conversation identifier (e.g. a chat id). Together with
    /// the platform name it forms the gateway session key.
    pub platform_chat_id: String,
    /// The message text forwarded to the agent as the turn prompt.
    pub text: String,
    /// The raw parsed payload, kept for adapter-specific rendering decisions.
    pub raw: Value,
}

/// A platform integration: parses inbound payloads and renders replies.
///
/// Implementations must be `Send + Sync` so a registered adapter can be shared
/// across the per-connection tasks of the gateway HTTP listener.
pub trait PlatformAdapter: Send + Sync {
    /// Stable platform name — the registry key (`adapter("telegram")`) and the
    /// `POST /webhook/<platform>` route segment. Must be unique per adapter
    /// instance family.
    fn platform_name(&self) -> &'static str;

    /// Parse a raw inbound webhook payload into one or more neutral messages.
    ///
    /// `content_type` is the HTTP `Content-Type` header of the request; an
    /// adapter may use it to pick a parse strategy (JSON body vs. form-encoded
    /// vs. raw text). A malformed payload is an error — never an empty list.
    fn parse_inbound(&self, raw: &[u8], content_type: &str) -> Result<Vec<InboundMessage>>;

    /// Render the agent's reply back into the platform's response format,
    /// addressed to the chat the original message came from.
    fn render_reply(&self, reply: &str, original: &InboundMessage) -> Result<Vec<u8>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Test-only adapter with a trivial JSON echo contract, used to validate
    /// the trait contract shape independent of any concrete adapter.
    struct TestEchoAdapter;

    impl PlatformAdapter for TestEchoAdapter {
        fn platform_name(&self) -> &'static str {
            "test-echo"
        }

        fn parse_inbound(&self, raw: &[u8], _content_type: &str) -> Result<Vec<InboundMessage>> {
            let value: Value = serde_json::from_slice(raw)?;
            let chat_id = value["chat_id"].as_str().unwrap_or_default().to_string();
            let text = value["text"].as_str().unwrap_or_default().to_string();
            Ok(vec![InboundMessage {
                platform_chat_id: chat_id,
                text,
                raw: value,
            }])
        }

        fn render_reply(&self, reply: &str, original: &InboundMessage) -> Result<Vec<u8>> {
            Ok(serde_json::to_vec(&json!({
                "chat_id": original.platform_chat_id,
                "text": reply,
            }))?)
        }
    }

    #[test]
    fn adapter_parse_and_render_round_trip() {
        let adapter = TestEchoAdapter;
        let inbound = adapter
            .parse_inbound(br#"{"chat_id":"chat-9","text":"ping"}"#, "application/json")
            .expect("valid payload parses");
        assert_eq!(inbound.len(), 1);
        assert_eq!(inbound[0].platform_chat_id, "chat-9");
        assert_eq!(inbound[0].text, "ping");

        let rendered = adapter
            .render_reply("pong", &inbound[0])
            .expect("reply renders");
        let value: Value = serde_json::from_slice(&rendered).expect("rendered bytes are JSON");
        assert_eq!(value["chat_id"], "chat-9");
        assert_eq!(value["text"], "pong");
        assert_eq!(adapter.platform_name(), "test-echo");
    }

    #[test]
    fn adapter_rejects_malformed_inbound() {
        let adapter = TestEchoAdapter;
        assert!(adapter.parse_inbound(b"not json", "text/plain").is_err());
        assert!(adapter.parse_inbound(b"", "application/json").is_err());
    }
}
