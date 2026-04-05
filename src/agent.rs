//! Agent system implementation
//!
//! This module defines the Agent trait, AgentRegistry, and related functionality
//! for managing and interacting with different AI agents.
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! They define task contracts, audit schemas, and agent interfaces that will be wired
//! into the execution flow once orchestration logic is implemented.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::agents::{
    Ai21Agent, AlephAgent, AnthropicAgent, CohereAgent, CopilotAgent, DeepQuestAgent,
    DeepSeekAgent, FaceWallAgent, FireworksAgent, GeminiAgent, GlmAgent, GroqAgent, HunyuanAgent,
    LangboatAgent, LlamaAgent, LoopAiAgent, MiniMaxAgent, MistralAgent, MoonshotAgent, NimAgent,
    OpenAiAgent, OpenAiCompatibleAgent, PerplexityAgent, QianfanAgent, QwenAgent, ReplicateAgent,
    SkyworkAgent, StepFunAgent, TitanAgent, TogetherAgent, WenxinAgent, XihuAgent, YiAgent,
};

use crate::config::{AgentConfig, AppConfig};
use crate::pua::PuaExecutionReport;

/// Agent task envelope (Phase 0/1 discipline)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTaskEnvelope {
    pub task_id: String,
    pub phase: String,
    pub role: String,
    pub objective: String,
    pub constraints: Option<String>,
    pub evidence: Option<String>,
    pub input: serde_json::Value,
}

/// Agent output schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTaskResult {
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub audit_log: Option<String>,
    pub pua_report: Option<PuaExecutionReport>,
}

/// Agent decision audit log schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAuditLog {
    pub agent: String,
    pub phase: String,
    pub task_id: String,
    pub decision: String,
    pub rationale: Option<String>,
    pub timestamp: String,
}

/// Keyring prefix for secret references
const KEYRING_PREFIX: &str = "keyring://";

/// Model information for provider selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier (e.g., "deepseek-chat")
    pub id: String,
    /// Human-readable model name
    pub name: String,
    /// Model description
    pub description: String,
    /// Whether this is the default model for the provider
    pub is_default: bool,
    /// Model capabilities (e.g., ["chat", "vision", "function_calling"])
    pub capabilities: Vec<String>,
    /// Context window size
    pub context_window: Option<usize>,
}

/// Chat message structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Message role (e.g., "user", "assistant", "system")
    pub role: String,
    /// Message content
    pub content: String,
}

/// Agent trait defining the interface for all AI agents
///
/// Phase 0/1: 推荐所有 agent 入口方法都支持 AgentTaskEnvelope 作为输入，AgentTaskResult 作为输出，
/// 并在决策点生成 AgentAuditLog 结构，便于后续 trace/replay/audit。
#[async_trait]
pub trait Agent: Send + Sync {
    /// Send chat messages to the agent and receive streaming responses
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: mpsc::UnboundedSender<String>,
    ) -> Result<()>;

    /// Get available models for this provider
    ///
    /// Returns a list of models that can be used with this provider.
    /// Default implementation returns an empty list (providers should override if applicable).
    fn available_models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }

    /// Get the default model for this provider
    ///
    /// Returns the currently configured default model.
    /// Default implementation returns None.
    fn default_model(&self) -> Option<ModelInfo> {
        None
    }

    /// (Phase 0/1 discipline) Structured agent task entrypoint
    fn run_task(&self, envelope: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string());

        let audit = AgentAuditLog {
            agent: "generic".to_string(),
            phase: envelope.phase.clone(),
            task_id: envelope.task_id.clone(),
            decision: "rejected".to_string(),
            rationale: Some(
                "This provider does not implement synchronous task execution; use chat() or provide a concrete run_task override."
                    .to_string(),
            ),
            timestamp,
        };

        Ok(AgentTaskResult {
            success: false,
            output: Some(json!({
                "task_id": envelope.task_id,
                "phase": envelope.phase,
                "role": envelope.role,
                "objective": envelope.objective,
                "status": "unsupported_operation"
            })),
            error: Some(
                "run_task is unsupported for this provider without a concrete override"
                    .to_string(),
            ),
            audit_log: Some(serde_json::to_string(&audit)?),
            pua_report: None,
        })
    }
}

/// Agent registry for managing and accessing agents
pub struct AgentRegistry {
    /// Map of agent names to agent instances
    agents: HashMap<String, Arc<dyn Agent>>,
}

impl AgentRegistry {
    /// Create an empty agent registry
    ///
    /// # Returns
    /// * `Self` - Returns an empty registry
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// Create an agent registry from configuration
    ///
    /// # Arguments
    /// * `config` - Application configuration containing agent definitions
    /// * `client` - HTTP client for agent requests
    ///
    /// # Returns
    /// * `Result<Self>` - Returns Ok(Self) if the registry is created successfully, or an error if something goes wrong
    pub fn from_config(config: Arc<AppConfig>, client: reqwest::Client) -> Result<Self> {
        let mut agents: HashMap<String, Arc<dyn Agent>> = HashMap::new();

        for (name, agent_cfg) in &config.agents {
            let agent = build_agent(agent_cfg, client.clone())
                .with_context(|| format!("failed to build agent '{}'", name))?;
            agents.insert(name.clone(), agent);
        }

        Ok(Self { agents })
    }

    /// Get an agent by name
    ///
    /// # Arguments
    /// * `name` - Agent name
    ///
    /// # Returns
    /// * `Option<Arc<dyn Agent>>` - Returns Some(agent) if found, or None if not found
    pub fn get(&self, name: &str) -> Option<Arc<dyn Agent>> {
        self.agents.get(name).cloned()
    }
}

/// Build an agent based on configuration
///
/// # Arguments
/// * `config` - Agent configuration
/// * `client` - HTTP client for agent requests
///
/// # Returns
/// * `Result<Arc<dyn Agent>>` - Returns Ok(agent) if built successfully, or an error if something goes wrong
fn build_agent(config: &AgentConfig, client: reqwest::Client) -> Result<Arc<dyn Agent>> {
    fn required_field(agent_name: &str, value: &Option<String>, field: &str) -> Result<String> {
        value
            .clone()
            .ok_or_else(|| anyhow::anyhow!("{} agent requires '{}'", agent_name, field))
    }

    match config.agent_type.as_str() {
        "copilot" => {
            let url = required_field("copilot", &config.url, "url")?;
            Ok(Arc::new(CopilotAgent::new(url, client)))
        }
        "deepseek" => {
            let api_key_env = required_field("deepseek", &config.api_key_env, "api_key_env")?;
            let model = config
                .model
                .clone()
                .unwrap_or_else(|| "deepseek-chat".to_string());
            Ok(Arc::new(DeepSeekAgent::new(api_key_env, model, client)))
        }
        "wenxin" => {
            let api_key_env = required_field("wenxin", &config.api_key_env, "api_key_env")?;
            let secret_key_env =
                required_field("wenxin", &config.secret_key_env, "secret_key_env")?;
            Ok(Arc::new(WenxinAgent::new(
                api_key_env,
                secret_key_env,
                client,
            )))
        }
        "openai_compatible" => {
            let url = required_field("openai_compatible", &config.url, "url")?;
            let chat_path = config
                .chat_path
                .clone()
                .unwrap_or_else(|| "/v1/chat/completions".to_string());
            let api_key_env =
                required_field("openai_compatible", &config.api_key_env, "api_key_env")?;
            let model = required_field("openai_compatible", &config.model, "model")?;
            let supports_system = config.supports_system.unwrap_or(true);
            Ok(Arc::new(OpenAiCompatibleAgent::new(
                url,
                chat_path,
                api_key_env,
                model,
                supports_system,
                client,
            )))
        }
        "doubao" => {
            let url = required_field("doubao", &config.url, "url")?;
            let chat_path = config
                .chat_path
                .clone()
                .unwrap_or_else(|| "/chat/completions".to_string());
            let api_key_env = required_field("doubao", &config.api_key_env, "api_key_env")?;
            let model = required_field("doubao", &config.model, "model")?;
            let supports_system = config.supports_system.unwrap_or(true);
            Ok(Arc::new(OpenAiCompatibleAgent::new(
                url,
                chat_path,
                api_key_env,
                model,
                supports_system,
                client,
            )))
        }
        "claude" => {
            let url = config
                .url
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com".to_string());
            let api_key_env = required_field("claude", &config.api_key_env, "api_key_env")?;
            let model = required_field("claude", &config.model, "model")?;
            let anthropic_version = config
                .anthropic_version
                .clone()
                .unwrap_or_else(|| "2023-06-01".to_string());
            let max_tokens = config.max_tokens.unwrap_or(4096);
            Ok(Arc::new(AnthropicAgent::new(
                url,
                api_key_env,
                model,
                anthropic_version,
                max_tokens,
                client,
            )))
        }
        "openai" => {
            let api_key_env = required_field("openai", &config.api_key_env, "api_key_env")?;
            let url = required_field("openai", &config.url, "url")?;
            let model = required_field("openai", &config.model, "model")?;
            Ok(Arc::new(OpenAiAgent::new(api_key_env, url, model, client)))
        }
        "ai21" => {
            let api_key_env = required_field("ai21", &config.api_key_env, "api_key_env")?;
            let url = required_field("ai21", &config.url, "url")?;
            let model = required_field("ai21", &config.model, "model")?;
            Ok(Arc::new(Ai21Agent::new(api_key_env, url, model, client)))
        }
        "aleph" => {
            let api_key_env = required_field("aleph", &config.api_key_env, "api_key_env")?;
            let url = required_field("aleph", &config.url, "url")?;
            let model = required_field("aleph", &config.model, "model")?;
            Ok(Arc::new(AlephAgent::new(api_key_env, url, model, client)))
        }
        "cohere" => {
            let api_key_env = required_field("cohere", &config.api_key_env, "api_key_env")?;
            let url = required_field("cohere", &config.url, "url")?;
            let model = required_field("cohere", &config.model, "model")?;
            Ok(Arc::new(CohereAgent::new(api_key_env, url, model, client)))
        }
        "deepquest" => {
            let api_key_env = required_field("deepquest", &config.api_key_env, "api_key_env")?;
            let url = required_field("deepquest", &config.url, "url")?;
            let model = required_field("deepquest", &config.model, "model")?;
            Ok(Arc::new(DeepQuestAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "facewall" => {
            let api_key_env = required_field("facewall", &config.api_key_env, "api_key_env")?;
            let url = required_field("facewall", &config.url, "url")?;
            let model = required_field("facewall", &config.model, "model")?;
            Ok(Arc::new(FaceWallAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "fireworks" => {
            let api_key_env = required_field("fireworks", &config.api_key_env, "api_key_env")?;
            let url = required_field("fireworks", &config.url, "url")?;
            let model = required_field("fireworks", &config.model, "model")?;
            Ok(Arc::new(FireworksAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "gemini" => {
            let api_key_env = required_field("gemini", &config.api_key_env, "api_key_env")?;
            let url = required_field("gemini", &config.url, "url")?;
            let model = required_field("gemini", &config.model, "model")?;
            Ok(Arc::new(GeminiAgent::new(api_key_env, url, model, client)))
        }
        "glm" => {
            let api_key_env = required_field("glm", &config.api_key_env, "api_key_env")?;
            let url = required_field("glm", &config.url, "url")?;
            let model = required_field("glm", &config.model, "model")?;
            Ok(Arc::new(GlmAgent::new(api_key_env, url, model, client)))
        }
        "groq" => {
            let api_key_env = required_field("groq", &config.api_key_env, "api_key_env")?;
            let url = required_field("groq", &config.url, "url")?;
            let model = required_field("groq", &config.model, "model")?;
            Ok(Arc::new(GroqAgent::new(api_key_env, url, model, client)))
        }
        "langboat" => {
            let api_key_env = required_field("langboat", &config.api_key_env, "api_key_env")?;
            let url = required_field("langboat", &config.url, "url")?;
            let model = required_field("langboat", &config.model, "model")?;
            Ok(Arc::new(LangboatAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "llama" => {
            let api_key_env = required_field("llama", &config.api_key_env, "api_key_env")?;
            let url = required_field("llama", &config.url, "url")?;
            let model = required_field("llama", &config.model, "model")?;
            Ok(Arc::new(LlamaAgent::new(api_key_env, url, model, client)))
        }
        "loopai" => {
            let api_key_env = required_field("loopai", &config.api_key_env, "api_key_env")?;
            let url = required_field("loopai", &config.url, "url")?;
            let model = required_field("loopai", &config.model, "model")?;
            Ok(Arc::new(LoopAiAgent::new(api_key_env, url, model, client)))
        }
        "minimax" => {
            let api_key_env = required_field("minimax", &config.api_key_env, "api_key_env")?;
            let url = required_field("minimax", &config.url, "url")?;
            let model = required_field("minimax", &config.model, "model")?;
            Ok(Arc::new(MiniMaxAgent::new(api_key_env, url, model, client)))
        }
        "mistral" => {
            let api_key_env = required_field("mistral", &config.api_key_env, "api_key_env")?;
            let url = required_field("mistral", &config.url, "url")?;
            let model = required_field("mistral", &config.model, "model")?;
            Ok(Arc::new(MistralAgent::new(api_key_env, url, model, client)))
        }
        "moonshot" => {
            let api_key_env = required_field("moonshot", &config.api_key_env, "api_key_env")?;
            let url = required_field("moonshot", &config.url, "url")?;
            let model = required_field("moonshot", &config.model, "model")?;
            Ok(Arc::new(MoonshotAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "nim" => {
            let api_key_env = required_field("nim", &config.api_key_env, "api_key_env")?;
            let url = required_field("nim", &config.url, "url")?;
            let model = required_field("nim", &config.model, "model")?;
            Ok(Arc::new(NimAgent::new(api_key_env, url, model, client)))
        }
        "perplexity" => {
            let api_key_env = required_field("perplexity", &config.api_key_env, "api_key_env")?;
            let url = required_field("perplexity", &config.url, "url")?;
            let model = required_field("perplexity", &config.model, "model")?;
            Ok(Arc::new(PerplexityAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "replicate" => {
            let api_key_env = required_field("replicate", &config.api_key_env, "api_key_env")?;
            let url = required_field("replicate", &config.url, "url")?;
            let model = required_field("replicate", &config.model, "model")?;
            Ok(Arc::new(ReplicateAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "skywork" => {
            let api_key_env = required_field("skywork", &config.api_key_env, "api_key_env")?;
            let url = required_field("skywork", &config.url, "url")?;
            let model = required_field("skywork", &config.model, "model")?;
            Ok(Arc::new(SkyworkAgent::new(api_key_env, url, model, client)))
        }
        "stepfun" => {
            let api_key_env = required_field("stepfun", &config.api_key_env, "api_key_env")?;
            let url = required_field("stepfun", &config.url, "url")?;
            let model = required_field("stepfun", &config.model, "model")?;
            Ok(Arc::new(StepFunAgent::new(api_key_env, url, model, client)))
        }
        "titan" => {
            let api_key_env = required_field("titan", &config.api_key_env, "api_key_env")?;
            let url = required_field("titan", &config.url, "url")?;
            let model = required_field("titan", &config.model, "model")?;
            Ok(Arc::new(TitanAgent::new(api_key_env, url, model, client)))
        }
        "together" => {
            let api_key_env = required_field("together", &config.api_key_env, "api_key_env")?;
            let url = required_field("together", &config.url, "url")?;
            let model = required_field("together", &config.model, "model")?;
            Ok(Arc::new(TogetherAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "xihu" => {
            let api_key_env = required_field("xihu", &config.api_key_env, "api_key_env")?;
            let url = required_field("xihu", &config.url, "url")?;
            let model = required_field("xihu", &config.model, "model")?;
            Ok(Arc::new(XihuAgent::new(api_key_env, url, model, client)))
        }
        "yi" => {
            let api_key_env = required_field("yi", &config.api_key_env, "api_key_env")?;
            let url = required_field("yi", &config.url, "url")?;
            let model = required_field("yi", &config.model, "model")?;
            Ok(Arc::new(YiAgent::new(api_key_env, url, model, client)))
        }
        "qianfan" => {
            let api_key_env = required_field("qianfan", &config.api_key_env, "api_key_env")?;
            let secret_key_env =
                required_field("qianfan", &config.secret_key_env, "secret_key_env")?;
            let model = required_field("qianfan", &config.model, "model")?;
            Ok(Arc::new(QianfanAgent::new(
                api_key_env,
                secret_key_env,
                model,
                client,
            )))
        }
        "qwen" => {
            let api_key_env = required_field("qwen", &config.api_key_env, "api_key_env")?;
            let secret_key_env = required_field("qwen", &config.secret_key_env, "secret_key_env")?;
            Ok(Arc::new(QwenAgent::new(
                api_key_env,
                secret_key_env,
                client,
            )))
        }
        "hunyuan" => {
            let api_key_env = required_field("hunyuan", &config.api_key_env, "api_key_env")?;
            let secret_key_env =
                required_field("hunyuan", &config.secret_key_env, "secret_key_env")?;
            let url = required_field("hunyuan", &config.url, "url")?;
            let model = required_field("hunyuan", &config.model, "model")?;
            Ok(Arc::new(HunyuanAgent::new(
                api_key_env,
                secret_key_env,
                url,
                model,
                client,
            )))
        }
        other => anyhow::bail!(
            "unsupported agent type '{}'; add implementation in agents/*",
            other
        ),
    }
}

/// Resolve a secret from environment variable or keyring
///
/// # Arguments
/// * `secret_ref` - Secret reference (either environment variable name or keyring:// reference)
/// * `field_name` - Field name for error messages
///
/// # Returns
/// * `Result<String>` - Returns Ok(secret) if resolved successfully, or an error if something goes wrong
pub(crate) fn resolve_secret(secret_ref: &str, field_name: &str) -> Result<String> {
    if let Some(locator) = secret_ref.strip_prefix(KEYRING_PREFIX) {
        let (service, account) = locator.split_once('/').ok_or_else(|| {
            anyhow::anyhow!(
                "invalid {} keyring reference '{}': expected keyring://<service>/<account>",
                field_name,
                secret_ref
            )
        })?;

        if service.trim().is_empty() || account.trim().is_empty() {
            anyhow::bail!(
                "invalid {} keyring reference '{}': service/account must be non-empty",
                field_name,
                secret_ref
            );
        }

        let entry = keyring::Entry::new(service, account)
            .map_err(|err| anyhow::anyhow!("failed to open keyring entry: {}", err))?;
        let value = entry
            .get_password()
            .map_err(|err| anyhow::anyhow!("failed to read keyring entry: {}", err))?;

        if value.trim().is_empty() {
            anyhow::bail!("keyring entry for {} resolved to empty value", field_name);
        }

        return Ok(value);
    }

    std::env::var(secret_ref)
        .with_context(|| format!("missing environment variable {}", secret_ref))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentConfig, AppConfig, FlowConfig, PhaseConfig, RuntimeConfig};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn build_agent_config(agent_type: &str) -> AgentConfig {
        AgentConfig {
            agent_type: agent_type.to_string(),
            url: Some("https://example.com".to_string()),
            chat_path: None,
            api_key_env: Some("TEST_API_KEY".to_string()),
            secret_key_env: Some("TEST_SECRET_KEY".to_string()),
            anthropic_version: Some("2023-06-01".to_string()),
            model: Some("test-model".to_string()),
            max_tokens: Some(1024),
            supports_system: Some(true),
        }
    }

    #[test]
    fn build_agent_registry_includes_known_agents() {
        let mut agents = HashMap::new();
        agents.insert("openai".to_string(), build_agent_config("openai"));
        agents.insert("qwen".to_string(), {
            let mut cfg = build_agent_config("qwen");
            cfg.url = None;
            cfg.model = None;
            cfg
        });
        agents.insert("qianfan".to_string(), build_agent_config("qianfan"));
        agents.insert("hunyuan".to_string(), build_agent_config("hunyuan"));

        let app_config = AppConfig {
            default_phase: "coding".to_string(),
            agents,
            flow: FlowConfig {
                name: "test".to_string(),
                phases: vec!["coding".to_string()],
            },
            phases: {
                let mut m = HashMap::new();
                m.insert(
                    "coding".to_string(),
                    PhaseConfig {
                        description: "coding".to_string(),
                        agents: vec![
                            "openai".to_string(),
                            "qwen".to_string(),
                            "qianfan".to_string(),
                            "hunyuan".to_string(),
                        ],
                        fallback: Some(true),
                        principles: None,
                        options: None,
                    },
                );
                m
            },
            runtime: Some(RuntimeConfig::default()),
            cache: None,
            vector: None,
            autotune: None,
            model_selection_mode: "adaptive".to_string(),
        };

        let registry = AgentRegistry::from_config(Arc::new(app_config), reqwest::Client::new());
        assert!(
            registry.is_ok(),
            "AgentRegistry should build all supported types"
        );
    }
}
