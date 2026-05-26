//! Go-On Skill System Types
//!
//! These types define the go-on skill management API responses.
//! They are go-on extensions, NOT part of the ACP/MCP protocol standard.
//!
//! All structs use camelCase serialization for JSON consistency.

/// SKILL.md / skill.mdc manifest — Claude Code compatible skill definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // F-GAP-25 — reserved ACP protocol type from v0.13.2 spec
pub struct SkillImportManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_object_schema")]
    pub input_schema: serde_json::Value,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub prompt_template: Option<String>,
}

#[allow(dead_code)] // F-GAP-25 — reserved ACP protocol type from v0.13.2 spec
fn default_object_schema() -> serde_json::Value {
    serde_json::json!({"type": "object"})
}

use serde::{Deserialize, Serialize};
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

#[allow(dead_code)] // F-GAP-25 — reserved ACP protocol type from v0.13.2 spec
impl SkillActionResponse {
    pub fn ok(action: impl Into<String>) -> Self {
        Self {
            ok: true,
            action: action.into(),
            name: None,
            skill: None,
            total: None,
            enabled: None,
            disabled: None,
            skills: None,
            removed: None,
            unregistered: None,
            version: None,
            versions: None,
        }
    }

    pub fn name(mut self, v: impl Into<String>) -> Self {
        self.name = Some(v.into());
        self
    }
    pub fn skill(mut self, v: Value) -> Self {
        self.skill = Some(v);
        self
    }
    pub fn total(mut self, v: usize) -> Self {
        self.total = Some(v);
        self
    }
    pub fn enabled(mut self, v: usize) -> Self {
        self.enabled = Some(v);
        self
    }
    pub fn disabled(mut self, v: usize) -> Self {
        self.disabled = Some(v);
        self
    }
    pub fn skills(mut self, v: Vec<Value>) -> Self {
        self.skills = Some(v);
        self
    }
    pub fn removed(mut self, v: bool) -> Self {
        self.removed = Some(v);
        self
    }
    pub fn unregistered(mut self, v: bool) -> Self {
        self.unregistered = Some(v);
        self
    }
    pub fn version(mut self, v: impl Into<String>) -> Self {
        self.version = Some(v.into());
        self
    }
    pub fn versions(mut self, v: Vec<Value>) -> Self {
        self.versions = Some(v);
        self
    }
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
    pub inflight: Value,
}

#[allow(dead_code)] // F-GAP-25 — reserved ACP protocol type from v0.13.2 spec
impl PhaseResponse {
    pub fn new(rate_limiter: Value, inflight: Value) -> Self {
        Self {
            rate_limiter,
            inflight,
        }
    }
}

/// Models list response.
#[derive(Debug, Clone, Serialize)]
pub struct ModelsListResponse {
    pub models: Vec<Value>,
}

#[allow(dead_code)] // F-GAP-25 — reserved ACP protocol type from v0.13.2 spec
impl ModelsListResponse {
    pub fn new(models: Vec<Value>) -> Self {
        Self { models }
    }
}
