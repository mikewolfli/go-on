//! Baidu Qianfan agent implementation
//!
//! This module provides an implementation for the Baidu Qianfan AI platform.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::chat_request_failed_msg;
use crate::agents::baidu_auth::BaiduAuthClient;
use crate::agents::{option_f64, principles_to_text, stream_sse_to_sender};

const STRICT_STAGE_NOTE: &str = "Enforce strict completeness checks: no empty functions, no unhandled errors, no missing boundary checks, and no placeholder implementations.";

pub struct QianfanAgent {
    model: String,
    client: reqwest::Client,
    auth_client: BaiduAuthClient,
}

impl QianfanAgent {
    pub fn new(
        api_key_env: String,
        secret_key_env: String,
        model: String,
        client: reqwest::Client,
    ) -> Self {
        Self {
            auth_client: BaiduAuthClient::new(api_key_env, secret_key_env, client.clone()),
            model,
            client,
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

        let mut payload = json! ({
            "model": self.model,
            "messages": final_messages,
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
impl Agent for QianfanAgent {
    async fn chat_once(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let token = self.auth_client.get_access_token("qianfan").await?;
        let endpoint = "https://qianfan.baidubce.com/v2/chat/completions";
        let payload = self.build_payload(messages, principles, options);

        let response = self
            .client
            .post(endpoint)
            .header("Authorization", format!("Bearer {}", token))
            .json(&payload)
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                chat_request_failed_msg("qianfan", &status.to_string(), &body)
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
                is_default: self.model == "ERNIE-4.5-8K",
                capabilities: vec!["chat".to_string(), "function_calling".to_string()],
                context_window: Some(8192),
            },
            ModelInfo {
                id: "ernie-4.0-8k".to_string(),
                name: "Ernie 4.0 8K".to_string(),
                description: "Baidu Ernie 4.0 flagship model (8K context)".to_string(),
                is_default: self.model == "ernie-4.0-8k",
                capabilities: vec!["chat".to_string(), "function_calling".to_string()],
                context_window: Some(8192),
            },
            ModelInfo {
                id: "ernie-3.5-8k".to_string(),
                name: "Ernie 3.5 8K".to_string(),
                description: "Baidu Ernie 3.5 balanced model (8K context)".to_string(),
                is_default: self.model == "ernie-3.5-8k",
                capabilities: vec!["chat".to_string(), "function_calling".to_string()],
                context_window: Some(8192),
            },
            ModelInfo {
                id: "ernie-speed".to_string(),
                name: "Ernie Speed".to_string(),
                description: "Baidu Ernie Speed (fast, cost-effective)".to_string(),
                is_default: self.model == "ernie-speed",
                capabilities: vec!["chat".to_string()],
                context_window: Some(4096),
            },
            ModelInfo {
                id: "ernie-lite".to_string(),
                name: "Ernie Lite".to_string(),
                description: "Baidu Ernie Lite (lightweight)".to_string(),
                is_default: self.model == "ernie-lite",
                capabilities: vec!["chat".to_string()],
                context_window: Some(4096),
            },
        ]
    }
}
