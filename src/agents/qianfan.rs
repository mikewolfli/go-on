//! Baidu Qianfan agent implementation
//!
//! This module provides an implementation for the Baidu Qianfan AI platform.

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, retry_chat_once, token_request_failed_msg};
use crate::agents::{option_f64, principles_to_text, stream_sse_to_sender};

const STRICT_STAGE_NOTE: &str = "Enforce strict completeness checks: no empty functions, no unhandled errors, no missing boundary checks, and no placeholder implementations.";

#[derive(Debug, Deserialize)]
struct QianfanTokenResponse {
    access_token: String,
    expires_in: Option<u64>,
}

struct CachedQianfanToken {
    token: String,
    expires_at: Instant,
}

pub struct QianfanAgent {
    api_key_env: String,
    secret_key_env: String,
    model: String,
    client: reqwest::Client,
    token_cache: Mutex<Option<CachedQianfanToken>>,
}

impl QianfanAgent {
    pub fn new(
        api_key_env: String,
        secret_key_env: String,
        model: String,
        client: reqwest::Client,
    ) -> Self {
        Self {
            api_key_env,
            secret_key_env,
            model,
            client,
            token_cache: Mutex::new(None),
        }
    }

    async fn get_access_token(&self) -> Result<String> {
        {
            let cache = self.token_cache.lock().await;
            if let Some(cached) = cache.as_ref() {
                if cached.expires_at > Instant::now() {
                    return Ok(cached.token.clone());
                }
            }
        }

        let api_key = resolve_secret(&self.api_key_env, "qianfan.api_key_env")?;
        let secret_key = resolve_secret(&self.secret_key_env, "qianfan.secret_key_env")?;

        let mut url = reqwest::Url::parse("https://aip.baidubce.com/oauth/2.0/token")?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("grant_type", "client_credentials");
            pairs.append_pair("client_id", api_key.as_str());
            pairs.append_pair("client_secret", secret_key.as_str());
        }
        let response = self.client.get(url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                token_request_failed_msg("qianfan", &status.to_string(), &body)
            );
        }

        let token_response: QianfanTokenResponse = response.json().await?;
        let ttl_seconds = token_response.expires_in.unwrap_or(1800);
        let safety_margin = ttl_seconds.min(120);
        let expires_at = Instant::now() + Duration::from_secs(ttl_seconds - safety_margin);

        {
            let mut cache = self.token_cache.lock().await;
            *cache = Some(CachedQianfanToken {
                token: token_response.access_token.clone(),
                expires_at,
            });
        }

        Ok(token_response.access_token)
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

    async fn chat_once(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let token = self.get_access_token().await?;
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
}

#[async_trait]
impl Agent for QianfanAgent {
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> crate::core::error::Result<()> {
        let chat_messages = messages;

        retry_chat_once(
            || async {
                self.chat_once(&chat_messages, &principles, &options, sender.clone())
                    .await
                    .map_err(Into::into)
            },
            3,
        )
        .await
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

    fn default_model(&self) -> Option<ModelInfo> {
        self.available_models().into_iter().find(|m| m.is_default)
    }

    fn supports_model_override(&self) -> bool {
        true
    }
}
