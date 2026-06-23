use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::autotune::AutoTuneConfig;
use crate::shared::role_types::RoleDefinition;

/// Default schema version string for deserialization fallback.
fn default_schema_version() -> String {
    "1.0.0".to_string()
}

// ---------------------------------------------------------------------------
// Sub-configs (A7: split AppConfig into logical sections)
// ---------------------------------------------------------------------------

/// Provider-related configuration fields.
///
/// Flattened into [`AppConfig`] so existing config files with
/// `agents.*`, `default_phase`, and `role_registry.*` keys continue
/// to deserialize without nesting changes.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProviderConfig {
    /// Default phase to use when none is specified
    #[serde(default)]
    pub default_phase: String,
    /// Map of agent configurations
    #[serde(default)]
    pub agents: HashMap<String, AgentConfig>,
    /// Custom role registry loaded from `[role_registry.*]`
    #[serde(default)]
    pub role_registry: HashMap<String, RoleDefinition>,
}

/// Security / governance configuration fields.
///
/// Extracted from the original monolithic `RuntimeConfig` so that
/// security-sensitive settings are grouped and independently
/// documented.  Flattened into [`AppConfig`] for config-file
/// backward compatibility.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SecurityConfig {
    /// Whether inbound entry auth is enabled at gateway/edge for exposed HTTP endpoints
    #[serde(default)]
    pub entry_auth_enabled: bool,
    /// Env var name holding entry API key used for HTTP ingress auth
    #[serde(default = "super::defaults::default_runtime_entry_auth_api_key_env")]
    pub entry_auth_api_key_env: String,
    /// Entry layer source-based rate limit (requests per minute)
    #[serde(default = "super::defaults::default_runtime_entry_rate_limit_rpm")]
    pub entry_rate_limit_rpm: u64,
    /// Entry layer token bucket burst capacity per source
    #[serde(default = "super::defaults::default_runtime_entry_rate_limit_burst")]
    pub entry_rate_limit_burst: u64,
    /// Master switch for user-level authentication.
    /// When `false`, all requests are treated as admin (single-user mode).
    #[serde(default)]
    pub user_auth_enabled: bool,
    /// HMAC secret for signing user authentication tokens.
    #[serde(default = "super::defaults::default_runtime_user_auth_token_secret")]
    pub user_auth_token_secret: String,
    /// Env var name holding the HMAC secret for user auth tokens.
    #[serde(default = "super::defaults::default_runtime_user_auth_token_secret_env")]
    pub user_auth_token_secret_env: String,
    /// Token TTL in seconds for user authentication tokens (default: 86400 = 24h).
    #[serde(default = "super::defaults::default_runtime_user_auth_token_ttl_seconds")]
    pub user_auth_token_ttl_seconds: u64,
    /// Enable request signature verification for incoming JSON-RPC requests.
    #[serde(default)]
    pub request_signing_enabled: bool,
    /// Base64-encoded Ed25519 public key (32 bytes) for request signature verification.
    #[serde(default)]
    pub request_signing_public_key: String,
    /// HMAC shared secret for request signature verification (plaintext).
    #[serde(default)]
    pub request_signing_hmac_secret: String,
    /// Enable mTLS for the ACP HTTP listener.
    #[serde(default)]
    pub mtls_enabled: bool,
    /// Path to the CA certificate file for mTLS.
    #[serde(default)]
    pub mtls_ca_cert_path: String,
    /// Path to the server certificate file for mTLS.
    #[serde(default)]
    pub mtls_server_cert_path: String,
    /// Path to the server private key file for mTLS.
    #[serde(default)]
    pub mtls_server_key_path: String,
    /// Whether to require client certificates in mTLS handshake.
    #[serde(default)]
    pub mtls_require_client_cert: bool,
    /// Comma-separated list of allowed client certificate CNs.
    /// Empty means any valid client cert is accepted.
    #[serde(default)]
    pub mtls_allowed_cns: String,
}

/// Feature-flag configuration fields.
///
/// Extracted from the original monolithic `RuntimeConfig` and
/// `AppConfig` so that feature flags are grouped in one place.
/// Flattened into [`AppConfig`] for config-file backward compatibility.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct FeatureConfig {
    /// Enable governance subsystem (policy enforcement, RBAC, budget, etc.).
    #[serde(default = "super::defaults::default_true")]
    pub governance_enabled: bool,
    /// Governance policy mode: "active" (enforce), "audit" (log-only), "disabled".
    #[serde(default)]
    pub governance_policy_mode: String,
    /// Enable builtin skills at server startup.
    #[serde(default = "super::defaults::default_runtime_skills_enabled")]
    pub skills_enabled: bool,
    /// Enable skills import APIs.
    #[serde(default)]
    pub skills_import_enabled: bool,
    /// Allowed source prefixes for importing skills.
    #[serde(default)]
    pub skills_allowed_sources: Vec<String>,
    /// Require import requests to provide expected SHA256 digest.
    #[serde(default = "super::defaults::default_runtime_skills_require_sha256")]
    pub skills_require_sha256: bool,
    /// Allow floating refs when importing from GitHub.
    #[serde(default)]
    pub skills_allow_floating_ref: bool,
    /// Cache directory for skill manifests and index.
    #[serde(default = "super::defaults::default_runtime_skills_cache_dir")]
    pub skills_cache_dir: String,
    /// Model selection mode for automatic selection (Phase 10+)
    #[serde(default)]
    pub model_selection_mode: String,
    /// Enable DAG-driven tool execution in autonomy loop.
    #[serde(default)]
    pub enable_dag_execution: bool,
    /// Enable adaptive agent reroute on weak rounds.
    #[serde(default = "super::defaults::default_true")]
    pub enable_agent_reroute: bool,
    /// Enable metacognitive + world-model feedback hooks.
    #[serde(default = "super::defaults::default_true")]
    pub enable_metacognitive_feedback: bool,
    /// Enable Delphi-method debate voting in rationalize_decision.
    #[serde(default)]
    pub enable_delphi_debate: bool,
}

/// Application configuration structure
///
/// The provider, security, and feature sub-configs are `#[serde(flatten)]`ed
/// so that existing TOML/JSON config files with top-level keys like
/// `agents.*`, `entry_auth_enabled`, `governance_enabled`, etc. continue
/// to deserialize without nesting changes (A7 backward-compat requirement).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct AppConfig {
    /// Config schema version for migration tracking
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    /// Provider-related configuration (flattened)
    #[serde(flatten)]
    pub provider: ProviderConfig,
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
    /// Security / governance configuration (flattened)
    #[serde(flatten)]
    pub security: SecurityConfig,
    /// Feature-flag configuration (flattened)
    #[serde(flatten)]
    pub feature: FeatureConfig,
    /// Compliance configuration (S3)
    #[serde(default)]
    pub compliance: Option<ComplianceConfig>,
    /// Startup context loader configuration (S5)
    #[serde(default)]
    pub startup_context: Option<StartupContextConfig>,
    /// Scheduler configuration (S8/S9)
    #[serde(default)]
    pub scheduler: Option<SchedulerConfig>,
    /// Reputation tracking configuration (S13)
    #[serde(default)]
    pub reputation: Option<ReputationConfig>,
}

/// Simplified adaptive configuration for AI-driven setup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveConfig {
    /// Whether to use adaptive mode (AI determines best configuration)
    #[serde(default = "super::defaults::default_true")]
    pub adaptive_mode: bool,

    /// Minimum configuration required for operation
    pub minimal_config: MinimalConfig,

    /// Learning preferences for AI adaptation
    #[serde(default)]
    pub learning_preferences: LearningPreferences,

    /// Conversation history for context-aware adaptation
    #[serde(default)]
    pub conversation_context: Vec<ConversationContext>,
}

/// Minimal configuration required for operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinimalConfig {
    /// Default phase name
    #[serde(default = "super::defaults::default_coding_phase")]
    pub default_phase: String,

    /// Available AI providers (auto-detected from environment)
    #[serde(default)]
    pub available_providers: Vec<String>,

    /// Whether to enable caching
    #[serde(default = "super::defaults::default_true")]
    pub enable_cache: bool,

    /// Whether to enable vector memory
    #[serde(default = "super::defaults::default_true")]
    pub enable_vector_memory: bool,
}

/// Learning preferences for AI adaptation
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LearningPreferences {
    /// Preferred communication style
    #[serde(default = "super::defaults::default_communication_style")]
    pub communication_style: String,

    /// Preferred level of detail
    #[serde(default = "super::defaults::default_detail_level")]
    pub detail_level: String,

    /// Learning speed preference
    #[serde(default = "super::defaults::default_learning_speed")]
    pub learning_speed: String,

    /// Whether to ask for clarification when uncertain
    #[serde(default = "super::defaults::default_true")]
    pub ask_for_clarification: bool,

    /// Whether to adapt based on conversation history
    #[serde(default = "super::defaults::default_true")]
    pub adapt_from_history: bool,
}

/// Conversation context for adaptive configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationContext {
    /// Conversation ID
    pub conversation_id: String,
    /// User preferences expressed in conversation
    pub expressed_preferences: Vec<String>,
    /// Successful adaptations from this conversation
    pub successful_adaptations: Vec<String>,
    /// Timestamp of last interaction
    pub last_interaction: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub agent_type: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub chat_path: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub secret_key_env: Option<String>,
    #[serde(default)]
    pub anthropic_version: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub supports_system: Option<bool>,
    #[serde(default)]
    pub supports_vision: Option<bool>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub recommended_default_phase: Option<String>,
    #[serde(default)]
    pub recommended_request_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub recommended_review_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub recommended_cache_enabled: Option<bool>,
    #[serde(default)]
    pub recommended_vector_enabled: Option<bool>,
    #[serde(default)]
    pub recommended_phase_max_inflight: Option<usize>,
    #[serde(default)]
    pub recommended_global_max_inflight: Option<usize>,
    #[serde(default)]
    pub recommended_planning_request_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub recommended_coding_request_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub recommended_review_request_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub recommended_delivery_request_timeout_seconds: Option<u64>,
}

// F-GAP-64: Add builder pattern for ProviderSpec (31 fields) to simplify
// construction with sensible defaults and avoid repetitive `Option` wrapping.

impl ProviderSpec {
    /// Returns whether this provider supports vision/image inputs.
    pub fn supports_vision(&self) -> bool {
        self.supports_vision.unwrap_or(false)
    }
}

/// Runtime configuration
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    /// Protocol mode: auto / acp / mcp
    #[serde(default)]
    pub protocol_mode: Option<String>,
    /// Platform mode: universal / phase_compat
    #[serde(default)]
    pub platform_mode: Option<String>,
    /// Emit PUA execution report into JSON-RPC response metadata when enabled
    #[serde(default)]
    pub pua_report: bool,
    /// Deployment target for hardening profile selection: local-dev | ci | managed-service
    #[serde(default)]
    pub deployment_target: Option<String>,
    /// Maintenance interval in seconds
    #[serde(default = "super::defaults::default_runtime_maintenance_interval_seconds")]
    pub maintenance_interval_seconds: u64,
    /// Health check interval in seconds
    #[serde(default = "super::defaults::default_runtime_health_interval_seconds")]
    pub health_interval_seconds: u64,
    /// Shutdown drain time in seconds
    #[serde(default = "super::defaults::default_runtime_shutdown_drain_seconds")]
    pub shutdown_drain_seconds: u64,
    /// Optional ACP HTTP bind address for REST/SSE endpoints
    #[serde(default)]
    pub acp_http_bind_addr: Option<String>,
    /// Whether inbound entry auth is enabled at gateway/edge for exposed HTTP endpoints
    #[serde(default)]
    pub entry_auth_enabled: bool,
    /// Env var name holding entry API key used for HTTP ingress auth
    #[serde(default = "super::defaults::default_runtime_entry_auth_api_key_env")]
    pub entry_auth_api_key_env: String,
    /// Entry layer source-based rate limit (requests per minute)
    #[serde(default = "super::defaults::default_runtime_entry_rate_limit_rpm")]
    pub entry_rate_limit_rpm: u64,
    /// Entry layer token bucket burst capacity per source
    #[serde(default = "super::defaults::default_runtime_entry_rate_limit_burst")]
    pub entry_rate_limit_burst: u64,
    /// Enforce production strict fail-fast checks on unsafe runtime configuration
    #[serde(default)]
    pub production_strict: bool,
    /// Enable OpenTelemetry exporter for distributed traces
    #[serde(default)]
    pub otel_enabled: bool,
    /// Exporter type: otlp or jaeger (jaeger uses OTLP endpoint)
    #[serde(default = "super::defaults::default_runtime_otel_exporter")]
    pub otel_exporter: String,
    /// Optional OTLP endpoint (for Jaeger, point to collector OTLP endpoint)
    #[serde(default)]
    pub otel_endpoint: Option<String>,
    /// OpenTelemetry service name
    #[serde(default = "super::defaults::default_runtime_otel_service_name")]
    pub otel_service_name: String,
    /// Sampling ratio in [0.0, 1.0]
    #[serde(default = "super::defaults::default_runtime_otel_sample_ratio")]
    pub otel_sample_ratio: f64,
    /// Number of slow requests to keep in top-N trace metrics
    #[serde(default = "super::defaults::default_runtime_trace_slow_top_n")]
    pub trace_slow_top_n: usize,
    /// Enable governance subsystem (policy enforcement, RBAC, budget, etc.).
    /// When disabled, governance-related initialization is skipped at startup.
    #[serde(default = "super::defaults::default_true")]
    pub governance_enabled: bool,

    /// Governance policy mode: "active" (enforce), "audit" (log-only), "disabled".
    /// Controls how governance policies are applied during request processing.
    #[serde(default)]
    pub governance_policy_mode: String,

    /// Enable builtin skills (e.g. `builtin.echo`) at server startup.
    /// Default is `true` for development; set to `false` in production (`config.production.toml`).
    #[serde(default = "super::defaults::default_runtime_skills_enabled")]
    pub skills_enabled: bool,
    /// Enable skills import APIs (`skill.import`, `skill.enable`, etc.).
    #[serde(default)]
    pub skills_import_enabled: bool,
    /// Allowed source prefixes for importing skills. Supports trailing `*` wildcard prefix matching.
    #[serde(default)]
    pub skills_allowed_sources: Vec<String>,
    /// Require import requests to provide expected SHA256 digest.
    #[serde(default = "super::defaults::default_runtime_skills_require_sha256")]
    pub skills_require_sha256: bool,
    /// Allow floating refs (`main`, `latest`, non-SHA refs`) when importing from GitHub.
    #[serde(default)]
    pub skills_allow_floating_ref: bool,
    /// Cache directory used to persist imported skill manifests and index.
    #[serde(default = "super::defaults::default_runtime_skills_cache_dir")]
    pub skills_cache_dir: String,
    /// Allowed CORS origins for the ACP HTTP server.
    /// Empty list means CORS is disabled entirely.
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,
    /// Master switch for user-level authentication.
    /// When `false`, all requests are treated as admin (single-user mode).
    #[serde(default)]
    pub user_auth_enabled: bool,
    /// HMAC secret for signing user authentication tokens.
    /// Should be overridden with a strong secret in production.
    #[serde(default = "super::defaults::default_runtime_user_auth_token_secret")]
    pub user_auth_token_secret: String,
    /// Env var name holding the HMAC secret for user auth tokens.
    /// When set, overrides `user_auth_token_secret`.
    #[serde(default = "super::defaults::default_runtime_user_auth_token_secret_env")]
    pub user_auth_token_secret_env: String,
    /// Token TTL in seconds for user authentication tokens (default: 86400 = 24h).
    #[serde(default = "super::defaults::default_runtime_user_auth_token_ttl_seconds")]
    pub user_auth_token_ttl_seconds: u64,
    /// Default daily token limit per tenant (when user auth is enabled).
    #[serde(default = "super::defaults::default_runtime_tenant_default_daily_token_limit")]
    pub tenant_default_daily_token_limit: u64,
    /// Default concurrent tasks limit per tenant.
    #[serde(default = "super::defaults::default_runtime_tenant_default_concurrent_tasks")]
    pub tenant_default_concurrent_tasks: usize,
    /// Default language for i18n (e.g. "en", "zh-CN").
    #[serde(default = "super::defaults::default_runtime_i18n_default_language")]
    pub i18n_default_language: String,
    /// Default daily API call limit per tenant.
    #[serde(default = "super::defaults::default_runtime_tenant_default_daily_api_calls")]
    pub tenant_default_daily_api_calls: usize,
    /// BLUE42 Step 8: Enable DAG-driven tool execution in autonomy loop (default: false)
    #[serde(default)]
    pub enable_dag_execution: bool,
    /// BLUE42 Step 8: Enable adaptive agent reroute on weak rounds (default: true)
    #[serde(default = "super::defaults::default_true")]
    pub enable_agent_reroute: bool,
    /// BLUE42 Step 8: Enable metacognitive + world-model feedback hooks (default: true)
    #[serde(default = "super::defaults::default_true")]
    pub enable_metacognitive_feedback: bool,

    /// BLUE48: Enable Delphi-method debate voting in rationalize_decision.
    /// When enabled, `rationalize_decision` calls `consensus_vote_with_reputation`
    /// for weighted reputation + Delphi debate before applying standard checks.
    #[serde(default)]
    pub enable_delphi_debate: bool,

    // ── Security (GAP-B52) ───────────────────────────────────────────────
    /// Enable request signature verification for incoming JSON-RPC requests.
    /// When enabled, requests must include a `_signature` param with a valid
    /// Ed25519 or HMAC-SHA256 signature (GAP-B52-23).
    #[serde(default)]
    pub request_signing_enabled: bool,

    /// Base64-encoded Ed25519 public key (32 bytes) for request signature
    /// verification. Only used when `request_signing_enabled` is true and
    /// the signing algorithm is Ed25519.
    #[serde(default)]
    pub request_signing_public_key: String,

    /// HMAC shared secret for request signature verification (plaintext).
    /// Only used when `request_signing_enabled` is true and the signing
    /// algorithm is HMAC-SHA256.
    #[serde(default)]
    pub request_signing_hmac_secret: String,

    /// Enable mTLS for the ACP HTTP listener.
    /// Requires paths to CA cert, server cert, and server key.
    #[serde(default)]
    pub mtls_enabled: bool,

    /// Path to the CA certificate file for mTLS.
    #[serde(default)]
    pub mtls_ca_cert_path: String,

    /// Path to the server certificate file for mTLS.
    #[serde(default)]
    pub mtls_server_cert_path: String,

    /// Path to the server private key file for mTLS.
    #[serde(default)]
    pub mtls_server_key_path: String,

    /// Whether to require client certificates in mTLS handshake.
    #[serde(default)]
    pub mtls_require_client_cert: bool,

    /// Comma-separated list of allowed client certificate CNs.
    /// Empty means any valid client cert is accepted.
    #[serde(default)]
    pub mtls_allowed_cns: String,
}

impl RuntimeConfig {
    /// Build a [`CorsConfig`] from the configured origins, or return `None` if
    /// CORS is disabled (empty list).
    pub fn cors_config(&self) -> Option<crate::acp::r#impl::cors::CorsConfig> {
        if self.cors_allowed_origins.is_empty() {
            return None;
        }
        let cfg = crate::acp::r#impl::cors::CorsConfig {
            allowed_origins: self.cors_allowed_origins.clone(),
            ..crate::acp::r#impl::cors::CorsConfig::default()
        };
        Some(cfg)
    }

    /// Build a [`DetectionConfig`] from the RuntimeConfig (BLUE56-GAP-D08).
    /// Uses a production-sensible threshold and enables model check when
    /// governance is active.
    pub fn detection_config(&self) -> crate::security::prompt_injection::DetectionConfig {
        let threshold = if self.governance_enabled && self.governance_policy_mode == "active" {
            0.6
        } else {
            0.8
        };
        crate::security::prompt_injection::DetectionConfig {
            threshold,
            contamination_threshold: 0.7,
            enable_contamination_check: self.governance_enabled && self.user_auth_enabled,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_cache_path")]
    pub path: String,
    #[serde(default = "super::defaults::default_cache_ttl_seconds")]
    pub default_ttl_seconds: u64,
    #[serde(default = "super::defaults::default_cache_max_entries")]
    pub max_entries: usize,
    /// PostgreSQL connection URL (used when compiled with multi-users-server).
    /// Example: "postgres://user:pass@localhost/go_on"
    #[serde(default)]
    pub connection_string: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VectorConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_vector_auto_mode")]
    pub auto_mode: bool,
    #[serde(default = "super::defaults::default_vector_path")]
    pub path: String,
    /// PostgreSQL connection URL (used when compiled with multi-users-server).
    /// Example: "postgres://user:pass@localhost/go_on"
    #[serde(default)]
    pub connection_string: Option<String>,
    #[serde(default = "super::defaults::default_vector_dimensions")]
    pub dimensions: usize,
    #[serde(default = "super::defaults::default_vector_min_query_chars")]
    pub min_query_chars: usize,
    #[serde(default = "super::defaults::default_vector_top_k")]
    pub top_k: usize,
    #[serde(default = "super::defaults::default_vector_min_similarity")]
    pub min_similarity: f32,
    #[serde(default = "super::defaults::default_vector_max_snippet_chars")]
    pub max_snippet_chars: usize,
    #[serde(default = "super::defaults::default_vector_max_entries")]
    pub max_entries: usize,
    #[serde(default = "super::defaults::default_summary_enabled")]
    pub summary_enabled: bool,
    #[serde(default = "super::defaults::default_summary_trigger_messages")]
    pub summary_trigger_messages: usize,
    #[serde(default = "super::defaults::default_summary_max_chars")]
    pub summary_max_chars: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[derive(Default)]
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
    pub supports_vision: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct FlowConfig {
    pub name: String,
    pub phases: Vec<String>,
    #[serde(default)]
    pub workflow_type: WorkflowType,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowType {
    #[default]
    Auto,
    Dev,
    General,
    Free,
    Custom,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ComplianceConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub standards: Vec<String>,
    #[serde(default)]
    pub data_classification_default: String,
    #[serde(default)]
    pub retention_policy_default: String,
    #[serde(default = "super::defaults::default_compliance_audit_retention_days")]
    pub audit_retention_days: u32,
    #[serde(default)]
    pub pii_fields: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StartupContextConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_startup_readme_max_chars")]
    pub readme_max_chars: usize,
    #[serde(default = "super::defaults::default_startup_recent_commits")]
    pub recent_commits: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SchedulerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_scheduler_workers")]
    pub worker_slots: usize,
    #[serde(default = "super::defaults::default_scheduler_max_depth")]
    pub max_queue_depth: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReputationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_reputation_alpha")]
    pub ema_alpha: f64,
    #[serde(default = "super::defaults::default_reputation_degraded")]
    pub degraded_threshold: f64,
    #[serde(default = "super::defaults::default_reputation_excluded")]
    pub exclusion_threshold: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[derive(Default)]
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
    // -----------------------------------------------------------------------
    // Backward-compatible delegation accessors (A7)
    //
    // The agent, provider, and role_registry fields moved into
    // `ProviderConfig`; model_selection_mode moved into
    // `FeatureConfig`.  These accessors keep existing code paths like
    // `config.agents()` working without requiring a rewrite of all
    // call sites.
    // -----------------------------------------------------------------------

    /// Agents map (delegated to `self.provider.agents`).
    pub fn agents(&self) -> &HashMap<String, AgentConfig> {
        &self.provider.agents
    }

    /// Mutable agents map.
    pub fn agents_mut(&mut self) -> &mut HashMap<String, AgentConfig> {
        &mut self.provider.agents
    }

    /// Default phase (delegated to `self.provider.default_phase`).
    pub fn default_phase(&self) -> &str {
        &self.provider.default_phase
    }

    /// Mutable default phase reference.
    pub fn default_phase_mut(&mut self) -> &mut String {
        &mut self.provider.default_phase
    }

    /// Role registry (delegated to `self.provider.role_registry`).
    pub fn role_registry(&self) -> &HashMap<String, RoleDefinition> {
        &self.provider.role_registry
    }

    /// Model selection mode (delegated to `self.feature.model_selection_mode`).
    pub fn model_selection_mode(&self) -> &str {
        &self.feature.model_selection_mode
    }

    /// Returns the effective default phase, accounting for free workflow bypass.
    pub fn effective_default_phase(&self) -> Option<&str> {
        match self.flow.workflow_type {
            WorkflowType::Free => None,
            WorkflowType::General => {
                if self.provider.default_phase.trim().is_empty() {
                    Some("executing")
                } else {
                    Some(self.provider.default_phase.as_str())
                }
            }
            WorkflowType::Custom => {
                if self.provider.default_phase.trim().is_empty() {
                    self.flow.phases.first().map(|phase| phase.as_str())
                } else {
                    Some(self.provider.default_phase.as_str())
                }
            }
            WorkflowType::Dev => {
                if self.provider.default_phase.trim().is_empty() {
                    Some("coding")
                } else {
                    Some(self.provider.default_phase.as_str())
                }
            }
            WorkflowType::Auto => {
                if self.provider.default_phase.trim().is_empty() {
                    Some("coding")
                } else {
                    Some(self.provider.default_phase.as_str())
                }
            }
        }
    }
}
