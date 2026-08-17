//! Office, PDF, DOCX, HTML-scrape, invoice and QR-code document tools.

use crate::orchestration::tool::{RetryPolicy, ToolCapabilityProfile, ToolRegistry, ToolRiskLevel};

pub(crate) fn register_documents(registry: &mut ToolRegistry) {
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
}
