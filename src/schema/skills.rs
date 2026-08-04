//! Go-On Skill System Types
//!
//! These types define the go-on skill management API responses.
//! They are go-on extensions, NOT part of the ACP/MCP protocol standard.
//!
//! All structs use camelCase serialization for JSON consistency.

use serde::Serialize;
use serde_json::Value;

/// Generic skill action response used by all skill.* handlers.
/// All optional fields are omitted when None via skip_serializing_if.
#[derive(Debug, Clone, Serialize)]
pub struct SkillActionResponse {
    pub ok: bool,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unregistered: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versions: Option<Vec<Value>>,
}

/// Normalised view of an imported skill record.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedSkillRecordView {
    pub name: String,
    pub version: String,
    pub description: String,
    pub source: String,
    pub source_ref: String,
    pub sha256: String,
    pub manifest_path: String,
    pub enabled: bool,
    pub imported_at: i64,
}

/// A single version-snapshot entry in a skill's version history.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillVersionSnapshot {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_template: Option<Value>,
    pub manifest_path: String,
    pub saved_at: String,
    pub updated_at: String,
    pub updated_by: String,
    pub change_summary: String,
}

/// Phase status response.
#[derive(Debug, Clone, Serialize)]
pub struct PhaseResponse {
    pub rate_limiter: Value,
}

/// Models list response.
#[derive(Debug, Clone, Serialize)]
pub struct ModelsListResponse {
    pub models: Vec<Value>,
}
