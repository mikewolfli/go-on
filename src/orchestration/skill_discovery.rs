//! Skill Discovery and Capability Matching (Step 10 / Full-Auto)
//!
//! Provides a semantic skill index with search, scoring, and caching
//! capabilities. Enables the full-auto flow to automatically identify
//! the skills and tools required for a given task.
//!
//! Design:
//! - `SkillIndex` maintains an in-memory index of registered skills
//!   with semantic search via token-based similarity.
//! - `SkillDiscovery` wraps the index and provides task-to-skill
//!   matching with scoring and ranking.
//! - Results are cached to avoid repeated searches for identical
//!   or overlapping task descriptions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::orchestration::skill::{Skill, SkillRegistry};
use crate::orchestration::tool::ToolRegistry;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Minimum composite score for a skill to be considered a match.
const MIN_MATCH_SCORE: f64 = 0.40;

/// Weight for name similarity in composite scoring.
const WEIGHT_NAME: f64 = 0.35;

/// Weight for description semantic similarity.
const WEIGHT_DESCRIPTION: f64 = 0.40;

/// Weight for runtime score (historical success rate).
const WEIGHT_RUNTIME: f64 = 0.25;

/// Default TTL for cached discovery results (5 minutes).
const CACHE_TTL: Duration = Duration::from_secs(300);

/// Maximum number of cached entries.
const MAX_CACHE_ENTRIES: usize = 200;

// ---------------------------------------------------------------------------
// SkillIndexEntry
// ---------------------------------------------------------------------------

/// A single entry in the skill index, holding all metadata needed for
/// semantic search and scoring.
#[derive(Debug, Clone
