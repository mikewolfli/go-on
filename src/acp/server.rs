/// ACP server implementation
///
/// This struct represents the main ACP server that handles incoming requests,
/// manages agents, and coordinates the overall system flow.
pub struct AcpServer {
    /// Flow manager for handling request routing through phases
    flow: Arc<StdMutex<Arc<FlowManager>>>,
    /// Agent registry for managing available agents
    registry: Arc<StdMutex<Arc<AgentRegistry>>>,
    /// Response cache (SQLite-based)
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    /// Vector store for similarity search and memory
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    /// Vector store configuration
    vector_config: Arc<StdMutex<Option<VectorConfig>>>,
    /// Autotune state for adaptive configuration
    autotune: Arc<StdMutex<Option<Arc<Mutex<AutoTuneState>>>>>,
    /// Autotune configuration
    autotune_config: Arc<StdMutex<Option<AutoTuneConfig>>>,
    /// Path to autotune state file
    autotune_state_path: Arc<StdMutex<Option<String>>>,
    /// Runtime configuration
    runtime_config: Arc<StdMutex<RuntimeConfig>>,
    /// Runtime metrics collection
    metrics: Arc<RuntimeMetrics>,
    /// Online controller for adaptive strategy from live outcomes
    online_controller: Arc<StdMutex<OnlineControllerState>>,
    /// OpenTelemetry runtime bridge
    telemetry: Arc<TelemetryRuntime>,
    /// In-memory request trace events (phase-1 OTel-compatible)
    trace_events: Arc<StdMutex<Vec<TraceEvent>>>,
    /// In-memory response cache for fast access
    memory_cache: Arc<MemoryResponseCache>,
    /// Conversation checkpoint store for branch/rollback control
    conversation_store: Arc<StdMutex<HashMap<String, ConversationState>>>,
    /// Most-recently touched conversations; used for bounded conversation-store eviction
    conversation_touch_order: Arc<StdMutex<Vec<String>>>,
    /// Maintenance tracker for system health
    maintenance: Arc<MaintenanceTracker>,
    /// Lifecycle state management
    lifecycle: Arc<LifecycleState>,
    /// Circuit breakers for agent failure handling
    circuit_breakers: Arc<CircuitBreakerRegistry>,
    /// Adaptive model selector: tracks per-model success rates for auto-switching
    adaptive_model_selector: Arc<StdMutex<AdaptiveModelSelector>>,
    /// Rate limiter for phase-level throttling
    phase_rate_limiter: Arc<PhaseRateLimiter>,
    /// In-flight request limiter
    inflight_limiter: Arc<InflightLimiter>,
    /// Path to configuration file
    config_path: Option<PathBuf>,
    /// Forced phase name (if specified)
    forced_phase: Option<String>,
    /// HTTP client for external requests
    http_client: Option<reqwest::Client>,
    /// Verbose logging flag
    verbose: bool,
    /// Output stream for responses
    output: Arc<Mutex<tokio::io::Stdout>>,
    /// Shutdown notification mechanism
    shutdown_notify: Arc<Notify>,
}

include!("impl_core.rs");
include!("impl_request.rs");
include!("impl_chat.rs");