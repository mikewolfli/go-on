//! Configuration system implementation
//!
//! This module defines the configuration structures and validation logic for the go-on application.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Application configuration structure
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct AppConfig {
    /// Default phase to use when none is specified
    pub default_phase: String,
    /// Map of agent configurations
    pub agents: HashMap<String, AgentConfig>,
    /// Flow configuration defining phase sequence
    pub flow: FlowConfig,
    /// Map of phase configurations
    pub phases: HashMap<String, PhaseConfig>,
    /// Runtime configuration
    pub runtime: Option<RuntimeConfig>,
    /// Cache configuration
    pub cache: Option<CacheConfig>,
    /// Vector store configuration
    pub vector: Option<VectorConfig>,
    /// Autotune configuration
    pub autotune: Option<AutoTuneConfig>,
    /// Model selection mode for automatic selection (Phase 10+)
    #[serde(default)]
    pub model_selection_mode: String,
}

/// Configuration warning severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigWarningSeverity {
    /// Informational warning
    Info,
    /// Warning that may affect functionality
    Warn,
    /// Critical issue that will prevent proper operation
    Critical,
}

/// Configuration warning structure
#[derive(Debug, Clone, Serialize)]
pub struct ConfigWarning {
    /// Warning code
    pub code: String,
    /// Warning severity
    pub severity: ConfigWarningSeverity,
    /// Warning message
    pub message: String,
}

/// Configuration health report
#[derive(Debug, Clone, Serialize)]
pub struct ConfigHealthReport {
    /// Health score (0-100)
    pub score: u32,
    /// Total number of warnings
    pub total: usize,
    /// Number of informational warnings
    pub info_count: usize,
    /// Number of warnings
    pub warn_count: usize,
    /// Number of critical warnings
    pub critical_count: usize,
    /// List of warnings
    pub warnings: Vec<ConfigWarning>,
}

impl ConfigHealthReport {
    /// Get all warning messages
    ///
    /// # Returns
    /// * `Vec<String>` - List of warning messages
    pub fn warning_messages(&self) -> Vec<String> {
        self.warnings
            .iter()
            .map(|item| item.message.clone())
            .collect()
    }
}

/// Runtime configuration
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    /// Maintenance interval in seconds
    #[serde(default = "default_runtime_maintenance_interval_seconds")]
    pub maintenance_interval_seconds: u64,
    /// Health check interval in seconds
    #[serde(default = "default_runtime_health_interval_seconds")]
    pub health_interval_seconds: u64,
    /// Shutdown drain time in seconds
    #[serde(default = "default_runtime_shutdown_drain_seconds")]
    pub shutdown_drain_seconds: u64,
    /// How often background maintenance performs SQLite VACUUM cycles
    #[serde(default = "default_runtime_sqlite_vacuum_interval_cycles")]
    pub sqlite_vacuum_interval_cycles: u64,
    /// Enable OpenTelemetry exporter for distributed traces
    #[serde(default)]
    pub otel_enabled: bool,
    /// Exporter type: otlp or jaeger (jaeger uses OTLP endpoint)
    #[serde(default = "default_runtime_otel_exporter")]
    pub otel_exporter: String,
    /// Optional OTLP endpoint (for Jaeger, point to collector OTLP endpoint)
    #[serde(default)]
    pub otel_endpoint: Option<String>,
    /// OpenTelemetry service name
    #[serde(default = "default_runtime_otel_service_name")]
    pub otel_service_name: String,
    /// Sampling ratio in [0.0, 1.0]
    #[serde(default = "default_runtime_otel_sample_ratio")]
    pub otel_sample_ratio: f64,
    /// Number of slow requests to keep in top-N trace metrics
    #[serde(default = "default_runtime_trace_slow_top_n")]
    pub trace_slow_top_n: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            maintenance_interval_seconds: default_runtime_maintenance_interval_seconds(),
            health_interval_seconds: default_runtime_health_interval_seconds(),
            shutdown_drain_seconds: default_runtime_shutdown_drain_seconds(),
            sqlite_vacuum_interval_cycles: default_runtime_sqlite_vacuum_interval_cycles(),
            otel_enabled: false,
            otel_exporter: default_runtime_otel_exporter(),
            otel_endpoint: None,
            otel_service_name: default_runtime_otel_service_name(),
            otel_sample_ratio: default_runtime_otel_sample_ratio(),
            trace_slow_top_n: default_runtime_trace_slow_top_n(),
        }
    }
}

fn default_runtime_maintenance_interval_seconds() -> u64 {
    60
}

fn default_runtime_health_interval_seconds() -> u64 {
    120
}

fn default_runtime_shutdown_drain_seconds() -> u64 {
    30
}

fn default_runtime_sqlite_vacuum_interval_cycles() -> u64 {
    60
}

fn default_runtime_otel_exporter() -> String {
    "otlp".to_string()
}

fn default_runtime_otel_service_name() -> String {
    "go-on".to_string()
}

fn default_runtime_otel_sample_ratio() -> f64 {
    1.0
}

fn default_runtime_trace_slow_top_n() -> usize {
    20
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct AutoTuneConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_autotune_evaluate_interval")]
    pub evaluate_interval: usize,
    #[serde(default = "default_autotune_min_query_chars_step")]
    pub min_query_chars_step: usize,
    #[serde(default = "default_autotune_min_query_chars_min")]
    pub min_query_chars_min: usize,
    #[serde(default = "default_autotune_min_query_chars_max")]
    pub min_query_chars_max: usize,
    #[serde(default = "default_autotune_max_top_k")]
    pub max_top_k: usize,
    #[serde(default = "default_autotune_low_precision")]
    pub low_precision_threshold: f32,
    #[serde(default = "default_autotune_high_precision")]
    pub high_precision_threshold: f32,
    #[serde(default = "default_autotune_state_path")]
    pub state_path: String,
    #[serde(default = "default_autotune_cooldown_windows")]
    pub cooldown_windows: usize,
    #[serde(default = "default_autotune_min_vector_searches")]
    pub min_vector_searches: usize,
    #[serde(default = "default_autotune_summary_trigger_min")]
    pub summary_trigger_min: usize,
    #[serde(default = "default_autotune_summary_trigger_max")]
    pub summary_trigger_max: usize,
}

fn default_autotune_evaluate_interval() -> usize {
    20
}

fn default_autotune_min_query_chars_step() -> usize {
    20
}

fn default_autotune_min_query_chars_min() -> usize {
    40
}

fn default_autotune_min_query_chars_max() -> usize {
    300
}

fn default_autotune_max_top_k() -> usize {
    4
}

fn default_autotune_low_precision() -> f32 {
    0.35
}

fn default_autotune_high_precision() -> f32 {
    0.75
}

fn default_autotune_state_path() -> String {
    "acp_autotune_state.json".to_string()
}

fn default_autotune_cooldown_windows() -> usize {
    2
}

fn default_autotune_min_vector_searches() -> usize {
    5
}

fn default_autotune_summary_trigger_min() -> usize {
    3
}

fn default_autotune_summary_trigger_max() -> usize {
    20
}

/// Runtime autotune state: tracks current parameter values and precision feedback metrics.
/// Persisted to JSON file at state_path to survive across server restarts.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AutoTuneState {
    /// Current minimum query character threshold for vector searches.
    pub current_min_query_chars: usize,
    /// Current top-k value for vector result limiting.
    pub current_top_k: usize,
    /// Which evaluation window we're in (incremented every evaluate_interval searches).
    pub window_phase: usize,
    /// Number of vector searches with high precision (above high_precision_threshold).
    pub high_precision_count: usize,
    /// Number of vector searches with low precision (below low_precision_threshold).
    pub low_precision_count: usize,
    /// Total vector searches in current window.
    pub vector_search_count: usize,
    /// Windows remaining before next adjustment is allowed (cooldown logic).
    pub cooldown_remaining: usize,
}

impl AutoTuneState {
    /// Create new state from AutoTuneConfig defaults.
    pub fn new(config: &AutoTuneConfig) -> Self {
        Self {
            current_min_query_chars: config.min_query_chars_min,
            current_top_k: 2, // Conservative initial value
            window_phase: 0,
            high_precision_count: 0,
            low_precision_count: 0,
            vector_search_count: 0,
            cooldown_remaining: 0,
        }
    }

    /// Load state from JSON file, or return new default if file doesn't exist.
    pub fn load_or_default(path: &str, config: &AutoTuneConfig) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => match serde_json::from_str::<AutoTuneState>(&content) {
                Ok(state) => state,
                Err(e) => {
                    log::warn!(
                        "failed to parse autotune state from {}: {}, using defaults",
                        path,
                        e
                    );
                    Self::new(config)
                }
            },
            Err(_) => Self::new(config),
        }
    }

    /// Save state to JSON file.
    pub fn save(&self, path: &str) -> Result<()> {
        let json =
            serde_json::to_string_pretty(self).context("failed to serialize autotune state")?;
        fs::write(path, json).context("failed to write autotune state to file")?;
        Ok(())
    }

    /// Record a vector search result with precision score.
    /// Called after each vector search to update metrics.
    pub fn record_vector_search(&mut self, precision: f32, config: &AutoTuneConfig) {
        if precision >= config.high_precision_threshold {
            self.high_precision_count += 1;
        } else if precision < config.low_precision_threshold {
            self.low_precision_count += 1;
        }
        self.vector_search_count += 1;
    }

    /// Advance one evaluation window while cooling down.
    /// This prevents the tuner from getting stuck with a non-zero cooldown.
    pub fn advance_cooldown_window(&mut self, config: &AutoTuneConfig) -> bool {
        if self.cooldown_remaining == 0 || self.vector_search_count < config.evaluate_interval {
            return false;
        }

        self.vector_search_count = 0;
        self.high_precision_count = 0;
        self.low_precision_count = 0;
        self.window_phase += 1;
        self.cooldown_remaining -= 1;
        true
    }

    /// Determine if it's time to evaluate and possibly adjust parameters.
    /// Returns true if adjustment window reached and cooldown expired.
    pub fn should_evaluate(&self, config: &AutoTuneConfig) -> bool {
        self.vector_search_count >= config.evaluate_interval && self.cooldown_remaining == 0
    }

    /// Evaluate precision metrics and adjust parameters if needed.
    /// Returns true if parameters were adjusted.
    pub fn evaluate_and_adjust(&mut self, config: &AutoTuneConfig) -> bool {
        if !self.should_evaluate(config) {
            return false;
        }

        if self.vector_search_count < config.min_vector_searches {
            // Not enough data, reset counters and proceed to next window
            self.vector_search_count = 0;
            self.high_precision_count = 0;
            self.low_precision_count = 0;
            self.window_phase += 1;
            return false;
        }

        let high_precision_ratio =
            self.high_precision_count as f32 / self.vector_search_count as f32;
        let low_precision_ratio = self.low_precision_count as f32 / self.vector_search_count as f32;

        let adjusted = if high_precision_ratio > 0.6 {
            // Most results are good - we can be more selective
            self.increase_min_query_chars(config)
        } else if low_precision_ratio > 0.4 {
            // Many poor results - relax the filter
            self.decrease_min_query_chars(config)
        } else {
            false
        };

        // Reset counters and move to next window
        self.vector_search_count = 0;
        self.high_precision_count = 0;
        self.low_precision_count = 0;
        self.window_phase += 1;

        if adjusted {
            self.cooldown_remaining = config.cooldown_windows;
        } else {
            self.cooldown_remaining = 0;
        }

        adjusted
    }

    /// Increase min_query_chars to be more selective (fewer but better results).
    fn increase_min_query_chars(&mut self, config: &AutoTuneConfig) -> bool {
        let new_value = (self.current_min_query_chars + config.min_query_chars_step)
            .min(config.min_query_chars_max);
        if new_value != self.current_min_query_chars {
            log::info!(
                "autotune: increasing min_query_chars from {} to {}",
                self.current_min_query_chars,
                new_value
            );
            self.current_min_query_chars = new_value;
            true
        } else {
            false
        }
    }

    /// Decrease min_query_chars to be more permissive (more results).
    fn decrease_min_query_chars(&mut self, config: &AutoTuneConfig) -> bool {
        let new_value = self
            .current_min_query_chars
            .saturating_sub(config.min_query_chars_step)
            .max(config.min_query_chars_min);
        if new_value != self.current_min_query_chars {
            log::info!(
                "autotune: decreasing min_query_chars from {} to {}",
                self.current_min_query_chars,
                new_value
            );
            self.current_min_query_chars = new_value;
            true
        } else {
            false
        }
    }

    /// Return current tuning state as JSON for RPC responses.
    pub fn snapshot(&self) -> Value {
        serde_json::json!({
            "current_min_query_chars": self.current_min_query_chars,
            "current_top_k": self.current_top_k,
            "window_phase": self.window_phase,
            "high_precision_count": self.high_precision_count,
            "low_precision_count": self.low_precision_count,
            "vector_search_count": self.vector_search_count,
            "cooldown_remaining": self.cooldown_remaining,
        })
    }

    /// Decrement cooldown counter (called once per evaluation window).
    #[allow(dead_code)]
    pub fn tick_cooldown(&mut self) {
        if self.cooldown_remaining > 0 {
            self.cooldown_remaining -= 1;
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_cache_path")]
    pub path: String,
    #[serde(default = "default_cache_ttl_seconds")]
    pub default_ttl_seconds: u64,
    #[serde(default = "default_cache_max_entries")]
    pub max_entries: usize,
}

fn default_cache_path() -> String {
    "acp_cache.sqlite3".to_string()
}

fn default_cache_ttl_seconds() -> u64 {
    3600
}

fn default_cache_max_entries() -> usize {
    5000
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct VectorConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_vector_auto_mode")]
    pub auto_mode: bool,
    #[serde(default = "default_vector_path")]
    pub path: String,
    #[serde(default = "default_vector_dimensions")]
    pub dimensions: usize,
    #[serde(default = "default_vector_min_query_chars")]
    pub min_query_chars: usize,
    #[serde(default = "default_vector_top_k")]
    pub top_k: usize,
    #[serde(default = "default_vector_min_similarity")]
    pub min_similarity: f32,
    #[serde(default = "default_vector_max_snippet_chars")]
    pub max_snippet_chars: usize,
    #[serde(default = "default_vector_max_entries")]
    pub max_entries: usize,
    #[serde(default = "default_summary_enabled")]
    pub summary_enabled: bool,
    #[serde(default = "default_summary_trigger_messages")]
    pub summary_trigger_messages: usize,
    #[serde(default = "default_summary_max_chars")]
    pub summary_max_chars: usize,
}

fn default_vector_auto_mode() -> bool {
    true
}

fn default_vector_path() -> String {
    "acp_vector.sqlite3".to_string()
}

fn default_vector_dimensions() -> usize {
    192
}

fn default_vector_min_query_chars() -> usize {
    80
}

fn default_vector_top_k() -> usize {
    2
}

fn default_vector_min_similarity() -> f32 {
    0.82
}

fn default_vector_max_snippet_chars() -> usize {
    800
}

fn default_vector_max_entries() -> usize {
    10000
}

fn default_summary_enabled() -> bool {
    true
}

fn default_summary_trigger_messages() -> usize {
    8
}

fn default_summary_max_chars() -> usize {
    1200
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    #[serde(rename = "type")]
    pub agent_type: String,
    pub url: Option<String>,
    pub chat_path: Option<String>,
    pub api_key_env: Option<String>,
    pub secret_key_env: Option<String>,
    pub anthropic_version: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub supports_system: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FlowConfig {
    pub name: String,
    pub phases: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PhaseConfig {
    pub description: String,
    pub agents: Vec<String>,
    pub fallback: Option<bool>,
    pub principles: Option<Vec<String>>,
    pub options: Option<PhaseOptions>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PhaseOptions {
    pub cache_enabled: Option<bool>,
    pub cache_ttl_seconds: Option<u64>,
    pub vector_enabled: Option<bool>,
    pub vector_auto: Option<bool>,
    pub vector_min_query_chars: Option<usize>,
    pub vector_top_k: Option<usize>,
    pub vector_min_similarity: Option<f32>,
    pub vector_max_snippet_chars: Option<usize>,
    pub summary_enabled: Option<bool>,
    pub summary_trigger_messages: Option<usize>,
    pub summary_max_chars: Option<usize>,
    pub max_history_messages: Option<usize>,
    pub max_history_chars: Option<usize>,
    pub autopilot_complexity: Option<String>,
    pub full_auto_review_agents: Option<Vec<String>>,
    pub request_timeout_seconds: Option<u64>,
    pub review_timeout_seconds: Option<u64>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl PhaseOptions {
    pub fn agent_options(&self) -> Option<HashMap<String, Value>> {
        if self.extra.is_empty() {
            None
        } else {
            Some(self.extra.clone())
        }
    }
}

impl AppConfig {
    /// Load configuration from file
    ///
    /// # Arguments
    /// * `path` - Path to configuration file
    ///
    /// # Returns
    /// * `Result<Self>` - Returns Ok(Self) if loaded successfully, or an error if something goes wrong
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;
        let mut cfg: AppConfig = toml::from_str(&content)
            .with_context(|| format!("failed to parse toml: {}", path.display()))?;
        apply_auto_rules(path, &mut cfg);
        Ok(cfg)
    }

    /// Validate configuration
    ///
    /// This method performs comprehensive validation of the configuration, including:
    /// - Checking that flow.phases is not empty
    /// - Verifying that default_phase is in flow.phases
    /// - Ensuring all phases in flow.phases are defined
    /// - Validating that each phase has at least one agent
    /// - Checking that all agents referenced in phases exist
    /// - Validating phase options
    /// - Verifying complex autopilot requirements
    ///
    /// # Returns
    /// * `Result<()>` - Returns Ok(()) if validation passes, or an error if validation fails
    pub fn validate(&self) -> Result<()> {
        if self.flow.phases.is_empty() {
            anyhow::bail!("flow.phases must not be empty");
        }

        if !self
            .flow
            .phases
            .iter()
            .any(|phase| phase == &self.default_phase)
        {
            anyhow::bail!(
                "default_phase '{}' is not listed in flow.phases",
                self.default_phase
            );
        }

        for phase_name in &self.flow.phases {
            let phase_cfg = self
                .phases
                .get(phase_name)
                .with_context(|| format!("phase '{}' missing in [phases]", phase_name))?;

            if phase_cfg.agents.is_empty() {
                anyhow::bail!("phase '{}' must contain at least one agent", phase_name);
            }

            for agent_name in &phase_cfg.agents {
                if !self.agents.contains_key(agent_name) {
                    anyhow::bail!(
                        "phase '{}' references undefined agent '{}'",
                        phase_name,
                        agent_name
                    );
                }
            }

            if let Some(options) = phase_cfg.options.as_ref() {
                validate_phase_options(phase_name, options)?;
            }

            if phase_uses_complex_autopilot(phase_cfg.options.as_ref()) {
                if !self.flow.phases.iter().any(|phase| phase == "review") {
                    anyhow::bail!(
                        "phase '{}' uses complex autopilot but flow.phases does not include 'review'",
                        phase_name
                    );
                }

                let review_phase = self
                    .phases
                    .get("review")
                    .with_context(|| "complex autopilot requires a [phases.review] definition")?;

                let reviewers = phase_cfg
                    .options
                    .as_ref()
                    .and_then(|options| options.full_auto_review_agents.clone())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "phase '{}' uses complex autopilot but does not define full_auto_review_agents",
                            phase_name
                        )
                    })?;

                if reviewers.len() < 2 {
                    anyhow::bail!(
                        "phase '{}' uses complex autopilot but must define at least 2 full_auto_review_agents",
                        phase_name
                    );
                }

                if review_phase.agents.len() < 2 {
                    anyhow::bail!(
                        "[phases.review] must contain at least 2 agents when complex autopilot is enabled"
                    );
                }

                for reviewer in reviewers.iter().take(2) {
                    if !self.agents.contains_key(reviewer) {
                        anyhow::bail!(
                            "phase '{}' references undefined review agent '{}'",
                            phase_name,
                            reviewer
                        );
                    }

                    if !review_phase.agents.iter().any(|agent| agent == reviewer) {
                        anyhow::bail!(
                            "review agent '{}' must also appear in [phases.review].agents",
                            reviewer
                        );
                    }
                }
            }
        }

        if let Some(cache) = &self.cache {
            if cache.enabled {
                if cache.default_ttl_seconds == 0 {
                    anyhow::bail!("cache.default_ttl_seconds must be > 0 when cache is enabled");
                }
                if cache.max_entries == 0 {
                    anyhow::bail!("cache.max_entries must be > 0 when cache is enabled");
                }
            }
        }

        if let Some(runtime) = &self.runtime {
            if runtime.maintenance_interval_seconds == 0 {
                anyhow::bail!("runtime.maintenance_interval_seconds must be > 0");
            }
            if runtime.health_interval_seconds == 0 {
                anyhow::bail!("runtime.health_interval_seconds must be > 0");
            }
            if runtime.shutdown_drain_seconds == 0 {
                anyhow::bail!("runtime.shutdown_drain_seconds must be > 0");
            }
            if runtime.sqlite_vacuum_interval_cycles == 0 {
                anyhow::bail!("runtime.sqlite_vacuum_interval_cycles must be > 0");
            }
            if !(0.0..=1.0).contains(&runtime.otel_sample_ratio) {
                anyhow::bail!("runtime.otel_sample_ratio must be in [0.0, 1.0]");
            }
            if runtime.trace_slow_top_n == 0 {
                anyhow::bail!("runtime.trace_slow_top_n must be > 0");
            }
            let exporter = runtime.otel_exporter.to_ascii_lowercase();
            if runtime.otel_enabled && exporter != "otlp" && exporter != "jaeger" {
                anyhow::bail!("runtime.otel_exporter must be 'otlp' or 'jaeger'");
            }
        }

        if let Some(vector) = &self.vector {
            if vector.enabled {
                if vector.dimensions == 0 {
                    anyhow::bail!("vector.dimensions must be > 0 when vector is enabled");
                }
                if vector.top_k == 0 {
                    anyhow::bail!("vector.top_k must be > 0 when vector is enabled");
                }
                if !(0.0..=1.0).contains(&vector.min_similarity) {
                    anyhow::bail!(
                        "vector.min_similarity must be in [0.0, 1.0] when vector is enabled"
                    );
                }
                if vector.max_entries == 0 {
                    anyhow::bail!("vector.max_entries must be > 0 when vector is enabled");
                }
                if vector.summary_trigger_messages == 0 {
                    anyhow::bail!(
                        "vector.summary_trigger_messages must be > 0 when vector is enabled"
                    );
                }
                if vector.summary_max_chars == 0 {
                    anyhow::bail!("vector.summary_max_chars must be > 0 when vector is enabled");
                }
            }
        }

        if let Some(autotune) = &self.autotune {
            if autotune.enabled {
                if autotune.evaluate_interval == 0 {
                    anyhow::bail!("autotune.evaluate_interval must be > 0 when enabled");
                }
                if autotune.min_query_chars_step == 0 {
                    anyhow::bail!("autotune.min_query_chars_step must be > 0 when enabled");
                }
                if autotune.min_query_chars_min == 0 {
                    anyhow::bail!("autotune.min_query_chars_min must be > 0 when enabled");
                }
                if autotune.min_query_chars_min > autotune.min_query_chars_max {
                    anyhow::bail!(
                        "autotune.min_query_chars_min must be <= autotune.min_query_chars_max"
                    );
                }
                if autotune.max_top_k == 0 {
                    anyhow::bail!("autotune.max_top_k must be > 0 when enabled");
                }
                if !(0.0..=1.0).contains(&autotune.low_precision_threshold) {
                    anyhow::bail!("autotune.low_precision_threshold must be in [0, 1]");
                }
                if !(0.0..=1.0).contains(&autotune.high_precision_threshold) {
                    anyhow::bail!("autotune.high_precision_threshold must be in [0, 1]");
                }
                if autotune.low_precision_threshold >= autotune.high_precision_threshold {
                    anyhow::bail!(
                        "autotune.low_precision_threshold must be < autotune.high_precision_threshold"
                    );
                }
                if autotune.min_vector_searches == 0 {
                    anyhow::bail!("autotune.min_vector_searches must be > 0 when enabled");
                }
                if autotune.summary_trigger_min == 0 {
                    anyhow::bail!("autotune.summary_trigger_min must be > 0 when enabled");
                }
                if autotune.summary_trigger_min > autotune.summary_trigger_max {
                    anyhow::bail!(
                        "autotune.summary_trigger_min must be <= autotune.summary_trigger_max"
                    );
                }
            }
        }

        Ok(())
    }
}

fn apply_auto_rules(config_path: &Path, config: &mut AppConfig) {
    let config_dir = config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    let mut shared_rules = Vec::new();
    for path in shared_rule_paths(config_dir) {
        append_unique(&mut shared_rules, load_optional_rule_items(&path));
    }

    for (phase_name, phase_cfg) in config.phases.iter_mut() {
        let mut merged = phase_cfg.principles.clone().unwrap_or_default();
        append_unique(&mut merged, shared_rules.clone());

        for path in phase_rule_paths(config_dir, phase_name) {
            append_unique(&mut merged, load_optional_rule_items(&path));
        }

        phase_cfg.principles = if merged.is_empty() {
            None
        } else {
            Some(merged)
        };
    }
}

fn shared_rule_paths(config_dir: &Path) -> Vec<std::path::PathBuf> {
    let rules_dir = config_dir.join("RULES");
    vec![
        config_dir.join("RULES.md"),
        rules_dir.join("global.md"),
        rules_dir.join("common.md"),
        rules_dir.join("local.md"),
    ]
}

fn phase_rule_paths(config_dir: &Path, phase_name: &str) -> Vec<std::path::PathBuf> {
    let rules_dir = config_dir.join("RULES");
    vec![
        config_dir.join(format!("{}.rules.md", phase_name)),
        rules_dir.join(format!("{}.md", phase_name)),
        rules_dir.join(format!("{}.rules.md", phase_name)),
        rules_dir.join(format!("{}.local.md", phase_name)),
    ]
}

fn load_optional_rule_items(path: &Path) -> Vec<String> {
    if !path.exists() {
        return Vec::new();
    }

    match fs::read_to_string(path) {
        Ok(content) => parse_rule_items(&content),
        Err(err) => {
            log::warn!(
                "failed to read optional rule file {}: {}",
                path.display(),
                err
            );
            Vec::new()
        }
    }
}

fn parse_rule_items(content: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut in_code_block = false;

    for raw_line in content.lines() {
        let trimmed = raw_line.trim();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let line = strip_rule_prefix(trimmed).trim();
        if !line.is_empty() {
            items.push(line.to_string());
        }
    }

    items
}

fn strip_rule_prefix(line: &str) -> &str {
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return rest;
        }
    }

    strip_ordered_prefix(line).unwrap_or(line)
}

fn strip_ordered_prefix(line: &str) -> Option<&str> {
    let mut idx = 0;
    for ch in line.chars() {
        if ch.is_ascii_digit() {
            idx += ch.len_utf8();
            continue;
        }
        break;
    }

    if idx == 0 || idx + 1 >= line.len() {
        return None;
    }

    let marker = line.as_bytes()[idx] as char;
    if (marker == '.' || marker == ')') && line.as_bytes()[idx + 1] == b' ' {
        return Some(&line[idx + 2..]);
    }

    None
}

fn append_unique(target: &mut Vec<String>, items: Vec<String>) {
    for item in items {
        if !target.iter().any(|existing| existing == &item) {
            target.push(item);
        }
    }
}

fn validate_phase_options(phase_name: &str, options: &PhaseOptions) -> Result<()> {
    if matches!(options.cache_ttl_seconds, Some(0)) {
        anyhow::bail!("phase '{}' cache_ttl_seconds must be > 0", phase_name);
    }
    if matches!(options.vector_min_query_chars, Some(0)) {
        anyhow::bail!("phase '{}' vector_min_query_chars must be > 0", phase_name);
    }
    if matches!(options.vector_top_k, Some(0)) {
        anyhow::bail!("phase '{}' vector_top_k must be > 0", phase_name);
    }
    if let Some(value) = options.vector_min_similarity {
        if !(0.0..=1.0).contains(&value) {
            anyhow::bail!(
                "phase '{}' vector_min_similarity must be in [0.0, 1.0]",
                phase_name
            );
        }
    }
    if matches!(options.vector_max_snippet_chars, Some(0)) {
        anyhow::bail!(
            "phase '{}' vector_max_snippet_chars must be > 0",
            phase_name
        );
    }
    if matches!(options.summary_trigger_messages, Some(0)) {
        anyhow::bail!(
            "phase '{}' summary_trigger_messages must be > 0",
            phase_name
        );
    }
    if matches!(options.summary_max_chars, Some(0)) {
        anyhow::bail!("phase '{}' summary_max_chars must be > 0", phase_name);
    }
    if matches!(options.max_history_messages, Some(0)) {
        anyhow::bail!("phase '{}' max_history_messages must be > 0", phase_name);
    }
    if matches!(options.max_history_chars, Some(0)) {
        anyhow::bail!("phase '{}' max_history_chars must be > 0", phase_name);
    }
    if matches!(options.request_timeout_seconds, Some(0)) {
        anyhow::bail!("phase '{}' request_timeout_seconds must be > 0", phase_name);
    }
    if matches!(options.review_timeout_seconds, Some(0)) {
        anyhow::bail!("phase '{}' review_timeout_seconds must be > 0", phase_name);
    }

    validate_extra_u64_range(phase_name, options, "max_request_chars", 1, 2_000_000)?;
    validate_extra_u64_range(phase_name, options, "rate_limit_rpm", 1, 10_000)?;
    validate_extra_u64_range(phase_name, options, "rate_limit_burst", 1, 50_000)?;
    validate_extra_f64_range(
        phase_name,
        options,
        "rate_limit_burst_multiplier",
        0.1,
        20.0,
    )?;
    validate_extra_u64_range(phase_name, options, "min_reviewers", 1, 16)?;
    validate_extra_u64_range(phase_name, options, "required_approvals", 1, 16)?;
    validate_extra_u64_range(phase_name, options, "phase_max_inflight", 1, 10_000)?;
    validate_extra_u64_range(phase_name, options, "global_max_inflight", 1, 10_000)?;
    validate_extra_u64_range(phase_name, options, "circuit_breaker_failures", 1, 100)?;
    validate_extra_u64_range(phase_name, options, "circuit_breaker_open_seconds", 1, 3600)?;
    validate_extra_u64_range(phase_name, options, "review_gate_timeout_seconds", 1, 3600)?;
    validate_extra_u64_range(phase_name, options, "review_min_response_chars", 1, 4000)?;

    if let Some(policy) = options
        .extra
        .get("review_timeout_policy")
        .and_then(|value| value.as_str())
    {
        if !policy.eq_ignore_ascii_case("reject") && !policy.eq_ignore_ascii_case("degrade_single")
        {
            anyhow::bail!(
                "phase '{}' option 'review_timeout_policy' must be one of: reject, degrade_single",
                phase_name
            );
        }
    }

    let min_reviewers = options
        .extra
        .get("min_reviewers")
        .and_then(|value| value.as_u64());
    let required_approvals = options
        .extra
        .get("required_approvals")
        .and_then(|value| value.as_u64());
    if let (Some(min_reviewers), Some(required_approvals)) = (min_reviewers, required_approvals) {
        if required_approvals > min_reviewers {
            anyhow::bail!(
                "phase '{}' required_approvals must be <= min_reviewers",
                phase_name
            );
        }
    }

    Ok(())
}

fn validate_extra_u64_range(
    phase_name: &str,
    options: &PhaseOptions,
    key: &str,
    min: u64,
    max: u64,
) -> Result<()> {
    let Some(value) = options.extra.get(key) else {
        return Ok(());
    };

    let Some(num) = value.as_u64() else {
        anyhow::bail!(
            "phase '{}' option '{}' must be a positive integer",
            phase_name,
            key
        );
    };

    if num < min || num > max {
        anyhow::bail!(
            "phase '{}' option '{}' must be in [{}, {}]",
            phase_name,
            key,
            min,
            max
        );
    }

    Ok(())
}

fn validate_extra_f64_range(
    phase_name: &str,
    options: &PhaseOptions,
    key: &str,
    min: f64,
    max: f64,
) -> Result<()> {
    let Some(value) = options.extra.get(key) else {
        return Ok(());
    };

    let Some(num) = value.as_f64() else {
        anyhow::bail!("phase '{}' option '{}' must be a number", phase_name, key);
    };

    if num < min || num > max {
        anyhow::bail!(
            "phase '{}' option '{}' must be in [{}, {}]",
            phase_name,
            key,
            min,
            max
        );
    }

    Ok(())
}

fn phase_uses_complex_autopilot(options: Option<&PhaseOptions>) -> bool {
    options
        .and_then(|opts| opts.autopilot_complexity.as_deref())
        .map(|value| value.eq_ignore_ascii_case("complex"))
        .unwrap_or(false)
}

pub fn missing_env_vars(config: &AppConfig) -> Vec<String> {
    let mut missing = Vec::new();

    for agent in config.agents.values() {
        for env_name in required_env_vars(agent) {
            match std::env::var(env_name) {
                Ok(value) if !value.trim().is_empty() => {}
                _ => missing.push(env_name.to_string()),
            }
        }
    }

    missing.sort();
    missing.dedup();
    missing
}

fn required_env_vars(agent: &AgentConfig) -> Vec<&str> {
    let mut envs = Vec::new();
    if let Some(value) = agent.api_key_env.as_deref() {
        if !is_keyring_ref(value) {
            envs.push(value);
        }
    }
    if let Some(value) = agent.secret_key_env.as_deref() {
        if !is_keyring_ref(value) {
            envs.push(value);
        }
    }
    envs
}

fn is_keyring_ref(value: &str) -> bool {
    value.starts_with("keyring://")
}

pub fn validate_external_secret_refs(config: &AppConfig) -> Result<()> {
    for (agent_name, agent) in &config.agents {
        if let Some(value) = agent.api_key_env.as_deref() {
            validate_secret_ref(value, &format!("agents.{}.api_key_env", agent_name))?;
        }
        if let Some(value) = agent.secret_key_env.as_deref() {
            validate_secret_ref(value, &format!("agents.{}.secret_key_env", agent_name))?;
        }
    }
    Ok(())
}

pub fn validate_runtime_readiness(
    config_path: &Path,
    config: &AppConfig,
) -> Result<ConfigHealthReport> {
    config.validate()?;

    let missing = missing_env_vars(config);
    if !missing.is_empty() {
        anyhow::bail!(
            "missing required environment variables for configured agents: {}",
            missing.join(", ")
        );
    }

    validate_external_secret_refs(config)?;
    Ok(build_config_health_report(config_path, config))
}

#[allow(dead_code)]
pub fn collect_config_warnings(config_path: &Path, config: &AppConfig) -> Vec<String> {
    collect_config_warnings_detailed(config_path, config)
        .into_iter()
        .map(|item| item.message)
        .collect()
}

pub fn build_config_health_report(config_path: &Path, config: &AppConfig) -> ConfigHealthReport {
    let mut warnings = collect_config_warnings_detailed(config_path, config);
    warnings.sort_by(|left, right| {
        severity_rank(left.severity)
            .cmp(&severity_rank(right.severity))
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| left.message.cmp(&right.message))
    });

    let info_count = warnings
        .iter()
        .filter(|item| item.severity == ConfigWarningSeverity::Info)
        .count();
    let warn_count = warnings
        .iter()
        .filter(|item| item.severity == ConfigWarningSeverity::Warn)
        .count();
    let critical_count = warnings
        .iter()
        .filter(|item| item.severity == ConfigWarningSeverity::Critical)
        .count();
    let penalty = (info_count * 5) + (warn_count * 15) + (critical_count * 40);
    let score = 100_u32.saturating_sub(penalty.min(100) as u32);

    ConfigHealthReport {
        score,
        total: warnings.len(),
        info_count,
        warn_count,
        critical_count,
        warnings,
    }
}

fn collect_config_warnings_detailed(config_path: &Path, config: &AppConfig) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();

    if let Some(vector) = &config.vector {
        if vector.enabled && !vector.summary_enabled {
            warnings.push(ConfigWarning {
                code: "VECTOR_SUMMARY_DISABLED".to_string(),
                severity: ConfigWarningSeverity::Info,
                message: "vector memory is enabled while summary_enabled=false; retrieval quality and long-session compression may degrade".to_string(),
            });
        }

        if vector.enabled && vector.max_entries >= 50_000 {
            warnings.push(ConfigWarning {
                code: "VECTOR_MAX_ENTRIES_HIGH".to_string(),
                severity: ConfigWarningSeverity::Warn,
                message: format!(
                    "vector.max_entries={} is unusually high; startup I/O, SQLite growth, and maintenance time may increase",
                    vector.max_entries
                ),
            });
        }
    }

    if let Some(cache) = &config.cache {
        if cache.enabled && cache.max_entries >= 50_000 {
            warnings.push(ConfigWarning {
                code: "CACHE_MAX_ENTRIES_HIGH".to_string(),
                severity: ConfigWarningSeverity::Warn,
                message: format!(
                    "cache.max_entries={} is unusually high; consider lowering it if startup or VACUUM pauses increase",
                    cache.max_entries
                ),
            });
        }
    }

    if let Some(autotune) = &config.autotune {
        let vector_enabled = config.vector.as_ref().map(|v| v.enabled).unwrap_or(false);
        if autotune.enabled && !vector_enabled {
            warnings.push(ConfigWarning {
                code: "AUTOTUNE_WITHOUT_VECTOR".to_string(),
                severity: ConfigWarningSeverity::Warn,
                message: "autotune is enabled but vector memory is disabled; autotune will have little practical effect".to_string(),
            });
        }
    }

    if let Some(runtime) = &config.runtime {
        if runtime.otel_enabled && runtime.otel_endpoint.is_none() {
            warnings.push(ConfigWarning {
                code: "OTEL_ENDPOINT_DEFAULTED".to_string(),
                severity: ConfigWarningSeverity::Info,
                message: "runtime.otel_enabled=true without otel_endpoint; default collector endpoint http://127.0.0.1:4317 will be used".to_string(),
            });
        }
    }

    for path in shared_rule_paths(config_path.parent().unwrap_or_else(|| Path::new("."))) {
        push_rule_warning(&mut warnings, &path, "RULE_FILE_EMPTY");
    }
    for phase_name in config.phases.keys() {
        for path in phase_rule_paths(
            config_path.parent().unwrap_or_else(|| Path::new(".")),
            phase_name,
        ) {
            push_rule_warning(&mut warnings, &path, "RULE_FILE_EMPTY");
        }
    }

    for (phase_name, phase_cfg) in &config.phases {
        let uses_complex = phase_uses_complex_autopilot(phase_cfg.options.as_ref());
        if !uses_complex {
            continue;
        }

        let review_options = config
            .phases
            .get("review")
            .and_then(|phase| phase.options.as_ref());
        let gate_timeout = phase_cfg
            .options
            .as_ref()
            .and_then(|opts| opts.extra.get("review_gate_timeout_seconds"))
            .and_then(|value| value.as_u64())
            .or_else(|| {
                review_options.and_then(|opts| {
                    opts.extra
                        .get("review_gate_timeout_seconds")
                        .and_then(|value| value.as_u64())
                })
            });
        let reviewer_timeout = review_options
            .and_then(|opts| opts.review_timeout_seconds.or(opts.request_timeout_seconds))
            .or_else(|| {
                phase_cfg
                    .options
                    .as_ref()
                    .and_then(|opts| opts.review_timeout_seconds.or(opts.request_timeout_seconds))
            });

        if gate_timeout.is_none() && reviewer_timeout.is_none() {
            warnings.push(ConfigWarning {
                code: "REVIEW_GATE_TIMEOUT_MISSING".to_string(),
                severity: ConfigWarningSeverity::Critical,
                message: format!(
                    "phase '{}' uses complex autopilot without review_gate_timeout_seconds or review_timeout_seconds/request_timeout_seconds; review gate may hang too long",
                    phase_name
                ),
            });
        }

        let review_phase_limit = review_options
            .and_then(|opts| opts.extra.get("phase_max_inflight"))
            .and_then(|value| value.as_u64());
        let review_global_limit = review_options
            .and_then(|opts| opts.extra.get("global_max_inflight"))
            .and_then(|value| value.as_u64());
        if review_phase_limit.is_none() || review_global_limit.is_none() {
            warnings.push(ConfigWarning {
                code: "REVIEW_INFLIGHT_LIMIT_MISSING".to_string(),
                severity: ConfigWarningSeverity::Warn,
                message:
                    "review phase is missing phase_max_inflight or global_max_inflight; high concurrency can degrade review stability"
                        .to_string(),
            });
        }
    }

    warnings.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.message.cmp(&right.message))
    });
    warnings.dedup_by(|left, right| left.code == right.code && left.message == right.message);
    warnings
}

fn severity_rank(value: ConfigWarningSeverity) -> usize {
    match value {
        ConfigWarningSeverity::Critical => 0,
        ConfigWarningSeverity::Warn => 1,
        ConfigWarningSeverity::Info => 2,
    }
}

fn push_rule_warning(warnings: &mut Vec<ConfigWarning>, path: &Path, code: &str) {
    if path.exists() && load_optional_rule_items(path).is_empty() {
        warnings.push(ConfigWarning {
            code: code.to_string(),
            severity: ConfigWarningSeverity::Info,
            message: format!(
                "rule file '{}' exists but contributed no usable rule lines",
                path.display()
            ),
        });
    }
}

fn validate_secret_ref(value: &str, field_name: &str) -> Result<()> {
    if !is_keyring_ref(value) {
        return Ok(());
    }

    let locator = value
        .strip_prefix("keyring://")
        .ok_or_else(|| anyhow::anyhow!("invalid keyring ref for {}", field_name))?;
    let (service, account) = locator.split_once('/').ok_or_else(|| {
        anyhow::anyhow!(
            "invalid {} keyring reference '{}': expected keyring://<service>/<account>",
            field_name,
            value
        )
    })?;
    let entry = keyring::Entry::new(service, account).map_err(|err| {
        anyhow::anyhow!("failed to open keyring entry for {}: {}", field_name, err)
    })?;
    let secret = entry.get_password().map_err(|err| {
        anyhow::anyhow!("failed to read keyring entry for {}: {}", field_name, err)
    })?;
    if secret.trim().is_empty() {
        anyhow::bail!("keyring entry for {} resolved to empty value", field_name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{AgentConfig, AppConfig, FlowConfig, PhaseConfig, PhaseOptions, RuntimeConfig};

    fn base_agent() -> AgentConfig {
        AgentConfig {
            agent_type: "copilot".to_string(),
            url: Some("http://127.0.0.1:8080".to_string()),
            chat_path: None,
            api_key_env: None,
            secret_key_env: None,
            anthropic_version: None,
            model: None,
            max_tokens: None,
            supports_system: None,
        }
    }

    fn valid_config() -> AppConfig {
        let mut agents = HashMap::new();
        agents.insert("copilot".to_string(), base_agent());
        agents.insert(
            "reviewer_a".to_string(),
            AgentConfig {
                agent_type: "claude".to_string(),
                url: Some("https://api.anthropic.com".to_string()),
                chat_path: None,
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                secret_key_env: None,
                anthropic_version: Some("2023-06-01".to_string()),
                model: Some("claude-3-7-sonnet-latest".to_string()),
                max_tokens: Some(4096),
                supports_system: None,
            },
        );
        agents.insert(
            "reviewer_b".to_string(),
            AgentConfig {
                agent_type: "wenxin".to_string(),
                url: None,
                chat_path: None,
                api_key_env: Some("WENXIN_API_KEY".to_string()),
                secret_key_env: Some("WENXIN_SECRET_KEY".to_string()),
                anthropic_version: None,
                model: None,
                max_tokens: None,
                supports_system: None,
            },
        );

        let mut phases = HashMap::new();
        phases.insert(
            "coding".to_string(),
            PhaseConfig {
                description: "coding".to_string(),
                agents: vec!["copilot".to_string()],
                fallback: Some(true),
                principles: None,
                options: None,
            },
        );
        phases.insert(
            "review".to_string(),
            PhaseConfig {
                description: "review".to_string(),
                agents: vec!["reviewer_a".to_string(), "reviewer_b".to_string()],
                fallback: Some(true),
                principles: None,
                options: None,
            },
        );

        AppConfig {
            default_phase: "coding".to_string(),
            agents,
            flow: FlowConfig {
                name: "flow".to_string(),
                phases: vec!["coding".to_string(), "review".to_string()],
            },
            phases,
            runtime: Some(RuntimeConfig::default()),
            cache: None,
            vector: None,
            autotune: None,
            model_selection_mode: "adaptive".to_string(),
        }
    }

    #[test]
    fn validate_accepts_valid_configuration() {
        let cfg = valid_config();
        cfg.validate().expect("valid config should pass");
    }

    #[test]
    fn validate_rejects_default_phase_not_in_flow() {
        let mut cfg = valid_config();
        cfg.default_phase = "missing".to_string();
        let err = cfg
            .validate()
            .expect_err("default phase outside flow must fail");
        assert!(
            err.to_string()
                .contains("default_phase 'missing' is not listed in flow.phases"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_phase_with_unknown_agent() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .agents = vec!["missing".to_string()];

        let err = cfg
            .validate()
            .expect_err("phase referencing undefined agent must fail");
        assert!(
            err.to_string()
                .contains("phase 'coding' references undefined agent 'missing'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_autotune_threshold_order() {
        let mut cfg = valid_config();
        cfg.autotune = Some(super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.8,
            high_precision_threshold: 0.5,
            state_path: "state.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        });

        let err = cfg
            .validate()
            .expect_err("invalid autotune threshold order must fail");
        assert!(
            err.to_string().contains(
                "autotune.low_precision_threshold must be < autotune.high_precision_threshold"
            ),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_zero_runtime_maintenance_interval() {
        let mut cfg = valid_config();
        cfg.runtime = Some(RuntimeConfig {
            maintenance_interval_seconds: 0,
            health_interval_seconds: 30,
            shutdown_drain_seconds: 10,
            sqlite_vacuum_interval_cycles: 60,
            otel_enabled: false,
            otel_exporter: "otlp".to_string(),
            otel_endpoint: None,
            otel_service_name: "go-on".to_string(),
            otel_sample_ratio: 1.0,
            trace_slow_top_n: 20,
        });

        let err = cfg
            .validate()
            .expect_err("zero maintenance interval must fail");
        assert!(
            err.to_string()
                .contains("runtime.maintenance_interval_seconds must be > 0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_autotune_summary_range() {
        let mut cfg = valid_config();
        cfg.autotune = Some(super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "state.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 9,
            summary_trigger_max: 6,
        });

        let err = cfg
            .validate()
            .expect_err("invalid autotune summary range must fail");
        assert!(
            err.to_string()
                .contains("autotune.summary_trigger_min must be <= autotune.summary_trigger_max"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_complex_autopilot_without_two_reviewers() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            autopilot_complexity: Some("complex".to_string()),
            full_auto_review_agents: Some(vec!["reviewer_a".to_string()]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("complex autopilot with one reviewer must fail");
        assert!(
            err.to_string()
                .contains("must define at least 2 full_auto_review_agents"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_complex_autopilot_when_reviewer_not_in_review_phase() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("review")
            .expect("review phase must exist")
            .agents = vec!["reviewer_a".to_string(), "copilot".to_string()];
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            autopilot_complexity: Some("complex".to_string()),
            full_auto_review_agents: Some(vec!["reviewer_a".to_string(), "reviewer_b".to_string()]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("missing reviewer in review phase must fail");
        assert!(
            err.to_string()
                .contains("review agent 'reviewer_b' must also appear in [phases.review].agents"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_zero_phase_timeout() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            request_timeout_seconds: Some(0),
            ..PhaseOptions::default()
        });

        let err = cfg.validate().expect_err("zero request timeout must fail");
        assert!(
            err.to_string()
                .contains("phase 'coding' request_timeout_seconds must be > 0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_required_approvals_exceeding_min_reviewers() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([
                ("min_reviewers".to_string(), serde_json::Value::from(2_u64)),
                (
                    "required_approvals".to_string(),
                    serde_json::Value::from(3_u64),
                ),
            ]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("required approvals above min reviewers must fail");
        assert!(
            err.to_string()
                .contains("phase 'coding' required_approvals must be <= min_reviewers"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_invalid_rate_limit_type() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "rate_limit_rpm".to_string(),
                serde_json::Value::from("fast"),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("non-numeric rate_limit_rpm must fail");
        assert!(
            err.to_string()
                .contains("phase 'coding' option 'rate_limit_rpm' must be a positive integer"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_invalid_burst_multiplier_range() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "rate_limit_burst_multiplier".to_string(),
                serde_json::Value::from(100.0_f64),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("burst multiplier out of range must fail");
        assert!(
            err.to_string().contains(
                "phase 'coding' option 'rate_limit_burst_multiplier' must be in [0.1, 20]"
            ),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_zero_breaker_open_seconds() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "circuit_breaker_open_seconds".to_string(),
                serde_json::Value::from(0_u64),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("zero breaker open seconds must fail");
        assert!(
            err.to_string().contains(
                "phase 'coding' option 'circuit_breaker_open_seconds' must be in [1, 3600]"
            ),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_invalid_review_timeout_policy() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "review_timeout_policy".to_string(),
                serde_json::Value::from("maybe"),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("invalid review timeout policy must fail");
        assert!(
            err.to_string().contains(
                "phase 'coding' option 'review_timeout_policy' must be one of: reject, degrade_single"
            ),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn missing_env_vars_detects_agent_requirements() {
        let cfg = valid_config();
        let missing = super::missing_env_vars(&cfg);

        assert!(missing.iter().any(|value| value == "ANTHROPIC_API_KEY"));
        assert!(missing.iter().any(|value| value == "WENXIN_API_KEY"));
        assert!(missing.iter().any(|value| value == "WENXIN_SECRET_KEY"));
    }

    #[test]
    fn config_example_loads_and_validates() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.toml.example");
        let cfg = AppConfig::load(&path).expect("config.toml.example should parse");

        cfg.validate()
            .expect("config.toml.example should be internally consistent");
    }

    #[test]
    fn load_auto_rules_from_rules_directory_and_phase_files() {
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        let rules_dir = dir.path().join("RULES");
        fs::create_dir_all(&rules_dir).expect("rules directory should be created");

        fs::write(
            &config_path,
            r#"default_phase = "coding"

[flow]
name = "test"
phases = ["coding", "review"]

[agents.copilot]
type = "copilot"
url = "http://127.0.0.1:8080"

[phases.coding]
description = "coding"
agents = ["copilot"]
fallback = true
principles = ["inline principle"]

[phases.review]
description = "review"
agents = ["copilot"]
fallback = true
"#,
        )
        .expect("config should be written");

        fs::write(
            dir.path().join("RULES.md"),
            "# Shared\n- shared one\n- shared two\n",
        )
        .expect("shared rules should be written");
        fs::write(
            rules_dir.join("coding.md"),
            "## Coding\n1. coding phase rule\n* extra coding rule\n",
        )
        .expect("phase rules should be written");

        let cfg = AppConfig::load(&config_path).expect("config should load");
        let coding = cfg
            .phases
            .get("coding")
            .and_then(|phase| phase.principles.as_ref())
            .expect("coding principles should exist");
        assert!(coding.iter().any(|v| v == "inline principle"));
        assert!(coding.iter().any(|v| v == "shared one"));
        assert!(coding.iter().any(|v| v == "shared two"));
        assert!(coding.iter().any(|v| v == "coding phase rule"));
        assert!(coding.iter().any(|v| v == "extra coding rule"));

        let review = cfg
            .phases
            .get("review")
            .and_then(|phase| phase.principles.as_ref())
            .expect("review principles should exist");
        assert!(review.iter().any(|v| v == "shared one"));
        assert!(review.iter().any(|v| v == "shared two"));
    }

    #[test]
    fn load_auto_rules_from_sidecar_phase_file() {
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");

        fs::write(
            &config_path,
            r#"default_phase = "coding"

[flow]
name = "test"
phases = ["coding"]

[agents.copilot]
type = "copilot"
url = "http://127.0.0.1:8080"

[phases.coding]
description = "coding"
agents = ["copilot"]
fallback = true
"#,
        )
        .expect("config should be written");

        fs::write(
            dir.path().join("coding.rules.md"),
            "- keep functions short\n- add tests\n",
        )
        .expect("sidecar rules should be written");

        let cfg = AppConfig::load(&config_path).expect("config should load");
        let coding = cfg
            .phases
            .get("coding")
            .and_then(|phase| phase.principles.as_ref())
            .expect("coding principles should exist");

        assert!(coding.iter().any(|v| v == "keep functions short"));
        assert!(coding.iter().any(|v| v == "add tests"));
    }

    #[test]
    fn autotune_state_initializes_with_config_defaults() {
        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let state = super::AutoTuneState::new(&config);
        assert_eq!(state.current_min_query_chars, 40);
        assert_eq!(state.current_top_k, 2);
        assert_eq!(state.window_phase, 0);
        assert_eq!(state.vector_search_count, 0);
    }

    #[test]
    fn autotune_state_records_vector_search_metrics() {
        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = super::AutoTuneState::new(&config);
        state.record_vector_search(0.9, &config); // high precision
        state.record_vector_search(0.3, &config); // low precision
        state.record_vector_search(0.5, &config); // medium (no increment)

        assert_eq!(state.vector_search_count, 3);
        assert_eq!(state.high_precision_count, 1);
        assert_eq!(state.low_precision_count, 1);
    }

    #[test]
    fn autotune_state_adjusts_on_high_precision() {
        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = super::AutoTuneState::new(&config);
        // Record 20 searches: 15 high precision (75%)
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }

        let adjusted = state.evaluate_and_adjust(&config);
        assert!(adjusted, "should adjust when precision is high");
        assert_eq!(
            state.current_min_query_chars, 60,
            "should increase min_query_chars"
        );
        assert_eq!(state.vector_search_count, 0, "should reset counters");
        assert_eq!(state.window_phase, 1);
    }

    #[test]
    fn autotune_state_adjusts_on_low_precision() {
        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = super::AutoTuneState::new(&config);
        state.current_min_query_chars = 100; // start higher
                                             // Record 20 searches: 10 low precision (50%)
        for _ in 0..10 {
            state.record_vector_search(0.2, &config);
        }
        for _ in 0..10 {
            state.record_vector_search(0.5, &config);
        }

        let adjusted = state.evaluate_and_adjust(&config);
        assert!(adjusted, "should adjust when precision is low");
        assert_eq!(
            state.current_min_query_chars, 80,
            "should decrease min_query_chars"
        );
    }

    #[test]
    fn autotune_state_respects_cooldown() {
        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = super::AutoTuneState::new(&config);
        // Fill evaluation window with high precision
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }

        // First adjustment should succeed
        let adjusted1 = state.evaluate_and_adjust(&config);
        assert!(adjusted1);
        assert_eq!(state.cooldown_remaining, 2);
        let min_query_chars_after_first = state.current_min_query_chars;

        // Fill next evaluation window
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }

        // Second adjustment attempt should fail due to cooldown
        let adjusted2 = state.evaluate_and_adjust(&config);
        assert!(!adjusted2, "should not adjust during cooldown");
        assert_eq!(
            state.current_min_query_chars, min_query_chars_after_first,
            "parameters should not change"
        );

        // Tick cooldown and try again
        state.tick_cooldown();
        state.tick_cooldown();
        state.tick_cooldown(); // Extra to fully clear

        // Now should be able to adjust again (cooldown expired and new window filled)
        for _ in 0..15 {
            state.record_vector_search(0.9, &config);
        }
        for _ in 0..5 {
            state.record_vector_search(0.5, &config);
        }
        let adjusted3 = state.evaluate_and_adjust(&config);
        assert!(adjusted3, "should adjust after cooldown expires");
    }

    #[test]
    fn autotune_cooldown_advances_across_windows() {
        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 4,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 2,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = super::AutoTuneState::new(&config);
        state.cooldown_remaining = 2;
        state.vector_search_count = 4;
        state.high_precision_count = 3;
        state.low_precision_count = 1;

        let advanced = state.advance_cooldown_window(&config);
        assert!(
            advanced,
            "cooldown window should advance once interval is reached"
        );
        assert_eq!(state.cooldown_remaining, 1);
        assert_eq!(state.vector_search_count, 0);
        assert_eq!(state.high_precision_count, 0);
        assert_eq!(state.low_precision_count, 0);
        assert_eq!(state.window_phase, 1);
    }

    #[test]
    fn autotune_state_load_and_save_roundtrip() {
        use tempfile::NamedTempFile;

        let config = super::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let temp_file = NamedTempFile::new().expect("failed to create temp file");
        let path = temp_file
            .path()
            .to_str()
            .expect("failed to get path")
            .to_string();

        // Create, modify, and save state
        let mut state = super::AutoTuneState::new(&config);
        state.current_min_query_chars = 120;
        state.current_top_k = 3;
        state.window_phase = 5;
        state.vector_search_count = 10;
        state.high_precision_count = 8;
        state.low_precision_count = 1;

        state.save(&path).expect("failed to save state");

        // Load and verify
        let loaded = super::AutoTuneState::load_or_default(&path, &config);
        assert_eq!(loaded.current_min_query_chars, 120);
        assert_eq!(loaded.current_top_k, 3);
        assert_eq!(loaded.window_phase, 5);
        assert_eq!(loaded.vector_search_count, 10);
        assert_eq!(loaded.high_precision_count, 8);
        assert_eq!(loaded.low_precision_count, 1);
    }
}
