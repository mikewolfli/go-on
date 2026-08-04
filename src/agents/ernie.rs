//! Baidu ERNIE agent (unified Wenxin + Qianfan implementations).
//!
//! Wenxin (`aip.baidubce.com` wenxinworkshop, OAuth token in the query
//! string) and Qianfan (`qianfan.baidubce.com`, OAuth token in the
//! `Authorization` header) expose the same chat payload shape, the same
//! strict-stage note logic, and the same SSE response stream. They differ
//! only in endpoint, auth transport, and default model list — so both are
//! one agent parameterized by [`ErnieApi`].

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::baidu_auth::BaiduAuthClient;
use crate::agents::{check_api_response, option_f64, option_string, principles_to_text};

/// Strict-completeness instruction injected during review/strict phases.
const STRICT_STAGE_NOTE: &str = "Enforce strict completeness checks: no empty functions, no unhandled errors, no missing boundary checks, and no placeholder implementations.";

/// Which Baidu ERNIE API family this agent talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErnieApi {
    /// `aip.baidubce.com` wenxinworkshop — OAuth token in the query string.
    Wenxin,
    /// `qianfan.baidubce.com` — OAuth token in the `Authorization` header.
    Qianfan,
}

impl ErnieApi {
    /// OAuth scope / provider id used for access-token caching.
    fn auth_scope(self) -> &'static str {
        match self {
            ErnieApi::Wenxin => "wenxin",
            ErnieApi::Qianfan => "qianfan",
        }
    }
}

/// Unified Baidu ERNIE chat agent (Wenxin + Qianfan).
pub struct BaiduErnieAgent {
    api: ErnieApi,
    /// Fixed model id (Qianfan). Wenxin resolves the model from request options.
    model: String,
    client: reqwest::Client,
    auth_client: BaiduAuthClient,
}

impl BaiduErnieAgent {
    /// Create an ERNIE agent for the given API family.
    ///
    /// `model` is only used by [`ErnieApi::Qianfan`]; Wenxin resolves the
    /// model from request options (default `ERNIE-4.5-8K`).
    pub fn new(
        api: ErnieApi,
        model: String,
        api_key_env: String,
        secret_key_env: String,
        client: reqwest::Client,
    ) -> Self {
        Self {
            api,
            model,
            auth_client: BaiduAuthClient::new(api_key_env, secret_key_env, client.clone()),
            client,
        }
    }

    /// Resolve the model for the outgoing request.
    ///
    /// Wenxin reads `options["model"]` (falling back to `ERNIE-4.5-8K` for
    /// `auto`/empty); Qianfan always uses the model fixed at construction.
    fn resolve_target_model(&self, options: &Option<HashMap<String, Value>>) -> String {
        match self.api {
            ErnieApi::Qianfan => self.model.clone(),
            ErnieApi::Wenxin => {
                let model = option_string(options, "model").unwrap_or_default();
                if model == "auto" || model.is_empty() {
                    "ERNIE-4.5-8K".to_string()
                } else {
                    model
                }
            }
        }
    }

    /// Wenxin endpoint path for a model id (pro tier uses `chat/completions_pro`).
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

        let model = self.resolve_target_model(options);
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
impl Agent for BaiduErnieAgent {
    async fn chat_once(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let scope = self.api.auth_scope();
        let token = self.auth_client.get_access_token(scope).await?;
        let payload = self.build_payload(messages, principles, options);

        let response = match self.api {
            ErnieApi::Wenxin => {
                let target_model = self.resolve_target_model(options);
                let endpoint_path = Self::endpoint_for_model(&target_model);
                let endpoint = format!(
                    "https://aip.baidubce.com/rpc/2.0/ai_custom/v1/wenxinworkshop/{endpoint_path}?access_token={token}"
                );
                self.client.post(endpoint).json(&payload).send().await?
            }
            ErnieApi::Qianfan => {
                self.client
                    .post("https://qianfan.baidubce.com/v2/chat/completions")
                    .header("Authorization", format!("Bearer {token}"))
                    .json(&payload)
                    .send()
                    .await?
            }
        };

        let response = check_api_response(response, scope).await?;

        let cfg = crate::agents::streaming_config(options, false);
        crate::agents::stream_sse_to_sender(response, sender, &cfg).await
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        match self.api {
            ErnieApi::Wenxin => vec![
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
            ],
            ErnieApi::Qianfan => vec![
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
            ],
        }
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

    fn wenxin_agent() -> BaiduErnieAgent {
        BaiduErnieAgent::new(
            ErnieApi::Wenxin,
            String::new(),
            "WENXIN_API_KEY".to_string(),
            "WENXIN_SECRET_KEY".to_string(),
            reqwest::Client::new(),
        )
    }

    fn qianfan_agent() -> BaiduErnieAgent {
        BaiduErnieAgent::new(
            ErnieApi::Qianfan,
            "ERNIE-4.5-8K".to_string(),
            "QIANFAN_API_KEY".to_string(),
            "QIANFAN_SECRET_KEY".to_string(),
            reqwest::Client::new(),
        )
    }

    #[test]
    fn stage_instruction_empty_when_no_principles() {
        assert_eq!(BaiduErnieAgent::stage_instruction(false, &None), "");
        assert_eq!(
            BaiduErnieAgent::stage_instruction(
                false,
                &Some(HashMap::from([("stage".to_string(), json!("early")),]))
            ),
            ""
        );
    }

    #[test]
    fn stage_instruction_returns_note_when_principles_and_strict_phase() {
        assert_eq!(
            BaiduErnieAgent::stage_instruction(
                true,
                &Some(HashMap::from([("stage".to_string(), json!("review")),]))
            ),
            STRICT_STAGE_NOTE
        );
        assert_eq!(
            BaiduErnieAgent::stage_instruction(
                true,
                &Some(HashMap::from([("stage".to_string(), json!("strict")),]))
            ),
            STRICT_STAGE_NOTE
        );
    }

    #[test]
    fn stage_instruction_empty_when_not_strict_phase() {
        assert_eq!(
            BaiduErnieAgent::stage_instruction(
                true,
                &Some(HashMap::from([("stage".to_string(), json!("early")),]))
            ),
            ""
        );
    }

    #[test]
    fn build_payload_combines_principles_stage_and_messages() {
        let payload = wenxin_agent().build_payload(
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
        let payload = wenxin_agent().build_payload(
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
    fn qianfan_payload_uses_fixed_model_ignoring_options() {
        let payload = qianfan_agent().build_payload(
            &[message("user", "hi")],
            &None,
            &Some(HashMap::from([("model".to_string(), json!("ernie-lite"))])),
        );
        assert_eq!(payload["model"], "ERNIE-4.5-8K");
    }

    #[test]
    fn wenxin_payload_resolves_model_from_options() {
        let payload = wenxin_agent().build_payload(
            &[message("user", "hi")],
            &None,
            &Some(HashMap::from([("model".to_string(), json!("ernie-lite"))])),
        );
        assert_eq!(payload["model"], "ernie-lite");
    }

    #[test]
    fn endpoint_for_model_routes_expected_path() {
        assert_eq!(
            BaiduErnieAgent::endpoint_for_model("ernie-4.0-turbo-8k"),
            "chat/completions_pro"
        );
        assert_eq!(
            BaiduErnieAgent::endpoint_for_model("ernie-3.5-turbo"),
            "chat/completions"
        );
    }
}
