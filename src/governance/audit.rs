//! Audit logging system for go-on (Phase 1/2)
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! AuditLog provides circular buffering of agent decisions for compliance and debugging,
//! to be integrated into all agent execution points.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Audit log entry for all agent/tool/phase decisions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub timestamp: String,
    pub task_id: String,
    pub phase: String,
    pub agent: Option<String>,
    pub tool: Option<String>,
    pub decision: String,
    pub inputs: serde_json::Value,
    pub outputs: Option<serde_json::Value>,
    pub error: Option<String>,
    pub confidence: Option<f32>,
}

/// Audit log sink for collecting decision traces
pub struct AuditLog {
    entries: VecDeque<AuditLogEntry>,
    max_entries: usize,
}

impl AuditLog {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_entries,
        }
    }

    pub fn record(&mut self, entry: AuditLogEntry) {
        self.entries.push_back(entry);
        while self.entries.len() > self.max_entries {
            self.entries.pop_front();
        }
    }

    pub fn entries(&self) -> Vec<AuditLogEntry> {
        self.entries.iter().cloned().collect()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
