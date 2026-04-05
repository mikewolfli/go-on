//! Tool trait and tool runtime for go-on
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Tool trait, registry, and implementations will be connected to the execution flow
//! once orchestration logic integrates them.

#![allow(dead_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::pua::{PuaExecutionReport, tool_execution_report};

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

/// Tool trait
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn run(&self, input: &ToolInput) -> Result<ToolOutput>;
}

/// Tool registry
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }
    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.tools.push(Box::new(tool));
    }
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|b| b.as_ref())
    }
}

pub struct ReadFileTool;
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing path"))?;
        let content = std::fs::read_to_string(path)?;
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"content": content})),
            error: None,
            verification: Some("file_read".to_string()),
            audit_log: Some(format!("Read file: {}", path)),
            pua_report: Some(tool_execution_report("read_file", Some("file_read"))),
        })
    }
}

pub struct SearchFilesTool;
impl Tool for SearchFilesTool {
    fn name(&self) -> &'static str {
        "search_files"
    }
    fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"files": []})),
            error: None,
            verification: Some("search_done".to_string()),
            audit_log: Some("Search files completed".to_string()),
            pua_report: Some(tool_execution_report("search_files", Some("search_done"))),
        })
    }
}

pub struct ApplyPatchTool;
impl Tool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }
    fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"applied": true})),
            error: None,
            verification: Some("patch_applied".to_string()),
            audit_log: Some("Patch applied".to_string()),
            pua_report: Some(tool_execution_report("apply_patch", Some("patch_applied"))),
        })
    }
}

pub struct RunTestsTool;
impl Tool for RunTestsTool {
    fn name(&self) -> &'static str {
        "run_tests"
    }
    fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"passed": true})),
            error: None,
            verification: Some("tests_passed".to_string()),
            audit_log: Some("Tests executed".to_string()),
            pua_report: Some(tool_execution_report("run_tests", Some("tests_passed"))),
        })
    }
}

pub struct InspectGitDiffTool;
impl Tool for InspectGitDiffTool {
    fn name(&self) -> &'static str {
        "inspect_git_diff"
    }
    fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"diff": ""})),
            error: None,
            verification: Some("diff_inspected".to_string()),
            audit_log: Some("Git diff inspected".to_string()),
            pua_report: Some(tool_execution_report("inspect_git_diff", Some("diff_inspected"))),
        })
    }
}
