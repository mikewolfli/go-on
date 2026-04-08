//! Baidu Wenxin agent implementation
//!
//! This module provides an implementation for the Baidu Wenxin API.

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::{option_f64, principles_to_text, stream_sse_to_sender};

const STRICT_STAGE_NOTE: &str = "Enforce strict completeness checks: no empty functions, no unhandled errors, no missing boundary checks, and no placeholder implementations.";

#[derive(Debug, Deserialize)]
struct WenxinTokenResponse {
    access_token: String,
    expires_in: Option<u64>,
}

struct CachedWenxinToken {
    token: String,
    expires_at: Instant,
}

pub struct WenxinAgent {
    api_key_env: String,
    secret_key_env: String,
    client: reqwest::Client,
    token_cache: Mutex<Option<CachedWenxinToken>>,
}

impl WenxinAgent {
    pub fn new(api_key_env: String, secret_key_env: String, client: reqwest::Client) -> Self {
        Self {
            api_key_env,
            secret_key_env,
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

        let api_key = resolve_secret(&self.api_key_env, "wenxin.api_key_env")?;
        let secret_key = resolve_secret(&self.secret_key_env, "wenxin.secret_key_env")?;

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
            anyhow::bail!("wenxin token request failed with {status}: {body}");
        }

        let token_response: WenxinTokenResponse = response.json().await?;
        let ttl_seconds = token_response.expires_in.unwrap_or(1800);
        let safety_margin = ttl_seconds.min(120);
        let expires_at = Instant::now() + Duration::from_secs(ttl_seconds - safety_margin);

        {
            let mut cache = self.token_cache.lock().await;
            *cache = Some(CachedWenxinToken {
                token: token_response.access_token.clone(),
                expires_at,
            });
        }

        Ok(token_response.access_token)
    }

    fn stage_instruction(_options: &Option<HashMap<String, Value>>) -> &'static str {
        STRICT_STAGE_NOTE
    }

    fn build_payload(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
    ) -> Value {
        let mut final_messages: Vec<Message> = Vec::new();
        let mut system_text = String::new();

        if let Some(items) = principles {
            if !items.is_empty() {
                system_text.push_str(&principles_to_text(&items));
                system_text.push('\n');
            }
        }

        system_text.push_str(Self::stage_instruction(options));

        final_messages.push(Message {
            role: "system".to_string(),
            content: system_text,
        });
        final_messages.extend(messages);

        let mut payload = json!({
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
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: mpsc::UnboundedSender<String>,
    ) -> Result<()> {
        let token = self.get_access_token().await?;
        let endpoint = format!(
            "https://aip.baidubce.com/rpc/2.0/ai_custom/v1/wenxinworkshop/chat/completions_pro?access_token={token}"
        );
        let payload = self.build_payload(messages, principles, &options);

        let response = self.client.post(endpoint).json(&payload).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("wenxin chat request failed with {status}: {body}");
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for WenxinAgent {
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: mpsc::UnboundedSender<String>,
    ) -> Result<()> {
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..=2 {
            match self
                .chat_once(
                    messages.clone(),
                    principles.clone(),
                    options.clone(),
                    sender.clone(),
                )
                .await
            {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_error = Some(err);
                    if attempt < 2 {
                        sleep(Duration::from_secs(1_u64 << attempt)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("wenxin request failed")))
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "ernie-4.0-turbo-8k".to_string(),
                name: "Ernie 4.0 Turbo 8K".to_string(),
                description: "Latest Ernie model with 8K context window".to_string(),
                is_default: false,
                capabilities: vec!["chat".to_string()],
                context_window: Some(8192),
            },
            ModelInfo {
                id: "ernie-3.5-turbo".to_string(),
                name: "Ernie 3.5 Turbo".to_string(),
                description: "Fast and balanced model for general use".to_string(),
                is_default: true,
                capabilities: vec!["chat".to_string()],
                context_window: Some(4096),
            },
        ]
    }

    fn default_model(&self) -> Option<ModelInfo> {
        self.available_models().into_iter().find(|m| m.is_default)
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
    fn stage_instruction_enforces_strict_mode() {
        assert_eq!(WenxinAgent::stage_instruction(&None), STRICT_STAGE_NOTE);
        assert_eq!(
            WenxinAgent::stage_instruction(&Some(HashMap::from([(
                "stage".to_string(),
                json!("early"),
            )]))),
            STRICT_STAGE_NOTE
        );
    }

    #[test]
    fn build_payload_combines_principles_stage_and_messages() {
        let payload = agent().build_payload(
            vec![message("user", "review this")],
            Some(vec!["Check safety".to_string()]),
            &Some(HashMap::from([
                ("stage".to_string(), json!("early")),
                ("temperature".to_string(), json!(0.3)),
                ("top_p".to_string(), json!(0.8)),
            ])),
        );

        let system = payload["messages"][0]["content"].as_str().unwrap();
        assert_eq!(payload["messages"][0]["role"], "system");
        assert!(system.contains("Check safety"));
        assert!(system.contains(STRICT_STAGE_NOTE));
        assert_eq!(payload["messages"][1]["content"], "review this");
        assert_eq!(payload["temperature"], 0.3);
        assert_eq!(payload["top_p"], 0.8);
    }
}
