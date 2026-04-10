//! copilot.rs
//! Auto-generated English doc: module overview.
//!
use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::agent::{Agent, Message};
use crate::agents::{
    option_f64, option_string, option_u64, principles_to_text, stream_sse_to_sender,
};

pub struct CopilotAgent {
    base_url: String,
    client: reqwest::Client,
}

impl CopilotAgent {
    pub fn new(base_url: String, client: reqwest::Client) -> Self {
        Self { base_url, client }
    }

    fn merge_principles_into_messages(
        &self,
        mut messages: Vec<Message>,
        principles: Option<Vec<String>>,
    ) -> Vec<Message> {
        if let Some(items) = principles {
            if !items.is_empty() {
                let instruction = principles_to_text(&items);
                if let Some(first_user) = messages.iter_mut().find(|m| m.role == "user") {
                    first_user.content = format!("{}\n{}", instruction, first_user.content);
                } else {
                    messages.insert(
                        0,
                        Message {
                            role: "user".to_string(),
                            content: instruction,
                        },
                    );
                }
            }
        }

        messages
    }

    fn build_payload(
        &self,
        messages: Vec<Message>,
        options: Option<HashMap<String, Value>>,
    ) -> Value {
        let model = option_string(&options, "model").unwrap_or_else(|| "copilot".to_string());
        let temperature = option_f64(&options, "temperature");
        let max_tokens = option_u64(&options, "max_tokens");
        let top_p = option_f64(&options, "top_p");

        let mut payload = json!({
            "model": model,
            "messages": messages,
            "stream": true
        });

        if let Some(value) = temperature {
            payload["temperature"] = Value::from(value);
        }
        if let Some(value) = max_tokens {
            payload["max_tokens"] = Value::from(value);
        }
        if let Some(value) = top_p {
            payload["top_p"] = Value::from(value);
        }

        payload
    }

    async fn chat_once(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> Result<()> {
        let messages = self.merge_principles_into_messages(messages, principles);

        let endpoint = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );
        let payload = self.build_payload(messages, options);

        let response = self.client.post(endpoint).json(&payload).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("copilot request failed with {status}: {body}");
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for CopilotAgent {
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
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

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("copilot request failed")))
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

    fn agent() -> CopilotAgent {
        CopilotAgent::new("http://127.0.0.1:8080".to_string(), reqwest::Client::new())
    }

    #[test]
    fn merge_principles_prefixes_first_user_message() {
        let merged = agent().merge_principles_into_messages(
            vec![
                message("system", "existing"),
                message("user", "implement feature"),
            ],
            Some(vec![
                "Use tests".to_string(),
                "Keep diffs small".to_string(),
            ]),
        );

        assert_eq!(merged.len(), 2);
        assert!(merged[1]
            .content
            .contains("Please follow these programming principles:"));
        assert!(merged[1].content.contains("implement feature"));
    }

    #[test]
    fn merge_principles_inserts_user_message_when_missing() {
        let merged = agent().merge_principles_into_messages(
            vec![message("assistant", "prior output")],
            Some(vec!["Use tests".to_string()]),
        );

        assert_eq!(merged[0].role, "user");
        assert!(merged[0].content.contains("Use tests"));
    }

    #[test]
    fn build_payload_applies_option_overrides() {
        let payload = agent().build_payload(
            vec![message("user", "hello")],
            Some(HashMap::from([
                ("model".to_string(), json!("copilot-chat")),
                ("temperature".to_string(), json!(0.2)),
                ("max_tokens".to_string(), json!(512)),
                ("top_p".to_string(), json!(0.9)),
            ])),
        );

        assert_eq!(payload["model"], "copilot-chat");
        assert_eq!(payload["messages"][0]["content"], "hello");
        assert_eq!(payload["stream"], true);
        assert_eq!(payload["temperature"], 0.2);
        assert_eq!(payload["max_tokens"], 512);
        assert_eq!(payload["top_p"], 0.9);
    }
}
