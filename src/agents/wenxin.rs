//! Baidu Wenxin agent implementation
//!
//! This module provides an implementation for the Baidu Wenxin API.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::chat_request_failed_msg;
use crate::agents::baidu_auth::BaiduAuthClient;
use crate::agents::{option_f64, option_string, principles_to_text, stream_sse_to_sender};

const STRICT_STAGE_NOTE: &str = "Enforce strict completeness checks: no empty functions, no unhandled errors, no missing boundary checks, and no placeholder implementations.";

pub struct WenxinAgent {
    client: reqwest::Client,
    auth_client: BaiduAuthClient,
}

impl WenxinAgent {
    pub fn new(api_key_env: String, secret_key_env: String, client: reqwest::Client) -> Self {
        Self {
            auth_client: BaiduAuthClient::new(api_key_env, secret_key_env, client.clone()),
            client,
        }
    }

    fn resolve_target_model(options: &Option<HashMap<String, Value>>) -> String {
        let model = option_string(options, "model").unwrap_or_default();
        if model == "auto" || model.is_empty() {
            "ERNIE-4.5-8K".to_string()
        } else {
            model
        }
    }

    fn endpoint_for_model(model: &str) -> &'static str {
        match model {
            "ernie-4.0-turbo-8k" => "chat/completions_pro",
            _ => "chat/completions",
        }
    }

    fn stage_instruction(
        has_principles: bool,
        options: &Option<HashMap<String, Value>>,
    ) -> &'static str {
        let stage = options
            .as_ref()
            .and_then(|o| o.get("stage"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let is_strict_phase = matches!(
            stage,
            "strict" | "review" | "audit" | "final_review" | "verification"
        );
        if has_principles && is_strict_phase {
            STRICT_STAGE_NOTE
        } else {
            ""
        }
    }

    fn build_payload(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
    ) -> Value {
        let mut final_messages: Vec<Message> = Vec::new();
        let mut system_text = String::new();

        let has_principles = principles.as_ref().is_some_and(|p| !p.is_empty());

        if let Some(ref items) = principles {
            if !items.is_empty() {
                system_text.push_str(&principles_to_text(items));
                system_text.push('\n');
            }
        }

        let stage_note = Self::stage_instruction(has_principles, options);
        if !stage_note.is_empty() {
            system_text.push_str(stage_note);
        }

        final_messages.push(Message {
            role: "system".to_string(),
            content: system_text,
        });
        final_messages.extend(messages.iter().cloned());

        let model = Self::resolve_target_model(options);
        let mut payload = json!({
            "messages": final_messages,
            "model": model,
            "stream": true
        });

        if let Some(value) = option_f64(options, "temperature") {
            payload["temperature"] = Value::from(value);
        }
        if let Some(value) = option_f64(options, "top_p") {
            payload["top_p"] = Value::from(value);
        }

        payload
    }
}

#[async_trait]
impl Agent for WenxinAgent {
    async fn chat_once(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let token = self.auth_client.get_access_token("wenxin").await?;
        let target_model = Self::resolve_target_model(options);
        let endpoint_path = Self::endpoint_for_model(&target_model);
        let endpoint = format!(
            "https://aip.baidubce.com/rpc/2.0/ai_custom/v1/wenxinworkshop/{endpoint_path}?access_token={token}"
        );
        let payload = self.build_payload(messages, principles, options);

        let response = self.client.post(endpoint).json(&payload).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                chat_request_failed_msg("wenxin", &status.to_string(), &body)
            );
        }

        stream_sse_to_sender(response, sender).await
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "ERNIE-4.5-8K".to_string(),
                name: "ERNIE 4.5 8K".to_string(),
                description: "Baidu ERNIE 4.5 flagship model (8K context)".to_string(),
                is_default: true, // Wenxin's first listed model is always the default
                capabilities: vec!["chat".to_string(), "function_calling".to_string()],
                context_window: Some(8192),
            },
            ModelInfo {
                id: "ernie-4.0-turbo-8k".to_string(),
                name: "Ernie 4.0 Turbo 8K".to_string(),
                description: "Ernie 4.0 Turbo with 8K context window".to_string(),
                is_default: false,
                capabilities: vec!["chat".to_string()],
                context_window: Some(8192),
            },
            ModelInfo {
                id: "ernie-3.5-turbo".to_string(),
                name: "Ernie 3.5 Turbo".to_string(),
                description: "Fast and balanced model for general use".to_string(),
                is_default: false,
                capabilities: vec!["chat".to_string()],
                context_window: Some(4096),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, content: &str) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    fn agent() -> WenxinAgent {
        WenxinAgent::new(
            "WENXIN_API_KEY".to_string(),
            "WENXIN_SECRET_KEY".to_string(),
            reqwest::Client::new(),
        )
    }

    #[test]
    fn stage_instruction_empty_when_no_principles() {
        assert_eq!(WenxinAgent::stage_instruction(false, &None), "");
        assert_eq!(
            WenxinAgent::stage_instruction(
                false,
                &Some(HashMap::from([("stage".to_string(), json!("early")),]))
            ),
            ""
        );
    }

    #[test]
    fn stage_instruction_returns_note_when_principles_and_strict_phase() {
        assert_eq!(
            WenxinAgent::stage_instruction(
                true,
                &Some(HashMap::from([("stage".to_string(), json!("review")),]))
            ),
            STRICT_STAGE_NOTE
        );
        assert_eq!(
            WenxinAgent::stage_instruction(
                true,
                &Some(HashMap::from([("stage".to_string(), json!("strict")),]))
            ),
            STRICT_STAGE_NOTE
        );
    }

    #[test]
    fn stage_instruction_empty_when_not_strict_phase() {
        assert_eq!(
            WenxinAgent::stage_instruction(
                true,
                &Some(HashMap::from([("stage".to_string(), json!("early")),]))
            ),
            ""
        );
    }

    #[test]
    fn build_payload_combines_principles_stage_and_messages() {
        let payload = agent().build_payload(
            &[message("user", "review this")],
            &Some(vec!["Check safety".to_string()]),
            &Some(HashMap::from([
                ("stage".to_string(), json!("review")),
                ("temperature".to_string(), json!(0.3)),
                ("top_p".to_string(), json!(0.8)),
            ])),
        );

        let system = payload["messages"][0]["content"]
            .as_str()
            .expect("messages[0] content should be a string");
        assert_eq!(payload["messages"][0]["role"], "system");
        assert!(system.contains("Check safety"));
        assert!(system.contains(STRICT_STAGE_NOTE));
        assert_eq!(payload["messages"][1]["content"], "review this");
        assert_eq!(payload["temperature"], 0.3);
        assert_eq!(payload["top_p"], 0.8);
        assert_eq!(payload["model"], "ERNIE-4.5-8K");
    }

    #[test]
    fn build_payload_omits_strict_note_when_not_strict_phase() {
        let payload = agent().build_payload(
            &[message("user", "review this")],
            &Some(vec!["Check safety".to_string()]),
            &Some(HashMap::from([
                ("stage".to_string(), json!("early")),
                ("temperature".to_string(), json!(0.3)),
            ])),
        );

        let system = payload["messages"][0]["content"]
            .as_str()
            .expect("messages[0] content should be a string");
        assert!(system.contains("Check safety"));
        assert!(!system.contains(STRICT_STAGE_NOTE));
    }

    #[test]
    fn endpoint_for_model_routes_expected_path() {
        assert_eq!(
            WenxinAgent::endpoint_for_model("ernie-4.0-turbo-8k"),
            "chat/completions_pro"
        );
        assert_eq!(
            WenxinAgent::endpoint_for_model("ernie-3.5-turbo"),
            "chat/completions"
        );
    }
}
