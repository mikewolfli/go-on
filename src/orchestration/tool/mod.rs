//! Tool trait and tool runtime for go-on
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Tool trait, registry, and implementations will be connected to the execution flow
//! once orchestration logic integrates them.

// ── Sub-modules (moved from orchestration/ for cohesion) ───────────────────
pub mod builtin_tools;
pub mod executor;
pub mod extended;
pub mod lock;
pub mod loop_executor;
pub mod native;
pub mod pipeline;
pub mod recommender;
pub mod types;
use crate::i18n::runtime::tf;
use anyhow::Result;
pub use loop_executor::*;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
pub use types::*;

use crate::orchestration::skill::SkillRegistry;
use crate::orchestration::tool::lock::ToolLockManager as TLM;

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
            hooks: ToolHookRegistry::default(),
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
                fallback_chain: vec!["move_path".to_string()],
            },
        );

        registry.register_with_profile(
            crate::orchestration::tool_extended::EditFileTool,
            ToolCapabilityProfile {
                capability: "filesystem_edit".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 15_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
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

        // ── 3D model (STL) reader tool (feature-gated, not when cad-stl already provides it) ─
        #[cfg(all(feature = "model-3d", not(feature = "cad-stl")))]
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

        // ── Web search tool ─────────────────────────────────────────--
        registry.register_with_profile(
            crate::orchestration::tool_extended::WebSearchTool,
            ToolCapabilityProfile {
                capability: "web_search".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
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

        // ── LSP-like code intelligence tools ─────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::GoToDefinitionTool,
            ToolCapabilityProfile {
                capability: "go_to_definition".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["grep".to_string()],
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::FindReferencesTool,
            ToolCapabilityProfile {
                capability: "find_references".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["grep".to_string()],
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::ApplyCodeActionTool,
            ToolCapabilityProfile {
                capability: "apply_code_action".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
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

        // ── Spawn sub-agent tool ───────────────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::SpawnAgentTool,
            ToolCapabilityProfile {
                capability: "agent_spawn".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Code format tool ───────────────────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::FormatCodeTool,
            ToolCapabilityProfile {
                capability: "format_code".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Package search tool ─────────────────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::SearchPackagesTool,
            ToolCapabilityProfile {
                capability: "package_search".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Utility tools (uuid, random_token, encode_decode, hash_file) ─
        registry.register_with_profile(
            crate::orchestration::tool_extended::UuidGenTool,
            ToolCapabilityProfile {
                capability: "uuid_gen".to_string(),
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
            crate::orchestration::tool_extended::RandomTokenTool,
            ToolCapabilityProfile {
                capability: "random_token".to_string(),
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
            crate::orchestration::tool_extended::EncodeDecodeTool,
            ToolCapabilityProfile {
                capability: "encode_decode".to_string(),
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
            crate::orchestration::tool_extended::HashFileTool,
            ToolCapabilityProfile {
                capability: "hash_file".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Build/lint/dependency tools (P1) ───────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::RunBuildTool,
            ToolCapabilityProfile {
                capability: "build_run".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::LintCodeTool,
            ToolCapabilityProfile {
                capability: "lint_run".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 120_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::AddDependencyTool,
            ToolCapabilityProfile {
                capability: "dependency_add".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Structured data query tools (P1) ────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::JsonQueryTool,
            ToolCapabilityProfile {
                capability: "json_query".to_string(),
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
            crate::orchestration::tool_extended::YamlQueryTool,
            ToolCapabilityProfile {
                capability: "yaml_query".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Template rendering tool (P1) ────────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::TemplateRenderTool,
            ToolCapabilityProfile {
                capability: "template_render".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Code metrics tool (P2) ──────────────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::CodeMetricsTool,
            ToolCapabilityProfile {
                capability: "code_metrics".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Security scan tool (P2) ────────────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::SecurityScanTool,
            ToolCapabilityProfile {
                capability: "security_scan".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 120_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Docker container tools (P2) ────────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::DockerPsTool,
            ToolCapabilityProfile {
                capability: "docker_ps".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 15_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::DockerExecTool,
            ToolCapabilityProfile {
                capability: "docker_exec".to_string(),
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
            crate::orchestration::tool_extended::DockerLogsTool,
            ToolCapabilityProfile {
                capability: "docker_logs".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 15_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::DockerBuildTool,
            ToolCapabilityProfile {
                capability: "docker_build".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::DockerPushTool,
            ToolCapabilityProfile {
                capability: "docker_push".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            crate::orchestration::tool_extended::DockerComposeTool,
            ToolCapabilityProfile {
                capability: "docker_compose".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── File watch tool (P2) ────────────────────────────────
        registry.register_with_profile(
            crate::orchestration::tool_extended::FileWatchTool,
            ToolCapabilityProfile {
                capability: "file_watch".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );

        // ── Backward-compatibility aliases ───────────────────────
        // These names exist in the governance evaluator's allowlist.
        // Some now have their own Tool implementations; others alias
        // to existing tools with the same functionality.
        // create_directory and copy_path have dedicated implementations above.
        // file_move/file_delete are the old primary names; now they alias to the new canonical names.
        registry.register_alias("file_move", "move_path");
        registry.register_alias("file_delete", "delete_path");
        registry.register_alias("execute_command", "shell_exec");
        registry.register_alias("terminal", "shell_exec");
        registry.register_alias("bash", "shell_exec");
        registry.register_alias("find_path", "search_files");
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
        // Auto-register with the governance gate so the tool is never
        // rejected as "unknown" — eliminates manual sync burden.
        crate::governance::status::register_tool(name);
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

    /// Run a tool synchronously with fallback chain support.
    #[tracing::instrument(level = "debug", skip(self, input), fields(tool = %name, success = false, latency_ms = 0u64, fallback_used = false))]
    pub fn run_with_fallback(&self, name: &str, input: &ToolInput) -> Result<ToolOutput> {
        let start = std::time::Instant::now();

        let Some(primary) = self.get(name) else {
            let elapsed = start.elapsed().as_millis() as u64;
            tracing::warn!(target: "tool_execution", tool = %name, latency_ms = elapsed, "tool not found");
            anyhow::bail!("{}", tf("error.tool_not_found", &[("name", name)]));
        };

        // ── Pre-execute hooks ──────────────────────────────────────────
        self.hooks.run_pre(name, input);

        let mut last_result = primary.run(input)?;
        let elapsed = start.elapsed().as_millis() as u64;

        // ── Post-execute hooks ─────────────────────────────────────────
        self.hooks.run_post(name, input, &last_result, elapsed);

        if last_result.success {
            record_tool_execution(
                "tool_execution_total",
                name,
                true,
                elapsed,
                serde_json::to_string(&input.payload).ok().map(|s| s.len()),
            );
            return Ok(last_result);
        }

        for fb_name in self
            .profile(name)
            .map(|p| p.fallback_chain.clone())
            .unwrap_or_default()
        {
            if let Some(fb) = self.get(&fb_name) {
                let mut fb_result = fb.run(input)?;
                if fb_result.success {
                    let elapsed = start.elapsed().as_millis() as u64;
                    fb_result.audit_log = Some(format!(
                        "primary '{name}' failed, fallback '{fb_name}' succeeded"
                    ));
                    record_tool_execution(
                        "tool_execution_total",
                        name,
                        true,
                        elapsed,
                        serde_json::to_string(&input.payload).ok().map(|s| s.len()),
                    );
                    return Ok(fb_result);
                }
                last_result = fb_result;
            }
        }

        let elapsed = start.elapsed().as_millis() as u64;
        record_tool_execution(
            "tool_execution_total",
            name,
            false,
            elapsed,
            serde_json::to_string(&input.payload).ok().map(|s| s.len()),
        );
        Ok(last_result)
    }

    /// Run a tool asynchronously with fallback chain support.
    /// Uses `run_async` directly without `block_in_place` to comply with principle #23.
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

        // ── Pre-execute hooks (async, supports GuardianReviewer) ───────
        self.hooks.run_pre_async(name, input).await?;

        let mut last_result = primary.run_async(input.clone()).await?;
        let elapsed = start.elapsed().as_millis() as u64;

        // ── Post-execute hooks ─────────────────────────────────────────
        self.hooks.run_post(name, input, &last_result, elapsed);

        if last_result.success {
            record_tool_execution(
                "tool_execution_total",
                name,
                true,
                elapsed,
                serde_json::to_string(&input.payload).ok().map(|s| s.len()),
            );
            return Ok(last_result);
        }

        for fb_name in self
            .profile(name)
            .map(|p| p.fallback_chain.clone())
            .unwrap_or_default()
        {
            if let Some(fb) = self.get_arc(&fb_name) {
                let mut fb_result = fb.run_async(input.clone()).await?;
                if fb_result.success {
                    let elapsed = start.elapsed().as_millis() as u64;
                    fb_result.audit_log = Some(format!(
                        "primary '{name}' failed, fallback '{fb_name}' succeeded"
                    ));
                    record_tool_execution(
                        "tool_execution_total",
                        name,
                        true,
                        elapsed,
                        serde_json::to_string(&input.payload).ok().map(|s| s.len()),
                    );
                    return Ok(fb_result);
                }
                last_result = fb_result;
            }
        }

        let elapsed = start.elapsed().as_millis() as u64;
        record_tool_execution(
            "tool_execution_total",
            name,
            false,
            elapsed,
            serde_json::to_string(&input.payload).ok().map(|s| s.len()),
        );
        Ok(last_result)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub use builtin_tools::{
    record_tool_execution, sanitize_path, sanitize_path_for_write, ApplyPatchTool,
    InspectGitDiffTool, ReadFileTool, RunTestsTool, SearchFilesTool, SkillCreateTool,
    SkillExecuteTool, SkillListTool, SkillReloadTool, WriteFileTool,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
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
            hooks: Default::default(),
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

    #[test]
    fn test_no_duplicate_tool_names() {
        let registry = ToolRegistry::new();
        let names = registry.names();
        let mut seen = std::collections::HashSet::new();
        for name in &names {
            assert!(
                seen.insert(name),
                "Duplicate tool name: {name}\nAll names: {names:#?}",
            );
        }
    }
}
