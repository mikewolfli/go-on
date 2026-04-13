use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

use serde::Serialize;

pub struct ManagedProcess {
    pub child: Child,
    pub pid: u32,
    pub started_at: SystemTime,
}

#[derive(Clone)]
pub struct ProcessConfig {
    pub executable_path: String,
    pub working_dir: String,
    pub log_path: String,
    pub extra_env: HashMap<String, String>,
}

#[derive(Default, Clone)]
pub struct RuntimeCounters {
    pub requests_total: u64,
    pub requests_success: u64,
    pub avg_latency_ms: f64,
    pub timeout_count: u64,
    pub rate_limit_count: u64,
    pub breaker_count: u64,
    pub upstream_failure_count: u64,
}

pub struct InnerState {
    pub config: ProcessConfig,
    pub process: Option<ManagedProcess>,
    pub counters: RuntimeCounters,
    pub recent_request_instants: VecDeque<Instant>,
    pub usage_events: VecDeque<UsageEvent>,
    pub endpoint_health: HashMap<String, EndpointHealthCounter>,
    pub last_restart_at: Option<Instant>,
    pub stop_requested: bool,
    pub crash_notified: bool,
    pub crash_message: Option<String>,
}

#[derive(Clone)]
pub struct UsageEvent {
    pub at: Instant,
    pub phase: Option<String>,
    pub agent: Option<String>,
}

#[derive(Default, Clone)]
pub struct EndpointHealthCounter {
    pub total: u64,
    pub success: u64,
    pub failure: u64,
    pub avg_latency_ms: f64,
}

pub struct AppState(pub Mutex<InnerState>);

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub executable_path: Option<String>,
    pub working_dir: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub started_at: Option<String>,
    pub last_error: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        let default_exe = if cfg!(target_os = "windows") {
            "go-on.exe"
        } else {
            "go-on"
        };
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let log_path = cwd.join("go-on.log");

        Self(Mutex::new(InnerState {
            config: ProcessConfig {
                executable_path: cwd.join(default_exe).to_string_lossy().to_string(),
                working_dir: cwd.to_string_lossy().to_string(),
                log_path: log_path.to_string_lossy().to_string(),
                extra_env: HashMap::new(),
            },
            process: None,
            counters: RuntimeCounters::default(),
            recent_request_instants: VecDeque::new(),
            usage_events: VecDeque::new(),
            endpoint_health: HashMap::new(),
            last_restart_at: None,
            stop_requested: false,
            crash_notified: false,
            crash_message: None,
        }))
    }
}
