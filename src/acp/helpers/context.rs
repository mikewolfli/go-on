//! Context helper functions for ACP server
//!
//! This module provides utility functions for managing request context,
//! vector configuration, message optimization, and cache key generation.

use crate::config::PhaseOptions;
use std::time::Duration;

/// Get request timeout from phase options
pub fn request_timeout(options: Option<&PhaseOptions>) -> Option<Duration> {
    options
        .and_then(|opts| opts.request_timeout_seconds)
        .map(Duration::from_secs)
}

/// Work grade classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkGrade {
    Ask,
    Edit,
    Agent,
    Safeguard,
    FullAuto,
}

/// Test function to verify module works
pub fn test_function() -> &'static str {
    "context module is working"
}
