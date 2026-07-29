//! ToolRegistryBuilder — category-based tool registration.
//!
//! Splits the monolithic `ToolRegistry::new()` (~1600 lines) into organized
//! category methods that each register a group of related tools.
//! Usage: `ToolRegistryBuilder::new().with_all().build()`

use super::*;
use crate::orchestration::tool_extended;

// ---------------------------------------------------------------------------
// ToolRegistryBuilder
// ---------------------------------------------------------------------------

/// Builder for constructing a fully-populated `ToolRegistry`.
///
/// Each `with_<category>(&mut self)` method registers a group of related tools.
/// Call `with_all()` to register everything, or pick specific groups for
/// custom registries.
pub struct ToolRegistryBuilder {
    registry: ToolRegistry,
}

impl ToolRegistryBuilder {
    /// Create a new builder with an empty registry.
    pub fn new() -> Self {
        Self {
            registry: ToolRegistry::new_empty(),
        }
    }

    /// Register all built-in and extended tools across all categories.
    pub fn with_all(&mut self) -> &mut Self {
        self.with_core_tools()
            .with_shell_http_tools()
            .with_git_tools()
            .with_filesystem_tools()
            .with_office_doc_tools()
            .with_data_tools()
            .with_web_tools()
            .with_image_tools()
            .with_cad_tools()
            .with_game_tools()
            .with_media_tools()
            .with_code_tools()
            .with_intelligence_tools()
            .with_utility_tools()
            .with_docker_tools()
            .with_security_tools()
            .with_template_tools()
            .with_aliases()
    }

    /// Build and return the populated `ToolRegistry`.
    pub fn build(self) -> ToolRegistry {
        self.registry
    }

    // ── Category: core built-in tools ─────────────────────────────────
    pub fn with_core_tools(&mut self) -> &mut Self {
        self.registry.register_with_profile(
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
        self.registry.register_with_profile(
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
        self.registry.register_with_profile(
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
        self.registry.register_with_profile(
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
        self.registry.register_with_profile(
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
        self.registry.register_with_profile(
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
        self
    }

    // ── Category: shell / HTTP / search ――――――――――――――――――――――――――――――
    pub fn with_shell_http_tools(&mut self) -> &mut Self {
        self.registry.register_with_profile(
            tool_extended::ShellExecTool,
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
        self.registry.register_with_profile(
            tool_extended::HttpRequestTool,
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
        self.registry.register_with_profile(
            tool_extended::GrepTool,
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
        self
    }

    // ── Category: git / version control ―――――――――――――――――――――――――――――
    pub fn with_git_tools(&mut self) -> &mut Self {
        self.registry.register_with_profile(
            tool_extended::GitTool,
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
        self
    }

    // ── Category: filesystem / directory tools ――――――――――――――――――――――
    pub fn with_filesystem_tools(&mut self) -> &mut Self {
        self.registry.register_with_profile(
            tool_extended::ListDirectoryTool,
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
        self.registry.register_with_profile(
            tool_extended::CargoCheckTool,
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
        self.registry.register_with_profile(
            tool_extended::FileMoveTool,
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
        self.registry.register_with_profile(
            tool_extended::FileDeleteTool,
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
        self.registry.register_with_profile(
            tool_extended::CreateDirectoryTool,
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
        self.registry.register_with_profile(
            tool_extended::CopyPathTool,
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
        self.registry.register_with_profile(
            tool_extended::EditFileTool,
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
        self.registry.register_with_profile(
            tool_extended::ReadFileLinesTool,
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
        self.registry.register_with_profile(
            tool_extended::FileWatchTool,
            ToolCapabilityProfile {
                capability: "file_watch".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        self
    }

    // ── Category: office / document tools (feature-gated) ―――――――――――
    pub fn with_office_doc_tools(&mut self) -> &mut Self {
        #[cfg(feature = "document-excel")]
        self.registry.register_with_profile(
            tool_extended::ReadExcelTool,
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
        self.registry.register_with_profile(
            tool_extended::ReadPptTool,
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
        self.registry.register_with_profile(
            tool_extended::WriteExcelTool,
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
        #[cfg(feature = "document-excel")]
        self.registry.register_with_profile(
            tool_extended::OfficeConvertTool,
            ToolCapabilityProfile {
                capability: "office_convert".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "document-pdf")]
        self.registry.register_with_profile(
            tool_extended::PdfReadTool,
            ToolCapabilityProfile {
                capability: "pdf_read".to_string(),
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
        self.registry.register_with_profile(
            tool_extended::PdfMergeTool,
            ToolCapabilityProfile {
                capability: "pdf_merge".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "document-docx")]
        self.registry.register_with_profile(
            tool_extended::ReadDocxTool,
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
        self.registry.register_with_profile(
            tool_extended::WriteDocxTool,
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
        self.registry.register_with_profile(
            tool_extended::WritePptTool,
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
        #[cfg(feature = "document-email")]
        self.registry.register_with_profile(
            tool_extended::EmailTool,
            ToolCapabilityProfile {
                capability: "email".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "document-invoice")]
        self.registry.register_with_profile(
            tool_extended::InvoiceTool,
            ToolCapabilityProfile {
                capability: "invoice".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self
    }

    // ── Category: data tools (CSV, JSON, SQLite, serialization) ―――――
    pub fn with_data_tools(&mut self) -> &mut Self {
        #[cfg(feature = "backend-sqlite")]
        self.registry.register_with_profile(
            tool_extended::SqliteQueryTool,
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
        #[cfg(feature = "data-export")]
        self.registry.register_with_profile(
            tool_extended::CsvReadTool,
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
        self.registry.register_with_profile(
            tool_extended::CsvWriteTool,
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
        self.registry.register_with_profile(
            tool_extended::DataSerDeTool,
            ToolCapabilityProfile {
                capability: "data_serialization".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self.registry.register_with_profile(
            tool_extended::JsonlReadTool,
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
        self.registry.register_with_profile(
            tool_extended::JsonlWriteTool,
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
        self.registry.register_with_profile(
            tool_extended::JsonQueryTool,
            ToolCapabilityProfile {
                capability: "data_query".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self
    }

    // ── Category: web / network tools ―――――――――――――――――――――――――――――
    pub fn with_web_tools(&mut self) -> &mut Self {
        #[cfg(feature = "document-html")]
        self.registry.register_with_profile(
            tool_extended::WebScrapeTool,
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
        self.registry.register_with_profile(
            tool_extended::WebSearchTool,
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
        self.registry.register_with_profile(
            tool_extended::RssReadTool,
            ToolCapabilityProfile {
                capability: "rss_feed".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self.registry.register_with_profile(
            tool_extended::PingTool,
            ToolCapabilityProfile {
                capability: "network_diagnostics".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self
    }

    // ── Category: image / multimedia tools (feature-gated) ―――――――――
    pub fn with_image_tools(&mut self) -> &mut Self {
        #[cfg(feature = "image-processing")]
        self.registry.register_with_profile(
            tool_extended::ImageTool,
            ToolCapabilityProfile {
                capability: "image_processing".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "drawing-svg")]
        self.registry.register_with_profile(
            tool_extended::SvgExportTool,
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
        self
    }

    // ── Category: CAD / 3D tools (feature-gated) ―――――――――――――――――
    pub fn with_cad_tools(&mut self) -> &mut Self {
        #[cfg(feature = "cad-stl")]
        self.registry.register_with_profile(
            tool_extended::StlReadTool,
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
        #[cfg(feature = "cad-obj")]
        self.registry.register_with_profile(
            tool_extended::ObjReadTool,
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
        #[cfg(feature = "cad-dxf")]
        self.registry.register_with_profile(
            tool_extended::DxfReadTool,
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
        #[cfg(feature = "cad-step")]
        self.registry.register_with_profile(
            tool_extended::StepReadTool,
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
        #[cfg(feature = "cad-geo")]
        self.registry.register_with_profile(
            tool_extended::GeoUtilTool,
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
        #[cfg(feature = "cad-utils")]
        self.registry.register_with_profile(
            tool_extended::CadConvertTool,
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
        #[cfg(feature = "cad-gltf")]
        self.registry.register_with_profile(
            tool_extended::GltfReadTool,
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
        #[cfg(feature = "cad-iges")]
        self.registry.register_with_profile(
            tool_extended::IgesReadTool,
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
        #[cfg(feature = "cad-ply")]
        self.registry.register_with_profile(
            tool_extended::PlyReadTool,
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
        #[cfg(feature = "model-3d")]
        self.registry.register_with_profile(
            tool_extended::StlModelReadTool,
            ToolCapabilityProfile {
                capability: "stl_model_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "model-3d-extra")]
        self.registry.register_with_profile(
            tool_extended::ObjModelReadTool,
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
        #[cfg(feature = "cam-gcode")]
        self.registry.register_with_profile(
            tool_extended::GcodeReadTool,
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
        #[cfg(feature = "gis-gpx")]
        self.registry.register_with_profile(
            tool_extended::GpxReadTool,
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
        self
    }

    // ── Category: game tools (feature-gated) ―――――――――――――――――――――――
    pub fn with_game_tools(&mut self) -> &mut Self {
        #[cfg(any(
            feature = "game-online",
            feature = "game-process",
            feature = "game-screen",
            feature = "game-input",
            feature = "game-agent",
            feature = "game-state",
            feature = "game-modding"
        ))]
        self.registry.register_with_profile(
            tool_extended::GameTool,
            ToolCapabilityProfile {
                capability: "game_automation".to_string(),
                risk_level: ToolRiskLevel::High,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        self
    }

    // ── Category: media / archive / compression ―――――――――――――――――――
    pub fn with_media_tools(&mut self) -> &mut Self {
        self.registry.register_with_profile(
            tool_extended::ArchiveInspectTool,
            ToolCapabilityProfile {
                capability: "archive".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 120_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self.registry.register_with_profile(
            tool_extended::CompressTool,
            ToolCapabilityProfile {
                capability: "compress".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        #[cfg(feature = "barcode-tools")]
        self.registry.register_with_profile(
            tool_extended::QrCodeTool,
            ToolCapabilityProfile {
                capability: "barcode".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self
    }

    // ── Category: code / build / LSP tools ――――――――――――――――――――――――
    pub fn with_code_tools(&mut self) -> &mut Self {
        self.registry.register_with_profile(
            tool_extended::RunBuildTool,
            ToolCapabilityProfile {
                capability: "build_execute".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        self.registry.register_with_profile(
            tool_extended::GoToDefinitionTool,
            ToolCapabilityProfile {
                capability: "lsp".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self.registry.register_with_profile(
            tool_extended::DiffTool,
            ToolCapabilityProfile {
                capability: "diff".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self.registry.register_with_profile(
            tool_extended::CodeIndexTool,
            ToolCapabilityProfile {
                capability: "code_index".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 60_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self.registry.register_with_profile(
            tool_extended::FormatCodeTool,
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
        self.registry.register_with_profile(
            tool_extended::ApplyCodeActionTool,
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
        self.registry.register_with_profile(
            tool_extended::DiagnosticsTool,
            ToolCapabilityProfile {
                capability: "diagnostics".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self.registry.register_with_profile(
            tool_extended::CodeMetricsTool,
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
        self
    }

    // ── Category: intelligence / agent tools ――――――――――――――――――――――
    pub fn with_intelligence_tools(&mut self) -> &mut Self {
        self.registry.register_with_profile(
            tool_extended::SpawnAgentTool,
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
        self.registry.register_with_profile(
            tool_extended::ToolSearchTool,
            ToolCapabilityProfile {
                capability: "tool_search".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self.registry.register_with_profile(
            tool_extended::SearchPackagesTool,
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
        self
    }

    // ── Category: utility tools (uuid, time, hash, etc.) ――――――――――
    pub fn with_utility_tools(&mut self) -> &mut Self {
        self.registry.register_with_profile(
            tool_extended::UuidGenTool,
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
        self.registry.register_with_profile(
            tool_extended::RandomTokenTool,
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
        self.registry.register_with_profile(
            tool_extended::EncodeDecodeTool,
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
        self.registry.register_with_profile(
            tool_extended::HashFileTool,
            ToolCapabilityProfile {
                capability: "hash_file".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self.registry.register_with_profile(
            tool_extended::DateTimeTool,
            ToolCapabilityProfile {
                capability: "time_util".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 5_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self.registry.register_with_profile(
            tool_extended::EnvironmentInfoTool,
            ToolCapabilityProfile {
                capability: "environment_info".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 5_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self
    }

    // ── Category: docker / container tools ―――――――――――――――――――――――
    pub fn with_docker_tools(&mut self) -> &mut Self {
        self.registry.register_with_profile(
            tool_extended::DockerPsTool,
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
        self.registry.register_with_profile(
            tool_extended::DockerExecTool,
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
        self.registry.register_with_profile(
            tool_extended::DockerLogsTool,
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
        self.registry.register_with_profile(
            tool_extended::DockerBuildTool,
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
        self.registry.register_with_profile(
            tool_extended::DockerPushTool,
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
        self.registry.register_with_profile(
            tool_extended::DockerComposeTool,
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
        self
    }

    // ── Category: security tools ―――――――――――――――――――――――――――――――――
    pub fn with_security_tools(&mut self) -> &mut Self {
        self.registry.register_with_profile(
            tool_extended::SecurityScanTool,
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
        self.registry.register_with_profile(
            tool_extended::DockerPsTool,
            ToolCapabilityProfile {
                capability: "container".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        self
    }

    // ── Category: template / rendering ―――――――――――――――――――――――――――
    pub fn with_template_tools(&mut self) -> &mut Self {
        #[cfg(feature = "template-engine")]
        self.registry.register_with_profile(
            tool_extended::TemplateRenderTool,
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
        self
    }

    // ── Aliases ─────────────────────────────────────────────────────
    pub fn with_aliases(&mut self) -> &mut Self {
        self.registry.register_alias("file_move", "move_path");
        self.registry.register_alias("file_delete", "delete_path");
        self.registry
            .register_alias("execute_command", "shell_exec");
        self.registry.register_alias("terminal", "shell_exec");
        self.registry.register_alias("bash", "shell_exec");
        self.registry.register_alias("find_path", "search_files");
        self.registry
            .register_alias("semantic_search", "code_index_search");
        self.registry.register_alias("cargo_test", "run_tests");
        self.registry.register_alias("find_files", "search_files");
        self
    }
}

impl Default for ToolRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}
