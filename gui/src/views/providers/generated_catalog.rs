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
