// Generated provider catalog — DO NOT EDIT.
//
// Regenerate with: python3 scripts/gen-provider-catalog.py
//
// Mirrors the backend's `built_in_provider_specs()` in
// `src/core/providers.rs` (the single source of truth). This is the
// GUI's offline fallback used before the backend is reachable; at
// runtime the `provider.catalog` RPC is the authoritative source.

use super::catalog::ProviderSpec;

/// Provider names in backend order (single source of truth for the
/// GUI's offline provider dropdown / keyring sync / config checks).
pub const PROVIDER_NAMES: &[&str] = &[
    "openai",
    "openai_compatible",
    "anthropic",
    "cohere",
    "deepseek",
    "wenxin",
    "qianfan",
    "qwen",
    "glm",
    "yi",
    "hunyuan",
    "doubao",
    "facewall",
    "langboat",
    "skywork",
    "stepfun",
    "xihu",
    "moonshot",
    "minimax",
    "kimi",
    "siliconflow",
    "ai21",
    "aleph",
    "copilot",
    "deepquest",
    "fireworks",
    "gemini",
    "groq",
    "llama",
    "loopai",
    "mistral",
    "nim",
    "perplexity",
    "replicate",
    "titan",
    "together",
    "xai",
];

/// Offline provider spec lookup (backend defaults). Unknown names fall
/// back to the generic openai_compatible shape.
pub fn built_in_provider_specs(name: &str) -> ProviderSpec {
    match name {
        "openai" => ProviderSpec {
            agent_type: "openai",
            default_url: Some("https://api.openai.com/v1"),
            default_model: "gpt-4o-mini",
            supports_system: true,
        },
        "openai_compatible" => ProviderSpec {
            agent_type: "openai_compatible",
            default_url: Some("http://127.0.0.1:8080/v1"),
            default_model: "compatible-model",
            supports_system: true,
        },
        "anthropic" => ProviderSpec {
            agent_type: "claude",
            default_url: Some("https://api.anthropic.com"),
            default_model: "claude-sonnet-4-20250514",
            supports_system: true,
        },
        "cohere" => ProviderSpec {
            agent_type: "cohere",
            default_url: Some("https://api.cohere.ai/v1"),
            default_model: "command-r-plus-08-2024",
            supports_system: true,
        },
        "deepseek" => ProviderSpec {
            agent_type: "deepseek",
            default_url: Some("https://api.deepseek.com"),
            default_model: "deepseek-v4-flash",
            supports_system: true,
        },
        "wenxin" => ProviderSpec {
            agent_type: "wenxin",
            default_url: None,
            default_model: "ERNIE-4.5-8K",
            supports_system: false,
        },
        "qianfan" => ProviderSpec {
            agent_type: "qianfan",
            default_url: None,
            default_model: "ERNIE-4.5-8K",
            supports_system: false,
        },
        "qwen" => ProviderSpec {
            agent_type: "qwen",
            default_url: Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            default_model: "qwen-max",
            supports_system: true,
        },
        "glm" => ProviderSpec {
            agent_type: "glm",
            default_url: Some("https://open.bigmodel.cn/api/paas/v4"),
            default_model: "glm-4-flash",
            supports_system: false,
        },
        "yi" => ProviderSpec {
            agent_type: "yi",
            default_url: Some("https://api.lingyiwanwu.com/v1"),
            default_model: "yi-lightning",
            supports_system: false,
        },
        "hunyuan" => ProviderSpec {
            agent_type: "hunyuan",
            default_url: Some("https://api.hunyuan.cloud.tencent.com/v1"),
            default_model: "hunyuan-turbo-latest",
            supports_system: false,
        },
        "doubao" => ProviderSpec {
            agent_type: "doubao",
            default_url: Some("https://ark.cn-beijing.volces.com/api/v3"),
            default_model: "doubao-1.5-pro-256k-250115",
            supports_system: true,
        },
        "facewall" => ProviderSpec {
            agent_type: "facewall",
            default_url: Some("https://api.facewall.ai/v1"),
            default_model: "facewall-chat",
            supports_system: false,
        },
        "langboat" => ProviderSpec {
            agent_type: "langboat",
            default_url: Some("https://api.langboat.com/v1"),
            default_model: "langboat-chat",
            supports_system: false,
        },
        "skywork" => ProviderSpec {
            agent_type: "skywork",
            default_url: Some("https://api.skywork.ai/v1"),
            default_model: "skywork-chat",
            supports_system: false,
        },
        "stepfun" => ProviderSpec {
            agent_type: "stepfun",
            default_url: Some("https://api.stepfun.com/v1"),
            default_model: "step-2-16k",
            supports_system: false,
        },
        "xihu" => ProviderSpec {
            agent_type: "xihu",
            default_url: Some("https://api.xihu.ai/v1"),
            default_model: "xihu-chat",
            supports_system: false,
        },
        "moonshot" => ProviderSpec {
            agent_type: "moonshot",
            default_url: Some("https://api.moonshot.cn/v1"),
            default_model: "moonshot-v1-8k",
            supports_system: false,
        },
        "minimax" => ProviderSpec {
            agent_type: "minimax",
            default_url: Some("https://api.minimax.chat/v1"),
            default_model: "MiniMax-Text-01",
            supports_system: false,
        },
        "kimi" => ProviderSpec {
            agent_type: "kimi",
            default_url: Some("https://api.moonshot.cn/v1"),
            default_model: "kimi-k2.6",
            supports_system: false,
        },
        "siliconflow" => ProviderSpec {
            agent_type: "openai_compatible",
            default_url: Some("https://api.siliconflow.cn/v1"),
            default_model: "deepseek-ai/DeepSeek-V3.2",
            supports_system: true,
        },
        "ai21" => ProviderSpec {
            agent_type: "ai21",
            default_url: Some("https://api.ai21.com/studio/v1"),
            default_model: "jamba-1.5-mini",
            supports_system: false,
        },
        "aleph" => ProviderSpec {
            agent_type: "aleph",
            default_url: Some("https://api.aleph-alpha.com"),
            default_model: "luminous-base",
            supports_system: false,
        },
        "copilot" => ProviderSpec {
            agent_type: "copilot",
            default_url: Some("https://api.githubcopilot.com"),
            default_model: "auto",
            supports_system: false,
        },
        "deepquest" => ProviderSpec {
            agent_type: "deepquest",
            default_url: Some("https://api.deepquest.ai/v1"),
            default_model: "deepquest-chat",
            supports_system: false,
        },
        "fireworks" => ProviderSpec {
            agent_type: "fireworks",
            default_url: Some("https://api.fireworks.ai/inference/v1"),
            default_model: "accounts/fireworks/models/llama-v3p1-8b-instruct",
            supports_system: false,
        },
        "gemini" => ProviderSpec {
            agent_type: "gemini",
            default_url: Some("https://generativelanguage.googleapis.com/v1beta"),
            default_model: "gemini-2.5-flash",
            supports_system: false,
        },
        "groq" => ProviderSpec {
            agent_type: "groq",
            default_url: Some("https://api.groq.com/openai/v1"),
            default_model: "llama-3.3-70b-versatile",
            supports_system: false,
        },
        "llama" => ProviderSpec {
            agent_type: "llama",
            default_url: Some("http://127.0.0.1:11434/v1"),
            default_model: "llama3.2",
            supports_system: true,
        },
        "loopai" => ProviderSpec {
            agent_type: "loopai",
            default_url: Some("https://api.loopai.com/v1"),
            default_model: "loopai-chat",
            supports_system: false,
        },
        "mistral" => ProviderSpec {
            agent_type: "mistral",
            default_url: Some("https://api.mistral.ai/v1"),
            default_model: "mistral-small-latest",
            supports_system: false,
        },
        "nim" => ProviderSpec {
            agent_type: "nim",
            default_url: Some("https://integrate.api.nvidia.com/v1"),
            default_model: "meta/llama-3.1-70b-instruct",
            supports_system: false,
        },
        "perplexity" => ProviderSpec {
            agent_type: "perplexity",
            default_url: Some("https://api.perplexity.ai"),
            default_model: "sonar-pro",
            supports_system: false,
        },
        "replicate" => ProviderSpec {
            agent_type: "replicate",
            default_url: Some("https://api.replicate.com/v1"),
            default_model: "meta/meta-llama-3-70b-instruct",
            supports_system: false,
        },
        "titan" => ProviderSpec {
            agent_type: "titan",
            default_url: Some("https://api.titanml.co/v1"),
            default_model: "titan-chat",
            supports_system: false,
        },
        "together" => ProviderSpec {
            agent_type: "together",
            default_url: Some("https://api.together.xyz/v1"),
            default_model: "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo",
            supports_system: false,
        },
        "xai" => ProviderSpec {
            agent_type: "openai_compatible",
            default_url: Some("https://api.x.ai/v1"),
            default_model: "grok-3",
            supports_system: true,
        },
        _ => ProviderSpec {
            agent_type: "openai_compatible",
            default_url: None,
            default_model: "auto",
            supports_system: false,
        },
    }
}

/// Offline default-model suggestions per provider (synced with the
/// backend `ProviderSpec::model_suggestions` field). Empty for
/// providers without curated suggestions. The UI-only `auto`
/// entry is not included — callers prepend it themselves.
pub fn default_models(name: &str) -> &'static [&'static str] {
    match name {
        "openai" => &["gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano", "o3-mini", "gpt-4o", "gpt-4o-mini", "gpt-4-turbo", "gpt-3.5-turbo"],
        "openai_compatible" => &[],
        "anthropic" => &["claude-opus-4-7", "claude-sonnet-4-6", "claude-haiku-4-5-20251001", "claude-3-5-sonnet", "claude-3-opus", "claude-3-haiku"],
        "cohere" => &["command-a-03-2025", "command-a-reasoning-08-2025", "command-r7b-12-2024", "command-r-plus-08-2024", "command-r-08-2024"],
        "deepseek" => &["deepseek-v4-flash", "deepseek-v4-pro", "deepseek-r1"],
        "wenxin" => &["ERNIE-4.5-8K", "ernie-4.0-turbo-8k", "ernie-3.5-turbo"],
        "qianfan" => &["ERNIE-4.5-8K", "ernie-4.0-8k", "ernie-3.5-8k", "ernie-speed", "ernie-lite"],
        "qwen" => &["qwen-max", "qwen-plus", "qwen-turbo", "qwen2.5-72b-instruct"],
        "glm" => &["glm-4-flash", "glm-4v", "glm-4-plus", "glm-3-turbo"],
        "yi" => &["yi-lightning", "yi-large"],
        "hunyuan" => &["hunyuan-turbo-latest", "hunyuan-turbo", "hunyuan-pro"],
        "doubao" => &["doubao-1.5-pro-32k-250115"],
        "facewall" => &["facewall-chat", "facewall-chat-large"],
        "langboat" => &["langboat-chat", "langboat-chat-large"],
        "skywork" => &["skywork-chat", "skywork-chat-large"],
        "stepfun" => &["step-2-16k", "step-1-8k", "step-1-flash"],
        "xihu" => &["xihu-chat", "xihu-chat-large"],
        "moonshot" => &["moonshot-v1-8k", "moonshot-v1-32k", "moonshot-v1-128k"],
        "minimax" => &["MiniMax-Text-01", "MiniMax-Text-01-mini"],
        "kimi" => &["kimi-k2.6", "kimi-k2.5", "kimi-k2", "kimi-k2-thinking", "moonshot-v1"],
        "siliconflow" => &["deepseek-ai/DeepSeek-V3.2", "deepseek-ai/DeepSeek-R1", "deepseek-ai/DeepSeek-V2.5", "Qwen/Qwen2.5-72B-Instruct-128K", "Qwen/Qwen2.5-32B-Instruct", "Qwen/QwQ-32B", "TeleAI/TeleChat-T2", "THUDM/glm-4-9b-chat", "internlm/internlm2_5-20b-chat"],
        "ai21" => &["jamba-1.5-mini", "jamba-1.5-large"],
        "aleph" => &["luminous-base", "luminous-extended"],
        "copilot" => &["claude-opus-4", "claude-sonnet-4", "gemini-2.5-pro", "gpt-5", "gpt-4.1", "gpt-4o", "o1", "o3-mini", "gpt-5-mini", "gpt-4.1-mini", "gpt-4o-mini", "claude-3.5-sonnet", "gemini-2.0-flash-001"],
        "deepquest" => &["deepquest-chat", "deepquest-chat-large"],
        "fireworks" => &["accounts/fireworks/models/llama-v3p1-8b-instruct", "accounts/fireworks/models/llama-v3p1-405b-instruct", "accounts/fireworks/models/mixtral-8x22b-instruct"],
        "gemini" => &["gemini-2.5-flash", "gemini-2.5-flash-lite", "gemini-2.5-pro", "gemini-3.1-pro-preview-03-2026", "gemini-3-flash-preview-03-2026", "gemini-2.0-flash", "gemini-2.0-pro"],
        "groq" => &["llama-3.3-70b-versatile", "llama-3.1-8b-instant", "openai/gpt-oss-120b", "qwen/qwen3-32b"],
        "llama" => &["llama3.2", "llama3.2-vision"],
        "loopai" => &["loopai-chat", "loopai-chat-pro"],
        "mistral" => &["mistral-large-2512", "mistral-medium-2508", "mistral-small-2603"],
        "nim" => &["meta/llama-3.1-70b-instruct", "meta/llama-3.1-405b-instruct", "mistralai/mixtral-8x22b-instruct"],
        "perplexity" => &["sonar-pro", "sonar", "sonar-reasoning-pro", "sonar-deep-research"],
        "replicate" => &["meta/meta-llama-3-70b-instruct", "meta/meta-llama-3-8b-instruct"],
        "titan" => &["amazon.titan-text-premier-v1:0", "amazon.titan-text-express-v1"],
        "together" => &["meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo", "meta-llama/Meta-Llama-3.1-405B-Instruct-Turbo", "mistralai/Mixtral-8x22B-Instruct-v0.1"],
        "xai" => &["grok-2", "grok-3"],
        _ => &[],
    }
}
