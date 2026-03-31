//! Google Gemini agent implementation
//!
//! This module provides an implementation for the Google Gemini API.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message};
use crate::agents::{option_f64, principles_to_text, stream_sse_to_sender};

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

        if let Some(items) = principles {
            if !items.is_empty() {
                let system_text = principles_to_text(&items);
                contents.push(json!({
                    "role": "user",
                    "parts": [{"text": system_text}]
                }));
            }
        }

        for message in messages {
            contents.push(json!({
                "role": message.role,
                "parts": [{"text": message.content}]
            }));
        }

        let mut payload = json!({
            "model": self.model,
            "contents": contents,
            "stream": true
        });

        if let Some(value) = option_f64(options, "temperature") {
            payload["temperature"] = Value::from(value);
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
        let api_key = resolve_secret(&self.api_key_env, "gemini.api_key_env")?;
        let endpoint = format!(
            "{}/models/{}/generateContent?key={}",
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
            anyhow::bail!("gemini chat request failed with {status}: {body}");
        }

        // Note: Gemini API uses a different streaming format, so we need to handle it specially
        // For now, we'll use the same stream_sse_to_sender function, but it may need adjustments
        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for GeminiAgent {
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

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("gemini request failed")))
    }
}
