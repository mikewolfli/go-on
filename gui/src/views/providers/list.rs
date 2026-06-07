//! Provider listing sub-module.
//!
//! Handles the display of existing (saved) providers in the providers view.
//! Extracted from the monolithic providers.rs for better organization.
//!
//! TODO (BLUE65): Migrate the saved-providers rendering logic from mod.rs into this module.

/// Provider names for the dropdown (36 total, matching built_in_provider_specs())
/// This is the CANONICAL source of provider names used throughout the codebase.
/// Keep in sync with `src/core/config.rs` built_in_provider_specs().
pub const PROVIDER_NAMES: &[&str] = &[
    // OpenAI Family (4)
    "openai",
    "openai_compatible",
    "anthropic",
    "cohere",
    // Chinese Vendors (16)
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
    "siliconflow",
    // Other Vendors (16)
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

/// Model suggestions per provider (GUI-side hardcoded).
/// These match the model IDs returned by backend agents' `available_models()`.
pub(crate) fn models_for_provider(provider: &str) -> &'static [&'static str] {
    match provider.to_lowercase().as_str() {
        "deepseek" => &[
            "auto",
            "deepseek-v4-flash",
            "deepseek-v4-pro",
            "deepseek-r1",
        ],
        "openai" => &[
            "auto",
            "gpt-4.1",
            "gpt-4.1-mini",
            "gpt-4.1-nano",
            "o3-mini",
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4-turbo",
            "gpt-3.5-turbo",
        ],
        "openai_compatible" => &["auto"],
        "anthropic" => &[
            "auto",
            "claude-opus-4-7",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
            "claude-3-5-sonnet",
            "claude-3-opus",
            "claude-3-haiku",
        ],
        "copilot" => &[
            "auto",
            "claude-opus-4",
            "claude-sonnet-4",
            "gemini-2.5-pro",
            "gpt-5",
            "gpt-4.1",
            "gpt-4o",
            "o1",
            "o3-mini",
            "gpt-5-mini",
            "gpt-4.1-mini",
            "gpt-4o-mini",
            "claude-3.5-sonnet",
            "gemini-2.0-flash-001",
        ],
        "gemini" => &[
            "auto",
            "gemini-2.5-flash",
            "gemini-2.5-flash-lite",
            "gemini-2.5-pro",
            "gemini-3.1-pro-preview-03-2026",
            "gemini-3-flash-preview-03-2026",
        ],
        _ => &["auto"],
    }
}
