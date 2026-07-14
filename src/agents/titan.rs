//! Amazon Titan agent implementation
//!
//! This module provides an implementation for the Amazon Titan API.
//!
//! ⚠ REQUIREMENTS:
//! This agent requires AWS credentials to sign requests via SigV4.
//! Set `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and optionally `AWS_SESSION_TOKEN`
//! as environment variables, or use an AWS profile with `AWS_PROFILE`.
//!
//! The URL should point to an AWS Bedrock Runtime endpoint, e.g.:
//! `https://bedrock-runtime.us-east-1.amazonaws.com`
//!
//! The model should be a Titan model ID, e.g.: `amazon.titan-text-premier-v1:0`
//!
//! ⚠ The current implementation sends OpenAI-compatible JSON payloads.
//! For production use, you may need a proxy/sidecar that handles AWS SigV4 signing,
//! or use the AWS SDK directly.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, retry_chat_once};
use crate::agents::{apply_openai_common_options, principles_to_text, stream_sse_to_sender};

pub struct TitanAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl TitanAgent {
    pub fn new(
        api_key_env: String,
        base_url: String,
        model: String,
        client: reqwest::Client,
    ) -> Self {
        Self {
            api_key_env,
            base_url,
            model,
            client,
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

        if let Some(items) = principles {
            if !items.is_empty() {
                system_text.push_str(&principles_to_text(items));
                system_text.push('\n');
            }
        }

        if !system_text.is_empty() {
            final_messages.push(Message {
                role: "system".to_string(),
                content: system_text,
            });
        }
        final_messages.extend(messages.iter().cloned());

        let mut payload = json!({
            "model": self.model,
            "messages": final_messages,
            "stream": true
        });

        apply_openai_common_options(&mut payload, options);

        payload
    }

    async fn chat_once(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let api_key = resolve_secret(&self.api_key_env, "titan.api_key_env")?;

        // Use the Bedrock Converse API endpoint (OpenAI-compatible style)
        // For proper SigV4 signing, deploy a proxy sidecar or use the AWS SDK.
        let base = self.base_url.trim_end_matches('/');
        let endpoint = format!("{}/model/{}/invoke", base, self.model);

        let payload = self.build_payload(messages, principles, options);

        let response = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                chat_request_failed_msg("titan", &status.to_string(), &body)
            );
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for TitanAgent {
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> crate::core::error::Result<()> {
        retry_chat_once(
            || async {
                self.chat_once(&messages, &principles, &options, sender.clone())
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
                id: "amazon.titan-text-premier-v1:0".to_string(),
                name: "Amazon Titan Text Premier V1".to_string(),
                description: "Amazon Titan Text Premier V1".to_string(),
                is_default: self.model == "amazon.titan-text-premier-v1:0",
                capabilities: vec!["chat".to_string()],
                context_window: Some(4096),
            },
            ModelInfo {
                id: "amazon.titan-text-express-v1".to_string(),
                name: "Amazon Titan Text Express V1".to_string(),
                description: "Amazon Titan Text Express V1".to_string(),
                is_default: self.model == "amazon.titan-text-express-v1",
                capabilities: vec!["chat".to_string()],
                context_window: Some(8192),
            },
        ]
    }
}
