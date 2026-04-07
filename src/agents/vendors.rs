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
    #[allow(dead_code)]
    pub fn agent_types(&self) -> Vec<&'static str> {
        match self {
            VendorCategory::OpenAIFamily => {
                vec!["openai", "openai_compatible", "anthropic", "cohere"]
            }
            VendorCategory::ChineseVendors => vec![
                "deepseek", "wenxin", "qianfan", "qwen", "glm", "yi", "hunyuan", "doubao",
                "facewall", "langboat", "skywork", "stepfun", "xihu", "moonshot", "minimax",
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
            ],
        }
    }

    /// Get category description
    #[allow(dead_code)]
    pub fn description(&self) -> &'static str {
        match self {
            VendorCategory::OpenAIFamily => "OpenAI and compatible vendors",
            VendorCategory::ChineseVendors => "Chinese AI vendors",
            VendorCategory::OtherVendors => "Other AI vendors",
        }
    }
}

/// Vendor information
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct VendorInfo {
    /// Vendor name
    pub name: &'static str,
    /// Vendor category
    pub category: VendorCategory,
    /// Vendor description
    pub description: &'static str,
    /// Supported agent types
    pub supported_agents: Vec<&'static str>,
}

/// Get vendor information for an agent type
#[allow(dead_code)]
pub fn get_vendor_info(agent_type: &str) -> Option<VendorInfo> {
    let info = match agent_type {
        // OpenAI Family
        "openai" => VendorInfo {
            name: "OpenAI",
            category: VendorCategory::OpenAIFamily,
            description: "OpenAI models (GPT-4, GPT-3.5)",
            supported_agents: vec!["openai"],
        },
        "openai_compatible" => VendorInfo {
            name: "OpenAI Compatible",
            category: VendorCategory::OpenAIFamily,
            description: "OpenAI-compatible API endpoints",
            supported_agents: vec!["openai_compatible"],
        },
        "anthropic" => VendorInfo {
            name: "Anthropic",
            category: VendorCategory::OpenAIFamily,
            description: "Anthropic Claude models",
            supported_agents: vec!["anthropic"],
        },
        "cohere" => VendorInfo {
            name: "Cohere",
            category: VendorCategory::OpenAIFamily,
            description: "Cohere models",
            supported_agents: vec!["cohere"],
        },

        // Chinese Vendors
        "deepseek" => VendorInfo {
            name: "DeepSeek",
            category: VendorCategory::ChineseVendors,
            description: "DeepSeek models",
            supported_agents: vec!["deepseek"],
        },
        "wenxin" => VendorInfo {
            name: "Wenxin (Baidu)",
            category: VendorCategory::ChineseVendors,
            description: "Baidu Wenxin models",
            supported_agents: vec!["wenxin"],
        },
        "qianfan" => VendorInfo {
            name: "Qianfan (Baidu)",
            category: VendorCategory::ChineseVendors,
            description: "Baidu Qianfan platform",
            supported_agents: vec!["qianfan"],
        },
        "qwen" => VendorInfo {
            name: "Qwen (Alibaba)",
            category: VendorCategory::ChineseVendors,
            description: "Alibaba Qwen models",
            supported_agents: vec!["qwen"],
        },
        "glm" => VendorInfo {
            name: "GLM (Zhipu AI)",
            category: VendorCategory::ChineseVendors,
            description: "Zhipu AI GLM models",
            supported_agents: vec!["glm"],
        },
        "yi" => VendorInfo {
            name: "Yi (01.AI)",
            category: VendorCategory::ChineseVendors,
            description: "01.AI Yi models",
            supported_agents: vec!["yi"],
        },

        // Other vendors (simplified for brevity)
        "gemini" => VendorInfo {
            name: "Gemini (Google)",
            category: VendorCategory::OtherVendors,
            description: "Google Gemini models",
            supported_agents: vec!["gemini"],
        },
        "llama" => VendorInfo {
            name: "Llama (Meta)",
            category: VendorCategory::OtherVendors,
            description: "Meta Llama models",
            supported_agents: vec!["llama"],
        },
        "mistral" => VendorInfo {
            name: "Mistral AI",
            category: VendorCategory::OtherVendors,
            description: "Mistral AI models",
            supported_agents: vec!["mistral"],
        },
        _ => return None,
    };

    Some(info)
}

/// Get all vendor categories
#[allow(dead_code)]
pub fn get_all_categories() -> Vec<VendorCategory> {
    vec![
        VendorCategory::OpenAIFamily,
        VendorCategory::ChineseVendors,
        VendorCategory::OtherVendors,
    ]
}

/// Get agent types by category
#[allow(dead_code)]
pub fn get_agents_by_category(category: VendorCategory) -> Vec<&'static str> {
    category.agent_types()
}
