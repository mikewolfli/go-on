//! Miscellaneous helper functions for ACP server
//!
//! This module contains various utility functions that don't fit neatly into
//! other helper categories but are used throughout the ACP server implementation.

use crate::config::PhaseOptions;
use serde_json::Value;
use std::path::PathBuf;

/// Extract u64 value from PhaseOptions extra field
pub fn extra_u64(options: Option<&PhaseOptions>, key: &str) -> Option<u64> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_u64())
}

/// Extract f64 value from PhaseOptions extra field
pub fn extra_f64(options: Option<&PhaseOptions>, key: &str) -> Option<f64> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_f64())
}

/// Extract string value from PhaseOptions extra field
pub fn extra_string(options: Option<&PhaseOptions>, key: &str) -> Option<String> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

/// Extract bool value from PhaseOptions extra field
pub fn extra_bool(options: Option<&PhaseOptions>, key: &str) -> Option<bool> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_bool())
}

/// Extract string list from PhaseOptions extra field
pub fn extra_string_list(options: Option<&PhaseOptions>, key: &str) -> Option<Vec<String>> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str())
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        })
}

/// Calculate percentile value from a sorted slice of u64 samples
pub fn percentile(samples: &[u64], percentile: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let clamped = percentile.clamp(0.0, 100.0);
    let rank = ((clamped / 100.0) * ((samples.len() - 1) as f64)).round() as usize;
    samples[rank]
}

/// Decision structure for requirement gate evaluation
#[derive(Debug, Clone)]
pub struct RequirementGateDecision {
    /// Whether the request is blocked
    pub blocked: bool,
    /// Reason for blocking (if blocked)
    pub reason: Option<String>,
    /// Missing required fields
    pub missing_fields: Vec<String>,
    /// Path to clarification artifact (if needed)
    pub clarification_artifact_path: Option<PathBuf>,
    /// Path to governance artifact
    pub governance_artifact_path: PathBuf,
}

/// Metrics for learning clarification process
#[derive(Debug, Clone, Copy)]
pub struct LearningClarificationMetrics {
    /// Number of clarification rounds
    pub rounds: u32,
    /// Quality score (0.0-1.0)
    pub quality_score: f64,
    /// Number of requirement changes
    pub requirement_change_count: u32,
}

/// Parse a string list from JSON Value
pub fn parse_string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str())
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}
