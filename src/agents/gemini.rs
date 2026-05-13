//! Google Gemini agent implementation
//!
//! This module provides an implementation for the Google Gemini API.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message};
use crate::agents::agent::{chat_request_failed_msg, request_failed_msg};
use crate::agents::{option_f64, principles_to_text, stream_sse_events, SseEventAction};

pub struct GeminiAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl GeminiAgent {
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
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
    ) -> Value {
        let mut contents: Vec<Value> = Vec::new();
        let mut system_instruction: Option<String> = None;

        if let Some(items) = principles {
            if !items.is_empty() {
                let system_text = principles_to_text(&items);
                system_instruction = Some(system_text);
            }
        }

        for message in &messages {
            if message.role == "system" {
                system_instruction = Some(message.content.clone());
            } else {
                contents.push(json!({
                    "role": message.role,
                    "parts": [{"text": message.content}]
                }));
            }
        }

        let mut payload = json!({
            "contents": contents
        });

        if let Some(system_text) = system_instruction {
            payload["system_instruction"] = json!({
                "parts": [{"text": system_text}]
            });
        }

        let mut generation_config = json!({});
        if let Some(value) = option_f64(options, "temperature") {
            generation_config["temperature"] = Value::from(value);
        }
        if let Some(value) = option_f64(options, "top_p") {
            generation_config["top_p"] = Value::from(value);
        }
        if let Some(value) = option_f64(options, "max_output_tokens") {
            generation_config["max_output_tokens"] = Value::from(value);
        }
        if let Some(value) = options.as_ref().and_then(|o| o.get("max_tokens")) {
            if let Some(n) = value.as_u64() {
                generation_config["max_output_tokens"] = Value::from(n);
            }
        }
        if generation_config.as_object().is_some_and(|o| !o.is_empty()) {
            payload["generationConfig"] = generation_config;
        }

        payload
    }

    async fn chat_once(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let api_key = resolve_secret(&self.api_key_env, "gemini.api_key_env")?;
        let endpoint = format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url.trim_end_matches('/'),
            self.model,
            api_key
        );
        let payload = self.build_payload(messages, principles, &options);

        let response = self
            .client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                chat_request_failed_msg("gemini", &status.to_string(), &body)
            );
        }

        // Parse Gemini streaming response which uses `candidates[0].content.parts[0].text`
        // format instead of OpenAI's `choices[0].delta.content`.
        stream_sse_events(response, move |data| {
            if data.trim() == "[DONE]" {
                return Ok(SseEventAction::Stop);
            }

            if let Ok(json) = serde_json::from_str::<Value>(data) {
                // Gemini streaming format: candidates[0].content.parts[0].text
                if let Some(token) = json
                    .get("candidates")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("content"))
                    .and_then(|c| c.get("parts"))
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("text"))
                    .and_then(|c| c.as_str())
                {
                    if sender.send(token.to_string()).is_err() {
                        return Ok(SseEventAction::Stop);
                    }
                }
            }

            Ok(SseEventAction::Continue)
        })
        .await
    }
}

#[async_trait]
impl Agent for GeminiAgent {
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> crate::core::error::Result<()> {
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

        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("gemini")))
            .into())
    }
}
