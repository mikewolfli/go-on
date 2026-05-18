//! Agent vendor categorization and organization
//!
//! This module provides organized access to agents by vendor/category.

/// Vendor categories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VendorCategory {
    /// OpenAI and compatible vendors
    OpenAIFamily,
    /// Chinese AI vendors
    ChineseVendors,
    /// Other vendors
    OtherVendors,
}

impl VendorCategory {
    /// Get all agents in this category
    pub fn agent_types(&self) -> Vec<&'static str> {
        match self {
            VendorCategory::OpenAIFamily => {
                vec!["openai", "openai_compatible", "anthropic", "cohere"]
            }
            VendorCategory::ChineseVendors => vec![
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
            ],
            VendorCategory::OtherVendors => vec![
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
            ],
        }
    }

    /// Get category description
    pub fn description(&self) -> &'static str {
        match self {
            VendorCategory::OpenAIFamily => "OpenAI and compatible vendors",
            VendorCategory::ChineseVendors => "Chinese AI vendors",
            VendorCategory::OtherVendors => "Other AI vendors",
        }
    }
}
