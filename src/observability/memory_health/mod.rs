//! System memory health checking and monitoring.
//!
//! This module provides:
//! - Pre-startup memory availability checks
//! - Runtime memory pressure monitoring
//! - System memory info queries (macOS, Linux, Windows)
//! - Adaptive resource limiting based on available memory
//!
//! # Memory Pressure Levels (macOS)
//!
//! macOS uses a `memory_pressure` mechanism to classify system memory state:
//!
//! | Level | `kern.memorystatus_vm_pressure_level` | Meaning |
//! |-------|--------------------------------------|---------|
//! | Normal    | 1 | Sufficient memory |
//! | Warning   | 2 | Memory is constrained |
//! | Critical  | 3 | Memory is critically low — Jetsam may kill processes |
//! | Panic     | 4 | System will likely panic/reboot |
//!
//! # Thresholds
//!
//! - **`MEMORY_WARN_MB`** (512 MB): Log warning if free memory below this
//! - **`MEMORY_CRITICAL_MB`** (256 MB): Refuse to start if free memory below this
//! - **`MEMORY_JETSAM_RISK_MB`** (128 MB): Immediate abort to avoid data corruption

// anyhow not needed here — no fallible functions
use tracing::{error, info, warn};

// ── Thresholds ──────────────────────────────────────────────────────────────

/// Free memory threshold (MB) below which a startup warning is emitted.
pub const MEMORY_WARN_MB: u64 = 512;

/// Free memory threshold (MB) below which the server will refuse to start.
pub const MEMORY_CRITICAL_MB: u64 = 256;

/// Free memory threshold (MB) below which we abort immediately.
pub const MEMORY_JETSAM_RISK_MB: u64 = 128;

/// Free memory threshold (MB) below which resource limits are tightened
/// (the "moderate" tier in [`estimate_safe_limits`]).
pub const MEMORY_MODERATE_MB: u64 = 1024;

/// Free memory threshold (MB) below which resource limits are relaxed
/// (the "comfortable" tier in [`estimate_safe_limits`]).
pub const MEMORY_COMFORTABLE_MB: u64 = 2048;

// ── System Memory Info ──────────────────────────────────────────────────────

/// Snapshot of system memory state.
#[derive(Debug, Clone, Default)]
pub struct SystemMemoryInfo {
    /// Total physical RAM in bytes.
    pub total_bytes: u64,
    /// Approximate free (available) memory in bytes.
    pub free_bytes: u64,
    /// macOS memory pressure level (1=normal, 2=warning, 3=critical, 4=panic). 0 if unavailable.
    pub pressure_level: u8,
}

impl SystemMemoryInfo {
    /// Free memory in MB.
    pub fn free_mb(&self) -> u64 {
        self.free_bytes / (1024 * 1024)
    }

    /// Total memory in MB.
    pub fn total_mb(&self) -> u64 {
        self.total_bytes / (1024 * 1024)
    }

    /// Whether the system is under critical memory pressure.
    pub fn is_critical(&self) -> bool {
        self.free_mb() < MEMORY_CRITICAL_MB || self.pressure_level >= 3
    }

    /// Whether the system is under warning memory pressure.
    pub fn is_warning(&self) -> bool {
        self.free_mb() < MEMORY_WARN_MB || self.pressure_level >= 2
    }
}

// ── Query System Memory ─────────────────────────────────────────────────────

/// Query system memory information.
///
/// Uses platform-specific mechanisms:
/// - **macOS**: `sysctl hw.memsize`, `vm_stat`, `sysctl vm.swapusage`, `sysctl kern.memorystatus_vm_pressure_level`
/// - **Linux**: `/proc/meminfo`
/// - **Windows**: `GlobalMemoryStatusEx`
///
/// # Blocking caveat
/// The macOS branch spawns `sysctl`/`vm_stat` subprocesses (blocking I/O) and
/// the Linux branch reads `/proc`; callers on tokio workers (e.g. the periodic
/// alert loop in `acp/background.rs`) should route this through
/// `tokio::task::spawn_blocking` on macOS. See `evaluate_memory_alerts`.
pub fn query_system_memory() -> SystemMemoryInfo {
    #[cfg(target_os = "macos")]
    {
        query_macos_memory()
    }
    #[cfg(target_os = "linux")]
    {
        query_linux_memory()
    }
    #[cfg(target_os = "windows")]
    {
        query_windows_memory()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    SystemMemoryInfo {
        total_bytes: 0,
        free_bytes: 0,
        pressure_level: 0,
    }
}

/// macOS-specific memory query using `sysctl` and `vm_stat`.
#[cfg(target_os = "macos")]
fn query_macos_memory() -> SystemMemoryInfo {
    // Total physical memory
    let total_bytes = sysctl_u64("hw.memsize").unwrap_or(0);

    // Memory pressure level
    let pressure_level = sysctl_u64("kern.memorystatus_vm_pressure_level").unwrap_or(0) as u8;

    // Parse vm_stat output for page counts (free + speculative are reclaimable)
    let (free_pages, speculative_pages) = vm_stat_pages();

    // Page size on Apple Silicon / Intel Macs
    let page_size = sysctl_u64("hw.pagesize").unwrap_or(16384);

    // Free = free pages + speculative (can be reclaimed) + purgeable
    let free_bytes = (free_pages + speculative_pages) * page_size;
    // A live macOS system always reports some free pages. `(0, 0)` with a
    // non-zero total means `vm_stat` failed (sandbox/container) — the
    // `free_bytes = 0, total_bytes > 0` combination must NOT be reported as
    // "0 MB free" (which would refuse to start the server); see
    // `check_startup_memory`.
    let query_failed = total_bytes > 0 && free_pages == 0 && speculative_pages == 0;

    SystemMemoryInfo {
        total_bytes: if query_failed { 0 } else { total_bytes },
        free_bytes: if query_failed { 0 } else { free_bytes },
        pressure_level,
    }
}

/// Read a numeric sysctl value on macOS.
#[cfg(target_os = "macos")]
fn sysctl_u64(key: &str) -> Option<u64> {
    let output = std::process::Command::new("sysctl")
        .args(["-n", key])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout);
    s.trim().parse::<u64>().ok()
}

/// Parse `vm_stat` output to get page counts for free and speculative pages.
#[cfg(target_os = "macos")]
fn vm_stat_pages() -> (u64, u64) {
    let output = std::process::Command::new("vm_stat").output().ok();
    let Some(out) = output else {
        return (0, 0);
    };
    if !out.status.success() {
        return (0, 0);
    }

    let s = String::from_utf8_lossy(&out.stdout);
    let mut free: u64 = 0;
    let mut speculative: u64 = 0;

    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // vm_stat output format: "Pages free:                         4148."
        if let Some(val) = parse_vm_stat_line(line, "Pages free:") {
            free = val;
        } else if let Some(val) = parse_vm_stat_line(line, "Pages speculative:") {
            speculative = val;
        }
    }

    (free, speculative)
}

/// Parse a single line from `vm_stat` output.
#[cfg(target_os = "macos")]
fn parse_vm_stat_line(line: &str, prefix: &str) -> Option<u64> {
    if let Some(rest) = line.strip_prefix(prefix) {
        let num_str = rest.trim().trim_end_matches('.');
        return num_str.parse::<u64>().ok();
    }
    None
}

/// Linux-specific memory query from `/proc/meminfo`.
#[cfg(target_os = "linux")]
fn query_linux_memory() -> SystemMemoryInfo {
    use std::fs;

    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();

    let mut total_bytes: u64 = 0;
    let mut free_bytes: u64 = 0;

    for line in meminfo.lines() {
        if let Some(val) = parse_meminfo_line(line, "MemTotal:") {
            total_bytes = val * 1024;
        } else if let Some(val) = parse_meminfo_line(line, "MemAvailable:") {
            free_bytes = val * 1024;
        }
    }

    // Read memory pressure from /proc/pressure/memory (if available)
    let pressure_level = if let Ok(pressure) = fs::read_to_string("/proc/pressure/memory") {
        // Format: "some avg10=0.00 avg60=0.00 avg300=0.00 total=0"
        if let Some(line) = pressure.lines().next() {
            if let Some(avg) = line
                .split_whitespace()
                .find_map(|s| s.strip_prefix("avg10="))
            {
                let val: f64 = avg.parse().unwrap_or(0.0);
                if val > 70.0 {
                    4
                } else if val > 40.0 {
                    3
                } else if val > 20.0 {
                    2
                } else {
                    1
                }
            } else {
                0
            }
        } else {
            0
        }
    } else {
        0
    };

    SystemMemoryInfo {
        total_bytes,
        free_bytes,
        pressure_level,
    }
}

/// Parse a line from `/proc/meminfo` like `MemTotal:       16384000 kB`.
#[cfg(target_os = "linux")]
fn parse_meminfo_line(line: &str, prefix: &str) -> Option<u64> {
    if let Some(rest) = line.strip_prefix(prefix) {
        let rest = rest.trim();
        // Remove " kB" suffix and parse
        let num_str = rest.strip_suffix(" kB").unwrap_or(rest);
        return num_str.trim().parse::<u64>().ok();
    }
    None
}

/// Windows-specific memory query.
#[cfg(target_os = "windows")]
fn query_windows_memory() -> SystemMemoryInfo {
    // We use `wmic` or `Get-CimInstance` as a fallback.
    // The performance.rs already has a Windows memory function using K32GetProcessMemoryInfo,
    // but that's for per-process memory. Here we need system-wide.
    let output = std::process::Command::new("wmic")
        .args([
            "OS",
            "get",
            "TotalVisibleMemorySize,FreePhysicalMemory",
            "/format:csv",
        ])
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout);
            for line in s.lines().skip(1) {
                // CSV: Node,TotalVisibleMemorySize,FreePhysicalMemory
                let fields: Vec<&str> = line.split(',').collect();
                if fields.len() >= 3 {
                    let total_kb = fields[1].trim().parse::<u64>().unwrap_or(0);
                    let free_kb = fields[2].trim().parse::<u64>().unwrap_or(0);
                    return SystemMemoryInfo {
                        total_bytes: total_kb * 1024,
                        free_bytes: free_kb * 1024,
                        pressure_level: 0,
                    };
                }
            }
            SystemMemoryInfo::default()
        }
        _ => SystemMemoryInfo::default(),
    }
}

// ── Runtime Memory Monitor ──────────────────────────────────────────────────

/// Evaluate current system memory against the AlertManager memory rules
/// (memory_free_mb / memory_low / memory_critical / memory_jetsam_risk).
/// Shared by the startup one-shot check and the periodic 30s alert loop so
/// runtime memory degradation is actually observed (previously the rules
/// were only evaluated once at startup).
///
/// # Blocking caveat
/// `query_system_memory` spawns subprocesses on macOS (`sysctl`/`vm_stat`)
/// and reads `/proc` on Linux. Both call sites run it off the tokio worker
/// through `tokio::task::spawn_blocking`: the periodic 30s loop in
/// `acp/background.rs` and the startup one-shot in
/// `acp/impl/runtime/server_builder.rs` (via [`start_memory_monitor`]).
pub fn evaluate_memory_alerts() {
    let info = query_system_memory();
    let free_mb = info.free_mb();

    // Evaluate memory thresholds against AlertManager rules.
    // NOTE: a second evaluate("memory_jetsam_risk") was removed — the
    // "memory_free_mb" call above already evaluates every rule sharing the
    // "memory" keyword prefix (including memory_jetsam_risk).
    let alert_manager = crate::observability::alert_manager::alert_manager();
    if let Ok(mut am) = alert_manager.lock() {
        let alerts = am.evaluate("memory_free_mb", free_mb as f64);
        if !alerts.is_empty() {
            for alert in &alerts {
                tracing::warn!(
                    alert_rule = %alert.rule,
                    severity = %alert.severity,
                    "Memory health alert: {}",
                    alert.message
                );
            }
        }
    }

    if info.is_critical() {
        error!(
            free_mb = %free_mb,
            total_mb = %info.total_mb(),
            pressure_level = %info.pressure_level,
            trace_id = %uuid::Uuid::new_v4(),
            "CRITICAL MEMORY PRESSURE — system may OOM kill this process",
        );
    } else if info.is_warning() {
        warn!(
            free_mb = %free_mb,
            total_mb = %info.total_mb(),
            pressure_level = %info.pressure_level,
            trace_id = %uuid::Uuid::new_v4(),
            "Low memory — consider reducing load",
        );
    }
}

/// Start a one-shot memory check at startup: logs warnings if
/// memory is critically low, and evaluates AlertManager rules.
pub fn start_memory_monitor() {
    evaluate_memory_alerts();
}

// ── Memory Health Check ───────────────────────────────────────────────────

/// Categorization of system memory health for startup decisions.
#[derive(Debug, Clone, PartialEq)]
pub enum MemoryHealth {
    /// Sufficient memory available.
    Healthy,
    /// Low memory — warning but not fatal.
    Low { free_mb: u64 },
    /// Critically low memory — should refuse to start.
    Critical { free_mb: u64, message: String },
    /// Unable to determine memory status.
    Unknown,
}

/// Perform a pre-startup memory availability check.
pub fn check_startup_memory() -> MemoryHealth {
    let info = query_system_memory();
    let free_mb = info.free_mb();

    if free_mb == 0 && info.total_bytes == 0 {
        return MemoryHealth::Unknown;
    }

    if free_mb < MEMORY_JETSAM_RISK_MB {
        MemoryHealth::Critical {
            free_mb,
            message: format!(
                "Only {} MB free — below Jetsam risk threshold ({} MB). Aborting to prevent data corruption.",
                free_mb, MEMORY_JETSAM_RISK_MB
            ),
        }
    } else if free_mb < MEMORY_CRITICAL_MB {
        MemoryHealth::Critical {
            free_mb,
            message: format!(
                "Only {} MB free — below critical threshold ({} MB). Refusing to start.",
                free_mb, MEMORY_CRITICAL_MB
            ),
        }
    } else if free_mb < MEMORY_WARN_MB {
        MemoryHealth::Low { free_mb }
    } else {
        MemoryHealth::Healthy
    }
}

/// Log a human-readable summary of the memory health check.
pub fn print_memory_health(health: &MemoryHealth) {
    match health {
        MemoryHealth::Healthy => {
            info!("Memory check: Healthy");
        }
        MemoryHealth::Low { free_mb } => {
            warn!(
                "Memory check: Low — only {} MB free (threshold: {} MB)",
                free_mb, MEMORY_WARN_MB
            );
        }
        MemoryHealth::Critical { free_mb, message } => {
            error!("Memory check: CRITICAL — {} MB free. {}", free_mb, message);
        }
        MemoryHealth::Unknown => {
            info!("Memory check: Unable to determine system memory (assuming healthy)");
        }
    }
}

/// Estimate safe resource limits based on available memory.
///
/// Returns recommended `(cache_max_entries, vector_max_entries, max_inflight)`.
pub fn estimate_safe_limits(
    user_cache: Option<usize>,
    user_vector: Option<usize>,
    user_inflight: Option<usize>,
    low_memory_mode: bool,
) -> (usize, usize, usize) {
    let info = query_system_memory();
    let free_mb = info.free_mb();

    // If we couldn't determine available memory, use user values or conservative defaults
    if free_mb == 0 && info.total_bytes == 0 {
        return (
            user_cache.unwrap_or(1000),
            user_vector.unwrap_or(2000),
            user_inflight.unwrap_or(16),
        );
    }

    // Estimate safe values based on free memory. Thresholds reference the
    // shared public constants (MEMORY_CRITICAL_MB / MEMORY_WARN_MB / …) so
    // the tier boundaries cannot drift from the documented thresholds.
    let (cache_max, vector_max, inflight_max) = if low_memory_mode {
        // Ultra-conservative: absolute minimum
        (500, 500, 8)
    } else if free_mb < MEMORY_CRITICAL_MB {
        // Critical: severely limit
        (500, 500, 8)
    } else if free_mb < MEMORY_WARN_MB {
        // Low
        (1000, 1000, 16)
    } else if free_mb < MEMORY_MODERATE_MB {
        // Moderate
        (2000, 2000, 32)
    } else if free_mb < MEMORY_COMFORTABLE_MB {
        // Comfortable
        (3000, 5000, 64)
    } else {
        // Plenty of memory
        (
            user_cache.unwrap_or(5000),
            user_vector.unwrap_or(10000),
            user_inflight.unwrap_or(128),
        )
    };

    // User values cap at safe estimates (don't exceed what memory allows)
    let cache_max = user_cache.map(|u| u.min(cache_max)).unwrap_or(cache_max);
    let vector_max = user_vector.map(|u| u.min(vector_max)).unwrap_or(vector_max);
    let inflight_max = user_inflight
        .map(|u| u.min(inflight_max))
        .unwrap_or(inflight_max);

    (cache_max, vector_max, inflight_max)
}
