//! Tool trait and tool runtime for go-on
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Tool trait, registry, and implementations will be connected to the execution flow
//! once orchestration logic integrates them.

// ── Sub-modules (moved from orchestration/ for cohesion) ───────────────────
pub mod extended;
pub mod lock;
pub mod native;
pub mod pipeline;
pub mod recommender;
use crate::governance::pua::{tool_execution_report, PuaExecutionReport};
use crate::i18n::runtime::{t, tf};
use anyhow::Result;
use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use crate::orchestration::skill::SkillRegistry;
use crate::orchestration::tool::lock::{LockMode, ToolLockManager as TLM};

/// Global tool lock manager for file access synchronization.
static TOOL_LOCK_MANAGER: OnceLock<TLM> = OnceLock::new();

fn tool_lock_manager() -> &'static TLM {
    TOOL_LOCK_MANAGER.get_or_init(TLM::new)
}

/// Global skill registry reference for tools that need access to registered skills.
static SKILL_REGISTRY: OnceLock<Arc<RwLock<SkillRegistry>>> = OnceLock::new();

/// Get the global skill registry reference, if set.
pub fn skill_registry() -> Option<&'static Arc<RwLock<SkillRegistry>>> {
    SKILL_REGISTRY.get()
}

/// Set the global skill registry reference used by `SkillListTool` and other
/// registry-aware tools. Call this once during server startup after the skill
/// registry has been initialized.
pub fn set_skill_registry(registry: Arc<RwLock<SkillRegistry>>) {
    if SKILL_REGISTRY.set(registry).is_err() {
        tracing::warn!(
            target: "tool",
            "set_skill_registry called more than once — ignoring duplicate"
        );
    }
}

use std::process::Command;
use std::time::Instant;
use tracing::{debug, info, warn};

/// Tool input envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInput {
    pub task_id: String,
    pub phase: String,
    pub agent_role: String,
    pub objective: String,
    pub constraints: Option<String>,
    pub evidence: Option<String>,
    pub payload: serde_json::Value,
    pub allowed_base_dir: Option<PathBuf>,
}

/// Tool output envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub verification: Option<String>,
    pub audit_log: Option<String>,
    pub pua_report: Option<PuaExecutionReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicy {
    pub max_retries: usize,
    pub retry_on_failure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCapabilityProfile {
    pub capability: String,
    pub risk_level: ToolRiskLevel,
    pub timeout_budget_ms: u64,
    pub retry_policy: RetryPolicy,
    pub fallback_chain: Vec<String>,
}

/// Tool trait
///
/// All tools must implement this trait. The `run` method should be instrumented for tracing and performance monitoring in the implementation, not on the trait itself.
pub trait Tool: Send + Sync + 'static {
    /// Returns the tool's unique name.
    fn name(&self) -> &'static str;

    /// Returns a human-readable description of what this tool does.
    /// Override this to provide rich descriptions for LLM function-calling schemas.
    fn description(&self) -> &str {
        ""
    }

    /// Returns the JSON Schema for this tool's input parameters.
    /// Used when building OpenAI/Anthropic-compatible function-calling schemas.
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    /// Executes the tool with the given input. Should emit tracing spans for performance analysis (implementations only).
    fn run(&self, input: &ToolInput) -> Result<ToolOutput>;

    /// Async variant of `run` for non-blocking execution in async contexts.
    /// The default implementation offloads the synchronous `run` call to
    /// `tokio::task::spawn_blocking`, which moves the work off the async
    /// runtime worker thread and onto the blocking thread pool.
    ///
    /// I/O-bound tools SHOULD override this method with a fully async
    /// implementation for optimal performance.
    fn run_async(
        self: std::sync::Arc<Self>,
        input: ToolInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || self.run(&input))
                .await
                .map_err(|e| anyhow::anyhow!("tool blocking task failed: {}", e))?
        })
    }
}

/// Tool registry
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
    profiles: HashMap<&'static str, ToolCapabilityProfile>,
    /// Alias map: alias → canonical tool name.
    /// Allows looking up tools by alternative names (e.g. "terminal" → "shell_exec").
    aliases: HashMap<&'static str, &'static str>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tool_names: Vec<&str> = self.tools.iter().map(|t| t.name()).collect();
        f.debug_struct("ToolRegistry")
            .field("tools", &tool_names)
            .field("profiles", &self.profiles)
            .field("aliases", &self.aliases)
            .finish()
    }
}

impl ToolRegistry {
    /// Create an empty tool registry (no built-in tools registered).
    pub fn new_empty() -> Self {
        Self {
            tools: Vec::new(),
            profiles: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    /// Create a new tool registry and register all built-in tools.
    #[tracing::instrument(level = "info")]
    pub fn new() -> Self {
        let mut registry = Self::new_empty();
        registry.register_with_profile(
            ReadFileTool,
            ToolCapabilityProfile {
                capability: "filesystem_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["search_files".to_string()],
            },
        );
        registry.register_with_profile(
            WriteFileTool,
            ToolCapabilityProfile {
                capability: "filesystem_write".to_string(),
                risk_level: ToolRiskLevel::High,
                timeout_budget_ms: 15_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            SearchFilesTool,
            ToolCapabilityProfile {
                capability: "filesystem_search".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["read_file".to_string()],
            },
        );
        registry.register_with_profile(
            ApplyPatchTool,
            ToolCapabilityProfile {
                capability: "patch_apply".to_string(),
                risk_level: ToolRiskLevel::High,
                timeout_budget_ms: 20_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: vec!["inspect_git_diff".to_string()],
            },
        );
        registry.register_with_profile(
            RunTestsTool,
            ToolCapabilityProfile {
                capability: "verification_execute".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["inspect_git_diff".to_string()],
            },
        );
        registry.register_with_profile(
            InspectGitDiffTool,
            ToolCapabilityProfile {
                capability: "scm_diff".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 8_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        // ── Extended tools ───────────────────────────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::ShellExecTool,
            ToolCapabilityProfile {
                capability: "shell_execution".to_string(),
                risk_level: ToolRiskLevel::High,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::HttpRequestTool,
            ToolCapabilityProfile {
                capability: "http_request".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::GrepTool,
            ToolCapabilityProfile {
                capability: "content_search".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 15_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["search_files".to_string()],
            },
        );

        registry.register_with_profile(
            crate::orchestration::tool_extended::GitTool,
            ToolCapabilityProfile {
                capability: "version_control".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 15_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["inspect_git_diff".to_string()],
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::ListDirectoryTool,
            ToolCapabilityProfile {
                capability: "directory_listing".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 5_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::CargoCheckTool,
            ToolCapabilityProfile {
                capability: "compilation_check".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 120_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::CargoTestTool,
            ToolCapabilityProfile {
                capability: "test_execution".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["cargo_check".to_string()],
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::FileMoveTool,
            ToolCapabilityProfile {
                capability: "filesystem_move".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::FileDeleteTool,
            ToolCapabilityProfile {
                capability: "filesystem_delete".to_string(),
                risk_level: ToolRiskLevel::High,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::CreateDirectoryTool,
            ToolCapabilityProfile {
                capability: "filesystem_create_directory".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::CopyPathTool,
            ToolCapabilityProfile {
                capability: "filesystem_copy".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["file_move".to_string()],
            },
        );

        // ── Office document tools (feature-gated) ──────────────────
        #[cfg(feature = "document-excel")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::ReadExcelTool,
            ToolCapabilityProfile {
                capability: "document_excel_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        #[cfg(feature = "document-ppt")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::ReadPptTool,
            ToolCapabilityProfile {
                capability: "document_ppt_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        #[cfg(feature = "document-excel-write")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::WriteExcelTool,
            ToolCapabilityProfile {
                capability: "document_excel_write".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Archive tools (no feature gate) ──────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::ArchiveInspectTool,
            ToolCapabilityProfile {
                capability: "archive_inspect".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::ArchiveExtractTool,
            ToolCapabilityProfile {
                capability: "archive_extract".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── PDF document tools (feature-gated) ────────────────
        #[cfg(feature = "document-pdf")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::ReadPdfTool,
            ToolCapabilityProfile {
                capability: "document_pdf_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        #[cfg(feature = "document-pdf")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::PdfMergeTool,
            ToolCapabilityProfile {
                capability: "document_pdf_merge".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "document-pdf")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::PdfSplitTool,
            ToolCapabilityProfile {
                capability: "document_pdf_split".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        #[cfg(feature = "document-email")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::EmailParseTool,
            ToolCapabilityProfile {
                capability: "document_email_parse".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 15_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── DOCX document tools (feature-gated) ──────────────
        #[cfg(feature = "document-docx")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::ReadDocxTool,
            ToolCapabilityProfile {
                capability: "document_docx_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "document-docx")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::WriteDocxTool,
            ToolCapabilityProfile {
                capability: "docx_write".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "document-ppt")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::WritePptTool,
            ToolCapabilityProfile {
                capability: "ppt_write".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── SQLite query tools (feature-gated) ───────────────
        #[cfg(feature = "backend-sqlite")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::SqliteQueryTool,
            ToolCapabilityProfile {
                capability: "sqlite_query".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Web scraping tools (feature-gated) ───────────────
        #[cfg(feature = "document-html")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::WebScrapeTool,
            ToolCapabilityProfile {
                capability: "web_scrape".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Data serialization tools (feature-gated) ─────────
        #[cfg(feature = "data-export")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::CsvReadTool,
            ToolCapabilityProfile {
                capability: "csv_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "data-export")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::CsvWriteTool,
            ToolCapabilityProfile {
                capability: "csv_write".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "data-export")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::CsvAnalyzeTool,
            ToolCapabilityProfile {
                capability: "csv_analyze".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "data-export")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::CsvTransformTool,
            ToolCapabilityProfile {
                capability: "csv_transform".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "data-export")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::TomlReadTool,
            ToolCapabilityProfile {
                capability: "toml_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "data-export")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::TomlWriteTool,
            ToolCapabilityProfile {
                capability: "toml_write".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "data-export")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::YamlReadTool,
            ToolCapabilityProfile {
                capability: "yaml_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "data-export")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::YamlWriteTool,
            ToolCapabilityProfile {
                capability: "yaml_write".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Image processing tools (feature-gated) ────────────
        #[cfg(feature = "image-processing")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::ImageResizeTool,
            ToolCapabilityProfile {
                capability: "image_resize".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "image-processing")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::ImageConvertTool,
            ToolCapabilityProfile {
                capability: "image_convert".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "image-processing")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::ImageAnalyzeTool,
            ToolCapabilityProfile {
                capability: "image_analyze".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "image-processing")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::ImageGenerateTool,
            ToolCapabilityProfile {
                capability: "image_generate".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 120_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Compression tools (no feature gate) ───────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::CompressTool,
            ToolCapabilityProfile {
                capability: "compress".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::DecompressTool,
            ToolCapabilityProfile {
                capability: "decompress".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── CAD/DXF tools (feature-gated) ─────────────────────
        #[cfg(feature = "cad-dxf")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::DxfReadTool,
            ToolCapabilityProfile {
                capability: "dxf_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── SVG drawing tools (feature-gated) ─────────────────
        #[cfg(feature = "drawing-svg")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::SvgReadTool,
            ToolCapabilityProfile {
                capability: "svg_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "drawing-svg")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::SvgGenerateTool,
            ToolCapabilityProfile {
                capability: "svg_generate".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── CAD/STL tools (feature-gated) ────────────────────
        #[cfg(all(feature = "cad-stl", not(feature = "model-3d")))]
        registry.register_with_profile(
            crate::orchestration::tool_extended::StlReadTool,
            ToolCapabilityProfile {
                capability: "stl_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── CAD/OBJ tools (feature-gated) ────────────────────
        #[cfg(feature = "cad-obj")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::ObjReadTool,
            ToolCapabilityProfile {
                capability: "obj_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── CAD/STEP tools (feature-gated) ────────────────────
        #[cfg(feature = "cad-step")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::StepReadTool,
            ToolCapabilityProfile {
                capability: "step_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── CAD/Geo utilities (feature-gated) ────────────────────
        #[cfg(feature = "cad-geo")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::GeoUtilTool,
            ToolCapabilityProfile {
                capability: "geo_util".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 5_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── CAD utilities (feature-gated) ────────────────────
        #[cfg(feature = "cad-utils")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::CadConvertTool,
            ToolCapabilityProfile {
                capability: "cad_convert".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 5_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── SVG export (feature-gated) ───────────────────────
        #[cfg(feature = "drawing-svg")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::SvgExportTool,
            ToolCapabilityProfile {
                capability: "svg_export".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── glTF 3D model tools (feature-gated) ────────────────
        #[cfg(feature = "cad-gltf")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::GltfReadTool,
            ToolCapabilityProfile {
                capability: "gltf_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── IGES CAD tools (feature-gated) ──────────────────────
        #[cfg(feature = "cad-iges")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::IgesReadTool,
            ToolCapabilityProfile {
                capability: "iges_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── PLY 3D mesh tools (feature-gated) ───────────────────
        #[cfg(feature = "cad-ply")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::PlyReadTool,
            ToolCapabilityProfile {
                capability: "ply_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── STL generate tool (feature-gated) ───────────────────
        #[cfg(feature = "cad-stl")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::StlGenerateTool,
            ToolCapabilityProfile {
                capability: "stl_generate".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Invoice parsing tool (feature-gated) ────────────
        #[cfg(feature = "document-invoice")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::InvoiceParseTool,
            ToolCapabilityProfile {
                capability: "document_invoice_parse".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 15_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── QR Code generation tool (feature-gated) ──────────
        #[cfg(feature = "barcode-tools")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::QrCodeTool,
            ToolCapabilityProfile {
                capability: "barcode_qrcode_generate".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 15_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── RSS feed reader tool (no feature gate) ────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::RssReadTool,
            ToolCapabilityProfile {
                capability: "rss_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── 3D model (STL) reader tool (feature-gated) ────────────
        #[cfg(feature = "model-3d")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::StlReadTool,
            ToolCapabilityProfile {
                capability: "stl_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── CAM/G-code reader tool (feature-gated) ────────────────
        #[cfg(feature = "cam-gcode")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::GcodeReadTool,
            ToolCapabilityProfile {
                capability: "gcode_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── GIS/GPX reader tool (feature-gated) ────────────────────
        #[cfg(feature = "gis-gpx")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::GpxReadTool,
            ToolCapabilityProfile {
                capability: "gpx_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── 3D model (OBJ) reader tool (feature-gated) ────────────────
        #[cfg(feature = "model-3d-extra")]
        registry.register_with_profile(
            crate::orchestration::tool_extended::ObjModelReadTool,
            ToolCapabilityProfile {
                capability: "obj_model_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── JSON Lines read tool (no feature gate) ────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::JsonlReadTool,
            ToolCapabilityProfile {
                capability: "jsonl_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── JSON Lines write tool (no feature gate) ───────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::JsonlWriteTool,
            ToolCapabilityProfile {
                capability: "jsonl_write".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Network tools ─────────────────────────────────────────--
        registry.register_with_profile(
            crate::orchestration::tool_extended::DnsLookupTool,
            ToolCapabilityProfile {
                capability: "dns_lookup".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::PingTool,
            ToolCapabilityProfile {
                capability: "network_ping".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::PortScanTool,
            ToolCapabilityProfile {
                capability: "port_scan".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Date/Time tools ─────────────────────────────────────────--
        registry.register_with_profile(
            crate::orchestration::tool_extended::DateTimeTool,
            ToolCapabilityProfile {
                capability: "date_time".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 5_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Diagnostics tool (no feature gate) ──────────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::DiagnosticsTool,
            ToolCapabilityProfile {
                capability: "project_diagnostics".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Environment info tool (no feature gate) ─────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::EnvironmentInfoTool,
            ToolCapabilityProfile {
                capability: "environment_discovery".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Game tools (feature-gated) ───────────────────────────────
        #[cfg(any(
            feature = "game-online",
            feature = "game-process",
            feature = "game-screen",
            feature = "game-input",
            feature = "game-agent",
            feature = "game-state",
            feature = "game-modding"
        ))]
        crate::orchestration::tool::extended::game::register_game_tools(&mut registry);

        // ── Skill listing tool (always compiled, no feature gate) ────
        registry.register_with_profile(
            SkillListTool,
            ToolCapabilityProfile {
                capability: "skill_list".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 5_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Skill execution tool (always compiled, no feature gate) ────
        registry.register_with_profile(
            SkillExecuteTool,
            ToolCapabilityProfile {
                capability: "skill_execute".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 120_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Skill creation tool (always compiled, no feature gate) ────
        registry.register_with_profile(
            SkillCreateTool,
            ToolCapabilityProfile {
                capability: "skill_create".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Skill reload tool (always compiled, no feature gate) ────
        registry.register_with_profile(
            SkillReloadTool,
            ToolCapabilityProfile {
                capability: "skill_reload".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Code index / semantic search tool ───────────────────
        // Provides workspace-wide code symbol indexing and ranked semantic
        // search across multiple programming languages.
        registry.register_with_profile(
            crate::orchestration::tool_extended::CodeIndexTool,
            ToolCapabilityProfile {
                capability: "code_index_search".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["grep".to_string()],
            },
        );

        // ── File diff tool ───────────────────────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::DiffTool,
            ToolCapabilityProfile {
                capability: "file_diff".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 15_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Read file lines tool ────────────────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::ReadFileLinesTool,
            ToolCapabilityProfile {
                capability: "file_read_lines".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["read_file".to_string()],
            },
        );

        // ── Backward-compatibility aliases ───────────────────────
        // These names exist in the governance evaluator's allowlist.
        // Some now have their own Tool implementations; others alias
        // to existing tools with the same functionality.
        // create_directory and copy_path have dedicated implementations above.
        registry.register_alias("delete_path", "file_delete");
        registry.register_alias("move_path", "file_move");
        registry.register_alias("execute_command", "shell_exec");
        registry.register_alias("terminal", "shell_exec");
        registry.register_alias("bash", "shell_exec");
        registry.register_alias("find_path", "find_files");
        registry.register_alias("semantic_search", "code_index_search");

        registry
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.register_with_profile(
            tool,
            ToolCapabilityProfile {
                capability: "custom".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
    }

    pub fn register_with_profile<T: Tool + 'static>(
        &mut self,
        tool: T,
        profile: ToolCapabilityProfile,
    ) {
        let name = tool.name();
        self.profiles.insert(name, profile);
        self.tools.push(Arc::new(tool));
    }

    /// Get a tool by name (with alias resolution).
    #[tracing::instrument(level = "debug", skip(self))]
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        // Direct lookup first
        if let Some(tool) = self.tools.iter().find(|t| t.name() == name) {
            return Some(tool.as_ref());
        }
        // Alias resolution: look up the canonical name and find that tool
        if let Some(&canonical) = self.aliases.get(name) {
            self.tools
                .iter()
                .find(|t| t.name() == canonical)
                .map(|b| b.as_ref())
        } else {
            None
        }
    }

    /// Get a tool by name (with alias resolution), returning an `Arc` for async usage.
    /// The returned `Arc` can be used to call `run_async` on the tool.
    pub fn get_arc(&self, name: &str) -> Option<Arc<dyn Tool>> {
        // Direct lookup first
        if let Some(tool) = self.tools.iter().find(|t| t.name() == name) {
            return Some(Arc::clone(tool));
        }
        // Alias resolution: look up the canonical name and find that tool
        if let Some(&canonical) = self.aliases.get(name) {
            self.tools
                .iter()
                .find(|t| t.name() == canonical)
                .map(Arc::clone)
        } else {
            None
        }
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|tool| tool.name()).collect()
    }

    /// Return all tool names including aliases.
    pub fn all_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&str> = self.tools.iter().map(|tool| tool.name()).collect();
        names.extend(self.aliases.keys().copied());
        names
    }

    /// Register an alias for a tool. When `alias` is looked up via `get()`,
    /// the tool registered under `canonical` name will be returned.
    ///
    /// This enables backward compatibility with legacy tool names that exist
    /// in the governance evaluator allowlist (e.g. "terminal" → "shell_exec").
    pub fn register_alias(&mut self, alias: &'static str, canonical: &'static str) {
        self.aliases.insert(alias, canonical);
    }

    /// Get the profile for a tool by name (with alias resolution).
    /// If `name` is an alias, returns the canonical tool's profile.
    pub fn profile(&self, name: &str) -> Option<&ToolCapabilityProfile> {
        let canonical = self.aliases.get(name).copied().unwrap_or(name);
        self.profiles.get(canonical)
    }

    pub fn capability_matrix(&self) -> serde_json::Value {
        let matrix = self
            .tools
            .iter()
            .filter_map(|tool| {
                self.profiles.get(tool.name()).map(|profile| {
                    serde_json::json!({
                        "name": tool.name(),
                        "capability": profile.capability,
                        "risk_level": profile.risk_level,
                        "timeout_budget_ms": profile.timeout_budget_ms,
                        "retry_policy": profile.retry_policy,
                        "fallback_chain": profile.fallback_chain,
                    })
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({ "tools": matrix })
    }

    #[tracing::instrument(level = "debug", skip(self, input), fields(tool = %name, success = false, latency_ms = 0u64, fallback_used = false))]
    pub fn run_with_fallback(&self, name: &str, input: &ToolInput) -> Result<ToolOutput> {
        let start = std::time::Instant::now();

        let Some(primary) = self.get(name) else {
            let elapsed = start.elapsed().as_millis() as u64;
            tracing::warn!(target: "tool_execution", tool = %name, latency_ms = elapsed, "tool not found");
            anyhow::bail!("{}", tf("error.tool_not_found", &[("name", name)]));
        };

        let mut primary_result = primary.run(input)?;
        if primary_result.success {
            let elapsed = start.elapsed().as_millis() as u64;
            tracing::debug!(
                target: "tool_execution",
                tool = %name,
                latency_ms = elapsed,
                success = true,
                "tool executed successfully"
            );
            record_tool_execution(
                "tool_execution_total",
                name,
                true,
                elapsed,
                serde_json::to_string(&input.payload).ok().map(|s| s.len()),
            );
            return Ok(primary_result);
        }

        let fallback_chain = self
            .profile(name)
            .map(|profile| profile.fallback_chain.clone())
            .unwrap_or_default();

        for fallback_name in fallback_chain {
            if let Some(fallback_tool) = self.get(&fallback_name) {
                let mut fallback_result = fallback_tool.run(input)?;
                if fallback_result.success {
                    let elapsed = start.elapsed().as_millis() as u64;
                    fallback_result.audit_log = Some(format!(
                        "primary '{}' failed, fallback '{}' succeeded",
                        name, fallback_name
                    ));
                    tracing::info!(
                        target: "tool_execution",
                        primary = %name,
                        fallback = %fallback_name,
                        latency_ms = elapsed,
                        success = true,
                        fallback_used = true,
                        "fallback tool executed successfully"
                    );
                    record_tool_execution(
                        "tool_execution_total",
                        name,
                        true,
                        elapsed,
                        serde_json::to_string(&input.payload).ok().map(|s| s.len()),
                    );
                    return Ok(fallback_result);
                }
                primary_result = fallback_result;
            }
        }

        let elapsed = start.elapsed().as_millis() as u64;
        tracing::warn!(
            target: "tool_execution",
            tool = %name,
            latency_ms = elapsed,
            success = false,
            fallback_used = !self.profile(name).map(|p| p.fallback_chain.is_empty()).unwrap_or(true),
            "tool execution failed after all fallbacks"
        );
        record_tool_execution(
            "tool_execution_total",
            name,
            false,
            elapsed,
            serde_json::to_string(&input.payload).ok().map(|s| s.len()),
        );
        Ok(primary_result)
    }

    #[tracing::instrument(level = "debug", skip(self, input), fields(tool = %name, success = false, latency_ms = 0u64, fallback_used = false))]
    pub async fn run_with_fallback_async(
        &self,
        name: &str,
        input: &ToolInput,
    ) -> Result<ToolOutput> {
        let start = std::time::Instant::now();

        let Some(primary) = self.get_arc(name) else {
            let elapsed = start.elapsed().as_millis() as u64;
            tracing::warn!(target: "tool_execution", tool = %name, latency_ms = elapsed, "tool not found");
            anyhow::bail!("{}", tf("error.tool_not_found", &[("name", name)]));
        };

        let mut primary_result = primary.run_async(input.clone()).await?;
        if primary_result.success {
            let elapsed = start.elapsed().as_millis() as u64;
            tracing::debug!(
                target: "tool_execution",
                tool = %name,
                latency_ms = elapsed,
                success = true,
                "tool executed successfully"
            );
            record_tool_execution(
                "tool_execution_total",
                name,
                true,
                elapsed,
                serde_json::to_string(&input.payload).ok().map(|s| s.len()),
            );
            return Ok(primary_result);
        }

        let fallback_chain = self
            .profile(name)
            .map(|profile| profile.fallback_chain.clone())
            .unwrap_or_default();

        for fallback_name in fallback_chain {
            if let Some(fallback_tool) = self.get_arc(&fallback_name) {
                let mut fallback_result = fallback_tool.run_async(input.clone()).await?;
                if fallback_result.success {
                    let elapsed = start.elapsed().as_millis() as u64;
                    fallback_result.audit_log = Some(format!(
                        "primary '{}' failed, fallback '{}' succeeded",
                        name, fallback_name
                    ));
                    tracing::info!(
                        target: "tool_execution",
                        primary = %name,
                        fallback = %fallback_name,
                        latency_ms = elapsed,
                        success = true,
                        fallback_used = true,
                        "fallback tool executed successfully"
                    );
                    record_tool_execution(
                        "tool_execution_total",
                        name,
                        true,
                        elapsed,
                        serde_json::to_string(&input.payload).ok().map(|s| s.len()),
                    );
                    return Ok(fallback_result);
                }
                primary_result = fallback_result;
            }
        }

        let elapsed = start.elapsed().as_millis() as u64;
        tracing::warn!(
            target: "tool_execution",
            tool = %name,
            latency_ms = elapsed,
            success = false,
            fallback_used = !self.profile(name).map(|p| p.fallback_chain.is_empty()).unwrap_or(true),
            "tool execution failed after all fallbacks"
        );
        record_tool_execution(
            "tool_execution_total",
            name,
            false,
            elapsed,
            serde_json::to_string(&input.payload).ok().map(|s| s.len()),
        );
        Ok(primary_result)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Record a tool execution metric via the global performance monitor.
///
/// Tracks tool call count, latency, and success/failure for observability
/// and alert-rule evaluation (P3-9). Uses an explicit info_span to record
/// the tool name, input size (when available), latency, and success status
/// into the distributed trace tree.
pub fn record_tool_execution(
    metric_name: &str,
    tool: &str,
    success: bool,
    latency_ms: u64,
    input_size: Option<usize>,
) {
    crate::observability::performance::record_global_operation(success, latency_ms as f64);

    let span = tracing::info_span!(
        target: "tool_execution",
        "tool.execute",
        tool = %tool,
        input_size = input_size.unwrap_or(0),
        latency_ms = latency_ms,
        success = success,
    );
    let _guard = span.enter();

    tracing::trace!(
        target: "tool_execution",
        metric = %metric_name,
        tool = %tool,
        input_size = input_size.unwrap_or(0),
        success = success,
        latency_ms = latency_ms,
        "tool execution metric"
    );
}

/// Sanitize and validate a file path against the allowed base directory.
///
/// 1. Resolves the path relative to the current working directory.
/// 2. Canonicalizes (or normalizes) the resolved path.
/// 3. If `allowed_base_dir` is set, verifies the resolved path starts with it.
pub fn sanitize_path(input: &ToolInput, path: &str) -> Result<PathBuf> {
    let resolved = PathBuf::from(path);
    let canonical = if resolved.is_absolute() {
        std::fs::canonicalize(&resolved)
            .map_err(|e| anyhow::anyhow!("path canonicalization failed: {e}"))?
    } else {
        let cwd = std::env::current_dir()
            .map_err(|e| anyhow::anyhow!("unable to determine current directory: {e}"))?;
        let joined = cwd.join(&resolved);
        std::fs::canonicalize(&joined)
            .map_err(|e| anyhow::anyhow!("path canonicalization failed: {e}"))?
    };

    if let Some(ref base_dir) = input.allowed_base_dir {
        let base_canonical = std::fs::canonicalize(base_dir).unwrap_or_else(|_| base_dir.clone());
        if !canonical.starts_with(&base_canonical) {
            anyhow::bail!(
                "{}",
                tf(
                    "error.path_traversal_denied",
                    &[("path", path), ("base", &base_dir.display().to_string())]
                )
            );
        }
    }

    Ok(canonical)
}

/// Sanitize and validate a path that may not exist yet (e.g. destination for
/// move/write operations). Resolves the parent directory and joins the
/// filename, then validates against the allowed base directory.
pub fn sanitize_path_for_write(input: &ToolInput, path: &str) -> Result<PathBuf> {
    let resolved = PathBuf::from(path);

    // Try canonicalizing the resolved path first; if it exists, use it directly.
    let canonical = if resolved.is_absolute() {
        std::fs::canonicalize(&resolved).unwrap_or_else(|_| {
            // Path doesn't exist — resolve via parent directory
            let parent = resolved
                .parent()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .unwrap_or_else(|| {
                    // If parent can't be canonicalized either, return resolved as-is
                    PathBuf::from(path)
                });
            parent.join(resolved.file_name().unwrap_or_default())
        })
    } else {
        let cwd = match std::env::current_dir() {
            Ok(dir) => dir,
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "unable to determine current directory: {e}"
                ));
            }
        };
        let joined = cwd.join(&resolved);
        std::fs::canonicalize(&joined).unwrap_or_else(|_| {
            let parent = joined
                .parent()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .unwrap_or_else(|| cwd.clone());
            parent.join(joined.file_name().unwrap_or_default())
        })
    };

    if let Some(ref base_dir) = input.allowed_base_dir {
        let base_canonical = std::fs::canonicalize(base_dir).unwrap_or_else(|_| base_dir.clone());
        if !canonical.starts_with(&base_canonical) {
            anyhow::bail!(
                "{}",
                tf(
                    "error.path_traversal_denied",
                    &[("path", path), ("base", &base_dir.display().to_string())]
                )
            );
        }
    }

    Ok(canonical)
}

pub struct ReadFileTool;
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = serde_json::to_string(&input.payload)
                .unwrap_or_default()
                .len() as u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let validated_path = sanitize_path(input, path)?;

        // Non-blocking try_acquire read lock to prevent concurrent writes.
        // If lock is contended, read proceeds without lock — the OS file
        // system provides coherence for concurrent reads.
        let _lock =
            tool_lock_manager().try_acquire(&validated_path.to_string_lossy(), LockMode::Read);

        let content = std::fs::read_to_string(&validated_path)?;
        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", true);
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"content": content})),
            error: None,
            verification: Some("file_read".to_string()),
            audit_log: Some(format!("Read file: {}", validated_path.display())),
            pua_report: Some(tool_execution_report("read_file", Some("file_read"))),
        })
    }
}

pub struct WriteFileTool;
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = serde_json::to_string(&input.payload)
                .unwrap_or_default()
                .len() as u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let content = input.payload["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_content")))?;
        let mode = input.payload["mode"].as_str().unwrap_or("overwrite");
        let path_buf = sanitize_path_for_write(input, path)?;

        // Non-blocking try_acquire write lock.
        // If lock is already held by another operation, return a transient
        // error so the TAO loop can retry.
        let _lock = tool_lock_manager()
            .try_acquire(&path_buf.to_string_lossy(), LockMode::Write)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "write lock contended for '{}' — another tool is modifying this file",
                    path_buf.display()
                )
            })?;

        if let Some(parent) = path_buf.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        match mode {
            "append" => {
                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path_buf)?;
                file.write_all(content.as_bytes())?;
            }
            "overwrite" => {
                fs::write(&path_buf, content)?;
            }
            other => {
                anyhow::bail!("{}", tf("error.unsupported_write_mode", &[("mode", other)]));
            }
        }

        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", true);
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"path": path, "mode": mode})),
            error: None,
            verification: Some("file_written".to_string()),
            audit_log: Some(format!("Wrote file: {} ({})", path_buf.display(), mode)),
            pua_report: Some(tool_execution_report("write_file", Some("file_written"))),
        })
    }
}

pub struct SearchFilesTool;
impl Tool for SearchFilesTool {
    fn name(&self) -> &'static str {
        "search_files"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = serde_json::to_string(&input.payload)
                .unwrap_or_default()
                .len() as u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let pattern = input.payload["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_pattern")))?;
        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let root = sanitize_path(input, directory)?;
        let matcher = Pattern::new(pattern)?;
        let mut files = Vec::new();
        collect_matching_files(&root, &root, &matcher, &mut files)?;

        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", true);
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"files": files})),
            error: None,
            verification: Some("search_done".to_string()),
            audit_log: Some(format!(
                "Search files completed for pattern '{}' in '{}'",
                pattern,
                root.display()
            )),
            pua_report: Some(tool_execution_report("search_files", Some("search_done"))),
        })
    }
}

pub struct ApplyPatchTool;
impl Tool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = serde_json::to_string(&input.payload)
                .unwrap_or_default()
                .len() as u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let patch = input.payload["patch"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_patch")))?;
        let check_only = input.payload["check"].as_bool().unwrap_or(false);
        let current_dir = input.payload["directory"].as_str().unwrap_or(".");
        let sanitized_dir = sanitize_path(input, current_dir)?;
        let mut command = Command::new("git");
        command.arg("apply");
        if check_only {
            command.arg("--check");
        }
        // Pipe patch via stdin to avoid Windows \\?\ long-path prefix issues
        // that arise when using tempfile (git apply can't open \\?\ prefixed paths).
        command.arg("-");
        debug!(directory = %current_dir, check_only = %check_only, "tool: running git apply");
        command.current_dir(&sanitized_dir);
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        let mut child = command.spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(patch.as_bytes())?;
        }
        let output = child.wait_with_output()?;
        let success = output.status.success();
        if !success {
            warn!(
                directory = %current_dir,
                check_only = %check_only,
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "tool: git apply failed"
            );
        }

        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", success);
        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "applied": success && !check_only,
                "checked": check_only,
                "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                "exit_code": output.status.code(),
            })),
            error: (!success).then(|| String::from_utf8_lossy(&output.stderr).trim().to_string()),
            verification: Some(
                if check_only {
                    "patch_checked"
                } else {
                    "patch_applied"
                }
                .to_string(),
            ),
            audit_log: Some(format!("git apply executed in '{}'", current_dir)),
            pua_report: Some(tool_execution_report(
                "apply_patch",
                Some(if check_only {
                    "patch_checked"
                } else {
                    "patch_applied"
                }),
            )),
        })
    }
}

/// Hardcoded allowlist of test commands for the `run_tests` tool.
///
/// Only commands in this list can be executed via the `run_tests` tool.
/// This prevents arbitrary command execution through the test runner.
/// To extend this list, modify `ALLOWED_TEST_COMMANDS` in this file.
const ALLOWED_TEST_COMMANDS: &[&str] = &[
    "cargo", "npm", "yarn", "pnpm", "make", "go", "python", "pytest", "mvn", "gradle", "git",
];

pub struct RunTestsTool;
impl Tool for RunTestsTool {
    fn name(&self) -> &'static str {
        "run_tests"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = serde_json::to_string(&input.payload)
                .unwrap_or_default()
                .len() as u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let command_name = input.payload["command"].as_str().unwrap_or("cargo");
        if !ALLOWED_TEST_COMMANDS.contains(&command_name) {
            let allowed = ALLOWED_TEST_COMMANDS.join(", ");
            anyhow::bail!(
                "{} — allowed commands: {}",
                tf("error.command_not_allowed", &[("command", command_name)]),
                allowed,
            );
        }
        let args = input.payload["args"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|text| text.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec!["test".to_string()]);
        // Validate arguments: only allow alphanumeric, `-`, `_`, `.`, `/`, `=`, and `--` prefixes
        for arg in &args {
            if !arg.chars().all(|c| {
                c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/' || c == '='
            }) {
                anyhow::bail!("Invalid test argument: '{}' — only alphanumeric, dashes, underscores, dots, slashes, and equals signs allowed", arg);
            }
            if arg.starts_with("--")
                && !arg[2..]
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '=')
            {
                anyhow::bail!("Invalid flag argument: '{}'", arg);
            }
        }
        let current_dir = sanitize_path(input, input.payload["directory"].as_str().unwrap_or("."))?;
        debug!(command = %command_name, args = ?args, directory = %current_dir.display(), "tool: running shell command");
        let output = Command::new(command_name)
            .args(&args)
            .current_dir(&current_dir)
            .output()?;
        let success = output.status.success();
        if !success {
            warn!(
                command = %command_name,
                exit_code = ?output.status.code(),
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "tool: shell command failed"
            );
        }

        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", success);
        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "command": command_name,
                "args": args,
                "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                "exit_code": output.status.code(),
            })),
            error: (!success).then(|| String::from_utf8_lossy(&output.stderr).trim().to_string()),
            verification: Some("tests_passed".to_string()),
            audit_log: Some(format!(
                "Executed '{}' in '{}'",
                command_name,
                current_dir.display()
            )),
            pua_report: Some(tool_execution_report("run_tests", Some("tests_passed"))),
        })
    }
}

pub struct InspectGitDiffTool;
impl Tool for InspectGitDiffTool {
    fn name(&self) -> &'static str {
        "inspect_git_diff"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = serde_json::to_string(&input.payload)
                .unwrap_or_default()
                .len() as u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let current_dir = input.payload["directory"].as_str().unwrap_or(".");
        let sanitized_dir = sanitize_path(input, current_dir)?;
        let staged = input.payload["staged"].as_bool().unwrap_or(false);
        let files = input.payload["files"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|text| text.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut command = Command::new("git");
        command.arg("diff").current_dir(&sanitized_dir);
        if staged {
            command.arg("--cached");
        }
        if !files.is_empty() {
            command.arg("--").args(&files);
        }
        let output = command.output()?;
        let success = output.status.success();

        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", success);
        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "diff": String::from_utf8_lossy(&output.stdout).to_string(),
                "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                "exit_code": output.status.code(),
                "staged": staged,
                "files": files,
            })),
            error: (!success).then(|| String::from_utf8_lossy(&output.stderr).trim().to_string()),
            verification: Some("diff_inspected".to_string()),
            audit_log: Some(format!("git diff inspected in '{}'", current_dir)),
            pua_report: Some(tool_execution_report(
                "inspect_git_diff",
                Some("diff_inspected"),
            )),
        })
    }
}

// ── SkillListTool ────────────────────────────────────────────────────────────

/// Tool that lists all registered skills with their name, description, and score.
///
/// Requires a `SkillRegistry` to have been set via `set_skill_registry()`
/// before calling `run()`. Returns an empty list if no registry is configured.
///
/// Input payload: ignored (no arguments required).
/// Output: `{ "skills": [{ "name": "...", "description": "...", "score": 0.0 }, ...] }`
pub struct SkillListTool;

impl Tool for SkillListTool {
    fn name(&self) -> &'static str {
        "skill_list"
    }

    fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = 0u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let skills = match SKILL_REGISTRY.get() {
            Some(registry) => match registry.read() {
                Ok(guard) => {
                    let descriptors = guard.list();
                    descriptors
                        .into_iter()
                        .map(|d| {
                            serde_json::json!({
                                "name": d.name,
                                "description": d.description,
                                "score": d.score,
                                "input_schema": d.input_schema,
                                "total_calls": d.total_calls,
                                "success_calls": d.success_calls,
                                "failure_calls": d.failure_calls,
                                "average_latency_ms": d.average_latency_ms,
                            })
                        })
                        .collect::<Vec<_>>()
                }
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        };

        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", true);
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"skills": skills})),
            error: None,
            verification: Some("skills_listed".to_string()),
            audit_log: Some(format!("Listed {} skill(s)", skills.len())),
            pua_report: Some(tool_execution_report("skill_list", Some("skills_listed"))),
        })
    }
}

// ── SkillExecuteTool ──────────────────────────────────────────────────────────

/// Tool that executes a registered skill by name with provided input.
///
/// Requires a `SkillRegistry` to have been set via `set_skill_registry()`.
/// Returns an error if no registry or the skill is not found.
pub struct SkillExecuteTool;

/// Shared static Arc for SkillExecuteTool — avoids allocating a new Arc on every call.
static SKILL_EXECUTE_TOOL: std::sync::OnceLock<std::sync::Arc<SkillExecuteTool>> =
    std::sync::OnceLock::new();

fn skill_execute_arc() -> std::sync::Arc<SkillExecuteTool> {
    SKILL_EXECUTE_TOOL
        .get_or_init(|| std::sync::Arc::new(SkillExecuteTool))
        .clone()
}

/// Shared tokio runtime for skill execution — lazily created once.
static SKILL_RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

fn skill_runtime() -> &'static tokio::runtime::Runtime {
    SKILL_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build shared skill runtime")
    })
}

impl Tool for SkillExecuteTool {
    fn name(&self) -> &'static str {
        "skill_execute"
    }

    /// Async execution: look up the skill from the registry (via spawn_blocking to
    /// avoid holding the async runtime on a RwLock read), then await
    /// `skill.execute(...)` directly. This avoids violating principle #23
    /// (no block_in_place + block_on in hot paths).
    fn run_async(
        self: std::sync::Arc<Self>,
        input: ToolInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            let span = tracing::info_span!(
                "tool.run",
                tool = self.name(),
                input_size = 0u64,
                latency_ms = 0u64,
                success = false,
            );
            let _guard = span.enter();
            let start = Instant::now();

            // ── Step 1: Extract skill name from payload ──
            let payload = &input.payload;
            let skill_name = payload
                .get("skill_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("skill_execute requires 'skill_name' argument"))?
                .to_string();
            let skill_input = payload
                .get("input")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            // ── Step 2: Look up skill in registry (async-safe via spawn_blocking) ──
            let skill = match SKILL_REGISTRY.get() {
                Some(registry) => {
                    let registry = Arc::clone(registry);
                    let skill_name = skill_name.clone();
                    let skill_input_val = skill_input.clone();
                    tokio::task::spawn_blocking(move || {
                        let guard = registry
                            .read()
                            .map_err(|e| anyhow::anyhow!("skill registry lock failed: {}", e))?;
                        // Try exact match first, then fuzzy match
                        guard
                            .get(&skill_name)
                            .or_else(|| {
                                let fuzzy =
                                    guard.best_match_with_input(&skill_name, &skill_input_val)?;
                                tracing::info!(
                                    "skill_execute: fuzzy-matched '{}' -> '{}'",
                                    skill_name,
                                    fuzzy
                                );
                                guard.get(&fuzzy)
                            })
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "skill '{}' not found in registry (no fuzzy match either). \
                                     Use 'skill_list' tool first to see available skills.",
                                    skill_name
                                )
                            })
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("skill registry lock task failed: {}", e))??
                }
                None => {
                    return Ok(ToolOutput {
                        success: false,
                        result: None,
                        error: Some(
                            "no skill registry configured — call set_skill_registry() first"
                                .to_string(),
                        ),
                        verification: None,
                        audit_log: None,
                        pua_report: None,
                    });
                }
            };

            // ── Step 3: Execute the skill (truly async) ──
            let exec_start = Instant::now();
            let result = skill.execute(&skill_input).await;
            let exec_elapsed = exec_start.elapsed();

            // Record outcome in registry (async-safe via spawn_blocking)
            let elapsed = start.elapsed().as_millis() as u64;
            let outcome_success = result.is_ok();
            if let Some(registry) = SKILL_REGISTRY.get() {
                let registry = Arc::clone(registry);
                let s_name = skill_name.clone();
                tokio::task::spawn_blocking(move || {
                    if let Ok(mut guard) = registry.write() {
                        guard.record_outcome(&s_name, outcome_success, exec_elapsed);
                    }
                })
                .await
                .ok();
            }
            span.record("latency_ms", elapsed);

            match result {
                Ok(value) => {
                    span.record("success", true);
                    Ok(ToolOutput {
                        success: true,
                        result: Some(serde_json::json!({
                            "skill": skill_name,
                            "output": value,
                        })),
                        error: None,
                        verification: Some("skill_executed".to_string()),
                        audit_log: Some(format!("Executed skill '{}'", skill_name)),
                        pua_report: Some(tool_execution_report(
                            "skill_execute",
                            Some("skill_executed"),
                        )),
                    })
                }
                Err(e) => {
                    span.record("success", false);
                    Ok(ToolOutput {
                        success: false,
                        result: None,
                        error: Some(format!("skill '{}' execution failed: {}", skill_name, e)),
                        verification: None,
                        audit_log: None,
                        pua_report: None,
                    })
                }
            }
        })
    }

    /// Sync fallback: bridges to `run_async` via the dedicated skill runtime.
    ///
    /// Always uses the dedicated blocking runtime to avoid `block_in_place` + `block_on`
    /// on hot paths (principle #23). Async callers should always use `run_async`
    /// directly for optimal non-blocking execution.
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let input = input.clone();
        let rt = skill_runtime();
        rt.block_on(skill_execute_arc().run_async(input))
    }
}

// ---------------------------------------------------------------------------
// SkillCreateTool — bridges existing SkillCreatorSkill to the ToolRegistry
// ---------------------------------------------------------------------------

/// Tool that creates a new skill from a prompt template.
///
/// Bridges to the existing `SkillCreatorSkill` in the skill execution system.
/// Requires a `SkillRegistry` to have been set via `set_skill_registry()`.
pub struct SkillCreateTool;

impl Tool for SkillCreateTool {
    fn name(&self) -> &'static str {
        "skill_create"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let payload = &input.payload;
        let name = payload
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("skill_create requires 'name' argument"))?;
        let description = payload
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("skill_create requires 'description' argument"))?;
        let prompt_template = payload
            .get("prompt_template")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("skill_create requires 'prompt_template' argument"))?;
        // Parse optional input_schema from JSON Value into HashMap<String, String>
        let input_schema: std::collections::HashMap<String, String> = payload
            .get("input_schema")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        let registry = SKILL_REGISTRY.get().ok_or_else(|| {
            anyhow::anyhow!("no skill registry configured — call set_skill_registry() first")
        })?;
        let mut guard = registry
            .write()
            .map_err(|e| anyhow::anyhow!("skill registry lock failed: {}", e))?;

        guard
            .create_skill_from_prompt(name, description, prompt_template, input_schema)
            .map_err(|e| anyhow::anyhow!("failed to create skill: {}", e))?;

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "skill": name,
                "description": description,
            })),
            error: None,
            verification: Some("skill_created".to_string()),
            audit_log: Some(format!("Created skill '{}': {}", name, description)),
            pua_report: Some(tool_execution_report("skill_create", Some("skill_created"))),
        })
    }
}

// ---------------------------------------------------------------------------
// SkillReloadTool — triggers immediate skill refresh from ~/.agents/skills/
// ---------------------------------------------------------------------------

/// Tool that triggers an immediate reload of skills from the local skills directory.
///
/// Without this tool, AI agents would need to wait up to 60s for the background
/// refresh task.  This is the instant version.
pub struct SkillReloadTool;

impl Tool for SkillReloadTool {
    fn name(&self) -> &'static str {
        "skill_reload"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let registry = SKILL_REGISTRY.get().ok_or_else(|| {
            anyhow::anyhow!("no skill registry configured — call set_skill_registry() first")
        })?;

        let custom_dir = input.payload.get("directory").and_then(|v| v.as_str());
        let agents_skills_dir = custom_dir.map(std::path::PathBuf::from);

        let mut guard = registry
            .write()
            .map_err(|e| anyhow::anyhow!("skill registry lock failed: {}", e))?;

        let summary = guard
            .discover_and_register_local_skills(agents_skills_dir.as_deref())
            .map_err(|e| anyhow::anyhow!("skill reload failed: {}", e))?;

        let total = guard.list().len();

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "registered": summary.registered,
                "skipped": summary.skipped,
                "errors": summary.errors,
                "total_skills": total,
            })),
            error: None,
            verification: Some("skills_reloaded".to_string()),
            audit_log: Some(format!(
                "Skill reload: {} new, {} skipped, {} errors ({} total)",
                summary.registered,
                summary.skipped,
                summary.errors.len(),
                total
            )),
            pua_report: Some(tool_execution_report(
                "skill_reload",
                Some("skills_reloaded"),
            )),
        })
    }
}

// ---------------------------------------------------------------------------
fn collect_matching_files(
    root: &Path,
    current: &Path,
    matcher: &Pattern,
    files: &mut Vec<String>,
) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_matching_files(root, &path, matcher, files)?;
            continue;
        }

        let relative = path.strip_prefix(root).unwrap_or(&path);
        let candidate = relative.to_string_lossy().replace('\\', "/");
        if matcher.matches(&candidate) || matcher.matches_path(relative) {
            files.push(path.to_string_lossy().to_string());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Think-Act-Observe tool execution loop (F-GAP-01)
// ---------------------------------------------------------------------------
//
// Full Think → Act → Observe orchestration loop:
//
// 1. Think:   Analyze task context, select the best tool candidate
// 2. Act:     Execute tool call with fallback-chain support
// 3. Observe: Validate output, decide next action (continue / retry /
//             switch tool / complete / escalate)
//
// Loop termination:
// - Tool succeeds and output verification passes
// - All tool candidates exhausted (retry + fallback limits reached)
// - Maximum iteration count reached

/// Outcome of a single Observe phase.
#[derive(Debug, Clone)]
pub enum LoopDecision {
    /// Continue to the next Think-Act-Observe cycle.
    Continue,
    /// Retry the same tool.
    Retry { tool: String, reason: String },
    /// Switch to a different tool candidate.
    SwitchTool {
        from: String,
        to: String,
        reason: String,
    },
    /// Loop completed successfully.
    Complete(ToolOutput),
    /// All candidates exhausted – final failure.
    Failed {
        reason: String,
        last_output: Option<ToolOutput>,
    },
    /// Escalate to human review.
    Escalate { reason: String, output: ToolOutput },
}

/// Configuration for the Think-Act-Observe loop.
#[derive(Debug, Clone)]
pub struct LoopConfig {
    /// Maximum number of iterations (Think→Act→Observe cycles).
    pub max_iterations: u32,
    /// Maximum retries per tool before switching.
    pub max_retries_per_tool: u32,
    /// Whether to enable fallback-chain execution.
    pub enable_fallback: bool,
    /// Optional output-verification function.
    pub verify_output: Option<fn(&ToolOutput) -> bool>,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 5,
            max_retries_per_tool: 2,
            enable_fallback: true,
            verify_output: None,
        }
    }
}

/// A single trace entry for one loop iteration.
#[derive(Debug, Clone, Serialize)]
pub struct LoopIteration {
    pub stage: String,
    pub tool: String,
    pub success: bool,
    pub duration_ms: u64,
    pub detail: String,
}

/// Full execution trace of a Think-Act-Observe loop.
#[derive(Debug, Clone, Serialize)]
pub struct LoopTrace {
    pub iterations: Vec<LoopIteration>,
    pub final_decision: String,
    pub total_duration_ms: u64,
}

/// Think phase result: which tool to run and why.
#[derive(Debug, Clone)]
struct ThinkResult {
    tool: String,
    confidence: f64,
    rationale: String,
}

/// Result of a single iteration's observe phase — tells the caller what to do next.
#[derive(Debug)]
enum IterationAction {
    /// Continue to the next iteration.
    Continue,
    /// Tool completed successfully.
    Complete(ToolOutput),
    /// All candidates exhausted.
    Failed {
        reason: String,
        last_output: Option<ToolOutput>,
    },
    /// Escalate to human review.
    Escalate { reason: String, output: ToolOutput },
}

/// Shared post-Act phase: record the result, observe the output, and decide
/// the next action. Called by both `execute_loop` and `execute_loop_async`
/// to avoid duplicating the observe-and-match logic.
#[allow(clippy::too_many_arguments)]
fn handle_iteration(
    task: &str,
    trace: &mut LoopTrace,
    start: Instant,
    iteration: u32,
    tr: &ThinkResult,
    output: ToolOutput,
    act_duration_ms: u64,
    config: &LoopConfig,
    retry_counts: &mut HashMap<String, u32>,
) -> IterationAction {
    // Record the act phase in the trace.
    trace.iterations.push(LoopIteration {
        stage: "act".to_string(),
        tool: tr.tool.clone(),
        success: output.success,
        duration_ms: act_duration_ms,
        detail: if output.success {
            "execution ok".to_string()
        } else {
            output
                .error
                .clone()
                .unwrap_or_else(|| "unknown error".to_string())
        },
    });

    // ── Observe ──────────────────────────────────────────────
    let observe_decision = observe(&output, &tr.tool, retry_counts, config, |tool, reason| {
        trace.iterations.push(LoopIteration {
            stage: "observe".to_string(),
            tool,
            success: false,
            duration_ms: 0,
            detail: reason,
        });
    });

    match observe_decision {
        LoopDecision::Continue => {
            trace.iterations.push(LoopIteration {
                stage: "think".to_string(),
                tool: tr.tool.clone(),
                success: true,
                duration_ms: 0,
                detail: "output ok, continuing".to_string(),
            });
            IterationAction::Continue
        }
        LoopDecision::Retry { tool, reason } => {
            trace.iterations.push(LoopIteration {
                stage: "think".to_string(),
                tool: tool.clone(),
                success: false,
                duration_ms: 0,
                detail: format!("retry: {}", reason),
            });
            IterationAction::Continue
        }
        LoopDecision::SwitchTool { from, to, reason } => {
            debug!(from, to, reason, "TAO: switching tool");
            trace.iterations.push(LoopIteration {
                stage: "think".to_string(),
                tool: from,
                success: false,
                duration_ms: 0,
                detail: format!("switch to '{}': {}", to, reason),
            });
            IterationAction::Continue
        }
        LoopDecision::Complete(output) => {
            trace.final_decision = "success".to_string();
            trace.total_duration_ms = start.elapsed().as_millis() as u64;
            info!(
                task,
                tool = tr.tool,
                iterations = iteration + 1,
                "TAO: completed"
            );
            IterationAction::Complete(output)
        }
        LoopDecision::Failed {
            reason,
            last_output,
        } => {
            trace.final_decision = "failed".to_string();
            trace.total_duration_ms = start.elapsed().as_millis() as u64;
            warn!(task, reason, "TAO: failed");
            IterationAction::Failed {
                reason,
                last_output,
            }
        }
        LoopDecision::Escalate { reason, output } => {
            trace.final_decision = "escalated".to_string();
            trace.total_duration_ms = start.elapsed().as_millis() as u64;
            warn!(task, reason, "TAO: escalated");
            IterationAction::Escalate { reason, output }
        }
    }
}

/// Run the Think-Act-Observe loop for a given task.
///
/// # Arguments
///
/// * `task` - Human-readable task description (used for logging / tracing).
/// * `registry` - Tool registry holding all available tools.
/// * `input` - Input envelope passed to each tool.
/// * `preferred_tools` - Ordered list of tool names to try first.
/// * `config` - Loop configuration (iterations, retries, verification).
///
/// # Returns
///
/// A tuple of `(LoopDecision, LoopTrace)` where the decision conveys the
/// final outcome and the trace records every iteration for observability.
pub fn execute_loop(
    task: &str,
    registry: &ToolRegistry,
    input: &ToolInput,
    preferred_tools: &[String],
    config: &LoopConfig,
    mut recommender: Option<&mut recommender::ToolRecommender>,
) -> (LoopDecision, LoopTrace) {
    let start = std::time::Instant::now();
    let mut trace = LoopTrace {
        iterations: Vec::new(),
        final_decision: String::new(),
        total_duration_ms: 0,
    };

    // Build the candidate list with retry bookkeeping.
    let tool_candidates: Vec<String> = if preferred_tools.is_empty() {
        registry.names().iter().map(|&n| n.to_string()).collect()
    } else {
        preferred_tools.to_vec()
    };
    let mut retry_counts: HashMap<String, u32> = HashMap::new();

    for iteration in 0..config.max_iterations {
        // ── Think ────────────────────────────────────────────────
        // Select the best tool candidate based on retry history.
        let think_result = think(
            task,
            &tool_candidates,
            &retry_counts,
            config,
            recommender.as_deref(),
        );

        let Some(tr) = think_result else {
            let decision = LoopDecision::Failed {
                reason: "no available tool candidates after think phase".to_string(),
                last_output: None,
            };
            trace.final_decision = "failed_no_candidates".to_string();
            trace.total_duration_ms = start.elapsed().as_millis() as u64;
            warn!(task, iteration, "TAO: no candidates – failed");
            return (decision, trace);
        };

        trace.iterations.push(LoopIteration {
            stage: "think".to_string(),
            tool: tr.tool.clone(),
            success: true,
            duration_ms: 0,
            detail: format!(
                "confidence={:.2}, rationale={}",
                tr.confidence, tr.rationale
            ),
        });

        // ── Act ──────────────────────────────────────────────────
        // Execute the selected tool (with fallback if enabled).
        let act_start = std::time::Instant::now();
        let output = if config.enable_fallback {
            registry
                .run_with_fallback(&tr.tool, input)
                .unwrap_or_else(|e| ToolOutput {
                    success: false,
                    result: None,
                    error: Some(format!("tool '{}' error: {}", tr.tool, e)),
                    verification: None,
                    audit_log: None,
                    pua_report: None,
                })
        } else {
            registry.get(&tr.tool).map_or_else(
                || ToolOutput {
                    success: false,
                    result: None,
                    error: Some(format!("tool '{}' not found", tr.tool)),
                    verification: None,
                    audit_log: None,
                    pua_report: None,
                },
                |tool| {
                    tool.run(input).unwrap_or_else(|e| ToolOutput {
                        success: false,
                        result: None,
                        error: Some(format!("{}", e)),
                        verification: None,
                        audit_log: None,
                        pua_report: None,
                    })
                },
            )
        };
        let act_duration_ms = act_start.elapsed().as_millis() as u64;

        // Record usage statistics with the recommender when available.
        if let Some(rec) = &mut recommender {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            rec.record_usage(&tr.tool, output.success, act_duration_ms, now, &[]);
        }

        // ── Observe ──────────────────────────────────────────────
        match handle_iteration(
            task,
            &mut trace,
            start,
            iteration,
            &tr,
            output,
            act_duration_ms,
            config,
            &mut retry_counts,
        ) {
            IterationAction::Continue => continue,
            IterationAction::Complete(output) => {
                return (LoopDecision::Complete(output), trace);
            }
            IterationAction::Failed {
                reason,
                last_output,
            } => {
                return (
                    LoopDecision::Failed {
                        reason,
                        last_output,
                    },
                    trace,
                );
            }
            IterationAction::Escalate { reason, output } => {
                return (LoopDecision::Escalate { reason, output }, trace);
            }
        }
    }

    // Exhausted maximum iterations.
    let decision = LoopDecision::Failed {
        reason: format!("max iterations ({}) reached", config.max_iterations),
        last_output: None,
    };
    trace.final_decision = "failed_max_iterations".to_string();
    trace.total_duration_ms = start.elapsed().as_millis() as u64;
    warn!(
        task,
        max_iterations = config.max_iterations,
        "TAO: max iterations reached"
    );
    (decision, trace)
}

/// Async version of `execute_loop`.
///
/// Has the exact same Think → Act → Observe logic as the synchronous version,
/// but executes tools via `run_with_fallback_async().await` instead of
/// `run_with_fallback()`, so it does not block the async runtime.
pub async fn execute_loop_async(
    task: &str,
    registry: &ToolRegistry,
    input: &ToolInput,
    preferred_tools: &[String],
    config: &LoopConfig,
    mut recommender: Option<&mut recommender::ToolRecommender>,
) -> (LoopDecision, LoopTrace) {
    let start = std::time::Instant::now();
    let mut trace = LoopTrace {
        iterations: Vec::new(),
        final_decision: String::new(),
        total_duration_ms: 0,
    };

    // Build the candidate list with retry bookkeeping.
    let tool_candidates: Vec<String> = if preferred_tools.is_empty() {
        registry.names().iter().map(|&n| n.to_string()).collect()
    } else {
        preferred_tools.to_vec()
    };
    let mut retry_counts: HashMap<String, u32> = HashMap::new();

    for iteration in 0..config.max_iterations {
        // ── Think ────────────────────────────────────────────────
        // Select the best tool candidate based on retry history.
        let think_result = think(
            task,
            &tool_candidates,
            &retry_counts,
            config,
            recommender.as_deref(),
        );

        let Some(tr) = think_result else {
            let decision = LoopDecision::Failed {
                reason: "no available tool candidates after think phase".to_string(),
                last_output: None,
            };
            trace.final_decision = "failed_no_candidates".to_string();
            trace.total_duration_ms = start.elapsed().as_millis() as u64;
            warn!(task, iteration, "TAO: no candidates – failed");
            return (decision, trace);
        };

        trace.iterations.push(LoopIteration {
            stage: "think".to_string(),
            tool: tr.tool.clone(),
            success: true,
            duration_ms: 0,
            detail: format!(
                "confidence={:.2}, rationale={}",
                tr.confidence, tr.rationale
            ),
        });

        // ── Act ──────────────────────────────────────────────────
        // Execute the selected tool (with fallback if enabled).
        let act_start = std::time::Instant::now();
        let output = if config.enable_fallback {
            registry
                .run_with_fallback_async(&tr.tool, input)
                .await
                .unwrap_or_else(|e| ToolOutput {
                    success: false,
                    result: None,
                    error: Some(format!("tool '{}' error: {}", tr.tool, e)),
                    verification: None,
                    audit_log: None,
                    pua_report: None,
                })
        } else {
            match registry.get_arc(&tr.tool) {
                None => ToolOutput {
                    success: false,
                    result: None,
                    error: Some(format!("tool '{}' not found", tr.tool)),
                    verification: None,
                    audit_log: None,
                    pua_report: None,
                },
                Some(tool) => tool
                    .run_async(input.clone())
                    .await
                    .unwrap_or_else(|e| ToolOutput {
                        success: false,
                        result: None,
                        error: Some(format!("{}", e)),
                        verification: None,
                        audit_log: None,
                        pua_report: None,
                    }),
            }
        };
        let act_duration_ms = act_start.elapsed().as_millis() as u64;

        // Record usage statistics with the recommender when available.
        if let Some(rec) = &mut recommender {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            rec.record_usage(&tr.tool, output.success, act_duration_ms, now, &[]);
        }

        // ── Observe ──────────────────────────────────────────────
        match handle_iteration(
            task,
            &mut trace,
            start,
            iteration,
            &tr,
            output,
            act_duration_ms,
            config,
            &mut retry_counts,
        ) {
            IterationAction::Continue => continue,
            IterationAction::Complete(output) => {
                return (LoopDecision::Complete(output), trace);
            }
            IterationAction::Failed {
                reason,
                last_output,
            } => {
                return (
                    LoopDecision::Failed {
                        reason,
                        last_output,
                    },
                    trace,
                );
            }
            IterationAction::Escalate { reason, output } => {
                return (LoopDecision::Escalate { reason, output }, trace);
            }
        }
    }

    // Exhausted maximum iterations.
    let decision = LoopDecision::Failed {
        reason: format!("max iterations ({}) reached", config.max_iterations),
        last_output: None,
    };
    trace.final_decision = "failed_max_iterations".to_string();
    trace.total_duration_ms = start.elapsed().as_millis() as u64;
    warn!(
        task,
        max_iterations = config.max_iterations,
        "TAO: max iterations reached"
    );
    (decision, trace)
}

/// Think phase: select the best tool candidate.
///
/// Selection strategy (in order of priority):
/// 1. If a `ToolRecommender` is available, consult it for task-based recommendations
///    and pick the highest-scoring candidate.
/// 2. Match tool names from keywords in the task description.
/// 3. Fall back to the tool with the fewest retries.
///
/// Returns `None` if no candidates are available.
fn think(
    task: &str,
    candidates: &[String],
    retry_counts: &HashMap<String, u32>,
    config: &LoopConfig,
    recommender: Option<&recommender::ToolRecommender>,
) -> Option<ThinkResult> {
    if candidates.is_empty() {
        return None;
    }

    // Phase 1: consult the ToolRecommender when available
    if let Some(rec) = recommender {
        let context: Vec<String> = Vec::new();
        let recommendations = rec.recommend(task, &context);
        if !recommendations.is_empty() {
            // Find the highest-scored recommendation that is in our candidate list
            // and hasn't exhausted its retries.
            for rec_candidate in &recommendations {
                if candidates.contains(&rec_candidate.tool_name) {
                    let retries = retry_counts
                        .get(&rec_candidate.tool_name)
                        .copied()
                        .unwrap_or(0);
                    if retries < config.max_retries_per_tool {
                        let confidence = (rec_candidate.relevance_score.min(1.0)
                            * (1.0
                                - (retries as f64 / config.max_retries_per_tool as f64).min(1.0)))
                        .max(0.1);
                        return Some(ThinkResult {
                            tool: rec_candidate.tool_name.clone(),
                            confidence,
                            rationale: format!(
                                "recommender task=\"{}\" tool={} score={:.3} retries={} reason={}",
                                task,
                                rec_candidate.tool_name,
                                rec_candidate.relevance_score,
                                retries,
                                rec_candidate.reason,
                            ),
                        });
                    }
                }
            }
        }
    }

    // Phase 2: try to match tool names from task description keywords
    if !task.is_empty() {
        let task_lower = task.to_lowercase();
        for candidate in candidates {
            if task_lower.contains(&candidate.to_lowercase()) {
                let retries = retry_counts.get(candidate).copied().unwrap_or(0);
                let confidence =
                    1.0 - (retries as f64 / config.max_retries_per_tool as f64).min(1.0);
                return Some(ThinkResult {
                    tool: candidate.clone(),
                    confidence,
                    rationale: format!(
                        "keyword_match task=\"{}\" tool={} retries={}",
                        task, candidate, retries,
                    ),
                });
            }
        }
    }

    // Phase 3: fall back to the tool with fewest retries
    let best = candidates
        .iter()
        .filter(|t| retry_counts.get(*t).copied().unwrap_or(0) < config.max_retries_per_tool)
        .min_by_key(|t| retry_counts.get(*t).copied().unwrap_or(0))?;

    let retries = retry_counts.get(best).copied().unwrap_or(0);
    let confidence = 1.0 - (retries as f64 / config.max_retries_per_tool as f64).min(1.0);

    Some(ThinkResult {
        tool: best.clone(),
        confidence,
        rationale: format!(
                "retries={}/{} candidates_remaining={}",
                retries,
                config.max_retries_per_tool,
                candidates
                    .iter()
                    .filter(|t| retry_counts.get(*t).copied().unwrap_or(0)
                        < config.max_retries_per_tool)
                    .count(),
            ),
    })
}

/// Observe phase: evaluate the output and decide the next action.
fn observe(
    output: &ToolOutput,
    tool: &str,
    retry_counts: &mut HashMap<String, u32>,
    config: &LoopConfig,
    mut on_fail: impl FnMut(String, String),
) -> LoopDecision {
    if output.success {
        // Optional verification check.
        if let Some(verify) = config.verify_output {
            if !verify(output) {
                let rc = retry_counts.entry(tool.to_string()).or_insert(0);
                *rc += 1;
                on_fail(tool.to_string(), "output verification failed".to_string());
                if *rc < config.max_retries_per_tool {
                    return LoopDecision::Retry {
                        tool: tool.to_string(),
                        reason: "verification failed".to_string(),
                    };
                }
                return LoopDecision::SwitchTool {
                    from: tool.to_string(),
                    to: "next_candidate".to_string(),
                    reason: "verification failed, retries exhausted".to_string(),
                };
            }
        }
        return LoopDecision::Complete(output.clone());
    }

    // Execution failed — increment retry count.
    let rc = retry_counts.entry(tool.to_string()).or_insert(0);
    *rc += 1;

    let error_msg = output
        .error
        .clone()
        .unwrap_or_else(|| "no error detail".to_string());
    on_fail(tool.to_string(), format!("execution failed: {}", error_msg));

    if *rc < config.max_retries_per_tool {
        return LoopDecision::Retry {
            tool: tool.to_string(),
            reason: format!(
                "attempt {}/{} failed: {}",
                rc, config.max_retries_per_tool, error_msg
            ),
        };
    }

    // Retries exhausted for this tool — try another candidate.
    LoopDecision::SwitchTool {
        from: tool.to_string(),
        to: "next_candidate".to_string(),
        reason: format!("retries exhausted for '{}': {}", tool, error_msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn init_git_repo(dir: &Path) {
        run_git(dir, &["init"]);
        run_git(dir, &["config", "user.email", "copilot@example.com"]);
        run_git(dir, &["config", "user.name", "Copilot Test"]);
        // Disable autocrlf to ensure consistent patch format across platforms
        run_git(dir, &["config", "core.autocrlf", "false"]);
    }

    fn run_git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git command should spawn");
        assert!(
            output.status.success(),
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-task".to_string(),
            phase: "test".to_string(),
            agent_role: "tool".to_string(),
            objective: "tool test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn apply_patch_tool_checks_and_applies_patch() {
        let temp = tempdir().expect("tempdir should be created");
        init_git_repo(temp.path());

        let file_path = temp.path().join("sample.txt");
        fs::write(&file_path, "hello\n").expect("initial file should be written");
        run_git(temp.path(), &["add", "sample.txt"]);
        run_git(temp.path(), &["commit", "-m", "init"]);

        fs::write(&file_path, "hello world\n").expect("updated file should be written");
        let patch = run_git(temp.path(), &["diff", "--", "sample.txt"]);
        run_git(temp.path(), &["checkout", "--", "sample.txt"]);

        let tool = ApplyPatchTool;
        let checked = tool
            .run(&tool_input(serde_json::json!({
                "patch": patch,
                "check": true,
                "directory": temp.path().to_string_lossy().to_string(),
            })))
            .expect("patch check should succeed");
        assert!(checked.success);

        let applied = tool
            .run(&tool_input(serde_json::json!({
                "patch": patch,
                "directory": temp.path().to_string_lossy().to_string(),
            })))
            .expect("patch apply should succeed");
        assert!(applied.success);
        let normalized = fs::read_to_string(&file_path)
            .expect("patched file should be readable")
            .replace("\r\n", "\n");
        assert_eq!(normalized, "hello world\n");
    }

    #[test]
    fn run_tests_tool_executes_configured_command() {
        // First check if git is available — skip if not (sandboxed CI
        // or parallel test execution with PATH changes).
        match std::process::Command::new("git").arg("--version").output() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("git not found in PATH, skipping test");
                return;
            }
            Err(e) => {
                eprintln!(
                    "git check failed with unexpected error: {}, skipping test",
                    e
                );
                return;
            }
            Ok(o) if !o.status.success() => {
                eprintln!("git --version returned non-zero, skipping test");
                return;
            }
            _ => {}
        }

        let tool = RunTestsTool;
        let result = match tool.run(&tool_input(serde_json::json!({
            "command": "git",
            "args": ["--version"],
            "directory": ".",
        }))) {
            Ok(r) => r,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("No such file or directory") || msg.contains("not found") {
                    eprintln!(
                        "git binary not found during test execution, skipping: {}",
                        msg
                    );
                    return;
                }
                panic!("command should execute, got: {}", msg);
            }
        };
        assert!(result.success);
        let stdout = result.result.expect("result should exist")["stdout"]
            .as_str()
            .expect("stdout should be string")
            .to_string();
        assert!(stdout.contains("git version"));
    }

    #[test]
    fn inspect_git_diff_tool_returns_actual_diff() {
        let temp = tempdir().expect("tempdir should be created");
        init_git_repo(temp.path());

        let file_path = temp.path().join("sample.txt");
        fs::write(&file_path, "hello\n").expect("initial file should be written");
        run_git(temp.path(), &["add", "sample.txt"]);
        run_git(temp.path(), &["commit", "-m", "init"]);
        fs::write(&file_path, "hello world\n").expect("updated file should be written");

        let tool = InspectGitDiffTool;
        let result = tool
            .run(&tool_input(serde_json::json!({
                "directory": temp.path().to_string_lossy().to_string(),
                "files": ["sample.txt"],
            })))
            .expect("git diff should execute");
        assert!(result.success);
        let diff = result.result.expect("result should exist")["diff"]
            .as_str()
            .expect("diff should be string")
            .to_string();
        assert!(diff.contains("hello world"));
    }

    struct AlwaysFailTool;
    impl Tool for AlwaysFailTool {
        fn name(&self) -> &'static str {
            "always_fail"
        }

        fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
            Ok(ToolOutput {
                success: false,
                result: None,
                error: Some("forced failure".to_string()),
                verification: Some("forced_failure".to_string()),
                audit_log: Some("always_fail executed".to_string()),
                pua_report: None,
            })
        }
    }

    struct AlwaysPassTool;
    impl Tool for AlwaysPassTool {
        fn name(&self) -> &'static str {
            "always_pass"
        }

        fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
            Ok(ToolOutput {
                success: true,
                result: Some(serde_json::json!({"ok": true})),
                error: None,
                verification: Some("forced_success".to_string()),
                audit_log: Some("always_pass executed".to_string()),
                pua_report: None,
            })
        }
    }

    #[test]
    fn tool_registry_runs_fallback_chain_when_primary_fails() {
        let mut registry = ToolRegistry {
            tools: Vec::new(),
            profiles: HashMap::new(),
            aliases: HashMap::new(),
        };
        registry.register_with_profile(
            AlwaysFailTool,
            ToolCapabilityProfile {
                capability: "primary".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 1_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: vec!["always_pass".to_string()],
            },
        );
        registry.register_with_profile(
            AlwaysPassTool,
            ToolCapabilityProfile {
                capability: "fallback".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 1_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        let output = registry
            .run_with_fallback("always_fail", &tool_input(serde_json::json!({})))
            .expect("fallback execution should succeed");
        assert!(output.success);
        let audit_log = output.audit_log.unwrap_or_default();
        assert!(audit_log.contains("fallback"));
    }

    // ── Think-Act-Observe loop tests ─────────────────────────────

    #[test]
    fn tao_loop_completes_on_first_tool_success() {
        let mut registry = ToolRegistry::new();
        registry.register(AlwaysPassTool);

        let input = tool_input(serde_json::json!({"test": true}));
        let config = LoopConfig::default();

        let (decision, trace) = execute_loop(
            "test success",
            &registry,
            &input,
            &["always_pass".to_string()],
            &config,
            None,
        );

        match decision {
            LoopDecision::Complete(output) => {
                assert!(output.success);
                assert_eq!(
                    trace.final_decision, "success",
                    "trace should record success"
                );
                assert!(!trace.iterations.is_empty(), "trace must have entries");
            }
            other => panic!("expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn tao_loop_retries_on_failure_then_switches_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(AlwaysFailTool);
        registry.register(AlwaysPassTool);

        let input = tool_input(serde_json::json!({"test": true}));
        let config = LoopConfig {
            max_iterations: 10,
            max_retries_per_tool: 1,
            enable_fallback: true,
            verify_output: None,
        };

        let (decision, trace) = execute_loop(
            "test fail then pass",
            &registry,
            &input,
            &["always_fail".to_string(), "always_pass".to_string()],
            &config,
            None,
        );

        match decision {
            LoopDecision::Complete(output) => {
                assert!(output.success);
                assert_eq!(trace.final_decision, "success");
                // Should have attempted always_fail at least once
                let fail_attempts: Vec<_> = trace
                    .iterations
                    .iter()
                    .filter(|i| i.tool == "always_fail")
                    .collect();
                assert!(!fail_attempts.is_empty(), "must have attempted always_fail");
            }
            other => panic!("expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn tao_loop_exhausts_all_candidates_and_fails() {
        let mut registry = ToolRegistry::new();
        registry.register(AlwaysFailTool);

        let input = tool_input(serde_json::json!({"test": true}));
        let config = LoopConfig {
            max_iterations: 5,
            max_retries_per_tool: 1,
            enable_fallback: false,
            verify_output: None,
        };

        let (decision, _trace) = execute_loop(
            "test all fail",
            &registry,
            &input,
            &["always_fail".to_string()],
            &config,
            None,
        );

        match decision {
            LoopDecision::Failed { reason, .. } => {
                assert!(!reason.is_empty(), "failure reason must be non-empty");
            }
            other => panic!("expected Failed, got {:?}", other),
        }
    }

    #[test]
    fn tao_loop_respects_custom_verify_function() {
        let mut registry = ToolRegistry::new();
        registry.register(AlwaysPassTool);

        // A verify function that always rejects the output.
        fn reject_always(_: &ToolOutput) -> bool {
            false
        }

        let input = tool_input(serde_json::json!({"test": true}));
        let config = LoopConfig {
            max_iterations: 3,
            max_retries_per_tool: 1,
            enable_fallback: false,
            verify_output: Some(reject_always),
        };

        let (decision, _trace) = execute_loop(
            "test verify reject",
            &registry,
            &input,
            &["always_pass".to_string()],
            &config,
            None,
        );

        // Tool succeeds but verification fails → should switch or fail.
        match decision {
            LoopDecision::SwitchTool { .. } | LoopDecision::Failed { .. } => {}
            other => panic!("expected SwitchTool or Failed, got {:?}", other),
        }
    }

    #[test]
    fn tao_loop_with_empty_preferred_tools_falls_back_to_registry_and_completes() {
        // When preferred_tools is empty, execute_loop falls back to registry.names().
        // ToolRegistry's tools field is private, so we create from ToolRegistry::new()
        // and simply test that the loop handles empty preferred_tools gracefully.
        // Use AlwaysPassTool which never depends on environment state.
        let mut registry = ToolRegistry::new_empty();
        registry.register(AlwaysPassTool);
        let input = tool_input(serde_json::json!({"dummy": true}));
        let config = LoopConfig::default();

        let (decision, trace) = execute_loop(
            "test fallback to registry",
            &registry,
            &input,
            &[], // no preferred tools — falls back to registry.names()
            &config,
            None,
        );

        match decision {
            LoopDecision::Complete(output) => {
                assert!(output.success);
                assert_eq!(trace.final_decision, "success");
            }
            other => panic!(
                "expected Complete (fallback to registry tools), got {:?}",
                other
            ),
        }
    }
}
