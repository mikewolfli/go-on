//! Image, CAD/DXF, SVG, 3D-model, CAM/G-code and GIS/GPX tools.

use crate::orchestration::tool::ToolRegistry;
#[cfg(any(
    feature = "image-processing",
    feature = "cad-dxf",
    feature = "drawing-svg",
    feature = "cad-stl",
    feature = "model-3d",
    feature = "cad-obj",
    feature = "model-3d-extra",
    feature = "cad-step",
    feature = "cad-geo",
    feature = "cad-utils",
    feature = "cad-gltf",
    feature = "cad-iges",
    feature = "cad-ply",
    feature = "cam-gcode",
    feature = "gis-gpx"
))]
use crate::orchestration::tool::{RetryPolicy, ToolCapabilityProfile, ToolRiskLevel};

// Every registration below is feature-gated, so in a build with none of the
// cad/media features enabled the `registry` parameter is genuinely unused.
#[allow(unused_variables)]
pub(crate) fn register_cad_media(registry: &mut ToolRegistry) {
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
    #[cfg(any(feature = "cad-stl", feature = "model-3d"))]
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
    #[cfg(any(feature = "cad-obj", feature = "model-3d-extra"))]
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
}
