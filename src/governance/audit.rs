//! Audit — F-GAP-03
//!
//! Audit logging system for go-on (Phase 1/2)
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! AuditLog provides circular buffering of agent decisions for compliance and debugging,
//! to be integrated into all agent execution points.

use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    #[serde(default)]
    pub data_classification: Option<String>,
    #[serde(default)]
    pub compliance_tags: Vec<String>,
    #[serde(default)]
    pub retention_policy: Option<String>,
}

/// Audit log sink for collecting decision traces.
///
/// NOTE: `AuditLog` uses `&mut self` methods — it is NOT thread-safe.
/// Wrap in `Arc<Mutex<AuditLog>>` when sharing across threads.
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
        let mut entry = entry;
        entry.inputs = redact_sensitive(&entry.inputs);
        entry.outputs = entry.outputs.map(|o| redact_sensitive(&o));
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

fn redact_sensitive(value: &serde_json::Value) -> serde_json::Value {
    match value {
        Value::Object(map) => {
            let mut redacted = serde_json::Map::new();
            for (k, v) in map {
                let lower = k.to_lowercase();
                if lower.contains("api_key")
                    || lower.contains("secret")
                    || lower.contains("password")
                    || lower.contains("token")
                {
                    redacted.insert(k.clone(), Value::String("**REDACTED**".to_string()));
                } else {
                    redacted.insert(k.clone(), redact_sensitive(v));
                }
            }
            Value::Object(redacted)
        }
        Value::String(s) => {
            // Redact common API key patterns in string values
            if s.len() > 20
                && (s.starts_with("sk-") || s.starts_with("pk-") || s.starts_with("AKIA"))
            {
                Value::String(format!("{}...{}", &s[..4], &s[s.len() - 4..]))
            } else {
                Value::String(s.clone())
            }
        }
        other => other.clone(),
    }
}
