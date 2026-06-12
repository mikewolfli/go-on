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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use tracing::{error, info, warn};

// ── Thresholds ──────────────────────────────────────────────────────────────

/// Free memory threshold (MB) below which a startup warning is emitted.
pub const MEMORY_WARN_MB: u64 = 512;

/// Free memory threshold (MB) below which the server will refuse to start.
pub const MEMORY_CRITICAL_MB: u64 = 256;

/// Free memory threshold (MB) below which we abort immediately.
pub const MEMORY_JETSAM_RISK_MB: u64 = 128;

/// How often the runtime monitor checks memory pressure (seconds).
pub const MEMORY_MONITOR_INTERVAL_SECS: u64 = 30;

// ── System Memory Info ──────────────────────────────────────────────────────

/// Snapshot of system memory state.
#[derive(Debug, Clone, Default)]
pub struct SystemMemoryInfo {
    /// Total physical RAM in bytes.
    pub total_bytes: u64,
    /// Approximate free (available) memory in bytes.
    pub free_bytes: u64,
    /// Active memory in bytes.
    #[allow(dead_code)] // F-GAP-49 — reserved health metrics fields
    // populated by query_system_memory; reserved for future pressure analysis
    pub active_bytes: u64,
    /// Wired (unpageable) memory in bytes.
    #[allow(dead_code)] // F-GAP-49 — reserved health metrics fields
    // populated by query_system_memory; reserved for future pressure analysis
    pub wired_bytes: u64,
    /// Swap usage in bytes (0 if swap is disabled).
    #[allow(dead_code)] // F-GAP-49 — reserved health metrics fields
    // populated by query_system_memory; reserved for future pressure analysis
    pub swap_used_bytes: u64,
    /// Swap total capacity in bytes.
    pub swap_total_bytes: u64,
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

    /// Whether the system has no swap available.
    pub fn swap_disabled(&self) -> bool {
        self.swap_total_bytes == 0
    }
}

// ── Query System Memory ─────────────────────────────────────────────────────

/// Query system memory information.
///
/// Uses platform-specific mechanisms:
/// - **macOS**: `sysctl hw.memsize`, `vm_stat`, `sysctl vm.swapusage`, `sysctl kern.memorystatus_vm_pressure_level`
/// - **Linux**: `/proc/meminfo`
/// - **Windows**: `GlobalMemoryStatusEx`
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

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        SystemMemoryInfo {
            total_bytes: 0,
            free_bytes: 0,
            active_bytes: 0,
            wired_bytes: 0,
            swap_used_bytes: 0,
            swap_total_bytes: 0,
            pressure_level: 0,
        }
    }
}

/// macOS-specific memory query using `sysctl` and `vm_stat`.
#[cfg(target_os = "macos")]
fn query_macos_memory() -> SystemMemoryInfo {
    // Total physical memory
    let total_bytes = sysctl_u64("hw.memsize").unwrap_or(0);

    // Memory pressure level
    let pressure_level = sysctl_u64("kern.memorystatus_vm_pressure_level").unwrap_or(0) as u8;

    // Parse vm_stat output for page counts
    let (free_pages, active_pages, wired_pages, speculative_pages) = vm_stat_pages();

    // Page size on Apple Silicon / Intel Macs
    let page_size = sysctl_u64("hw.pagesize").unwrap_or(16384);

    let active_bytes = active_pages * page_size;
    let wired_bytes = wired_pages * page_size;

    // Free = free pages + speculative (can be reclaimed) + purgeable
    let free_bytes = (free_pages + speculative_pages) * page_size;

    // Swap usage from sysctl vm.swapusage
    let swap_used_bytes;
    let swap_total_bytes;
    {
        let output = std::process::Command::new("sysctl")
            .args(["vm.swapusage"])
            .output();
        match output {
            Ok(out) if out.status.success() => {
                let s = String::from_utf8_lossy(&out.stdout);
                // Format: vm.swapusage: total = 0.00M  used = 0.00M  free = 0.00M  (encrypted)
                if let Some(rest) = s.strip_prefix("vm.swapusage:") {
                    let parts: Vec<&str> = rest.split(',').collect();
                    let total_str = parts
                        .first()
                        .and_then(|p| p.split('=').nth(1))
                        .unwrap_or("0")
                        .trim()
                        .trim_end_matches('M');
                    let used_str = parts
                        .get(1)
                        .and_then(|p| p.split('=').nth(1))
                        .unwrap_or("0")
                        .trim()
                        .trim_end_matches('M');
                    swap_total_bytes =
                        (total_str.parse::<f64>().unwrap_or(0.0) * 1024.0 * 1024.0) as u64;
                    swap_used_bytes =
                        (used_str.parse::<f64>().unwrap_or(0.0) * 1024.0 * 1024.0) as u64;
                } else {
                    swap_total_bytes = 0;
                    swap_used_bytes = 0;
                }
            }
            _ => {
                swap_total_bytes = 0;
                swap_used_bytes = 0;
            }
        }
    }

    SystemMemoryInfo {
        total_bytes,
        free_bytes,
        active_bytes,
        wired_bytes,
        swap_used_bytes,
        swap_total_bytes,
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

/// Parse `vm_stat` output to get page counts for free, active, wired, and speculative pages.
#[cfg(target_os = "macos")]
fn vm_stat_pages() -> (u64, u64, u64, u64) {
    let output = std::process::Command::new("vm_stat").output().ok();
    let Some(out) = output else {
        return (0, 0, 0, 0);
    };
    if !out.status.success() {
        return (0, 0, 0, 0);
    }

    let s = String::from_utf8_lossy(&out.stdout);
    let mut free: u64 = 0;
    let mut active: u64 = 0;
    let mut wired: u64 = 0;
    let mut speculative: u64 = 0;

    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // vm_stat output format: "Pages free:                         4148."
        if let Some(val) = parse_vm_stat_line(line, "Pages free:") {
            free = val;
        } else if let Some(val) = parse_vm_stat_line(line, "Pages active:") {
            active = val;
        } else if let Some(val) = parse_vm_stat_line(line, "Pages wired down:") {
            wired = val;
        } else if let Some(val) = parse_vm_stat_line(line, "Pages speculative:") {
            speculative = val;
        }
    }

    (free, active, wired, speculative)
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
    let mut active_bytes: u64 = 0;

    for line in meminfo.lines() {
        if let Some(val) = parse_meminfo_line(line, "MemTotal:") {
            total_bytes = val * 1024;
        } else if let Some(val) = parse_meminfo_line(line, "MemAvailable:") {
            free_bytes = val * 1024;
        } else if let Some(val) = parse_meminfo_line(line, "Active:") {
            active_bytes = val * 1024;
        }
    }

    let swap_total_bytes;
    let swap_used_bytes;
    {
        let mut swap_total: u64 = 0;
        let mut swap_free: u64 = 0;
        for line in meminfo.lines() {
            if let Some(val) = parse_meminfo_line(line, "SwapTotal:") {
                swap_total = val * 1024;
            } else if let Some(val) = parse_meminfo_line(line, "SwapFree:") {
                swap_free = val * 1024;
            }
        }
        swap_total_bytes = swap_total;
        swap_used_bytes = swap_total.saturating_sub(swap_free);
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
        active_bytes,
        wired_bytes: 0, // Not easily available on Linux from /proc/meminfo
        swap_used_bytes,
        swap_total_bytes,
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
                    let page_size = 4096u64;
                    return SystemMemoryInfo {
                        total_bytes: total_kb * 1024,
                        free_bytes: free_kb * 1024,
                        active_bytes: (total_kb - free_kb) * 1024,
                        wired_bytes: 0,
                        swap_used_bytes: 0,
                        swap_total_bytes: 0,
                        pressure_level: 0,
                    };
                }
            }
            SystemMemoryInfo::default()
        }
        _ => SystemMemoryInfo::default(),
    }
}

// ── Startup Health Check ────────────────────────────────────────────────────

/// Result of a pre-startup memory health check.
// F-GAP-11 — reserved for future startup health-check integration
#[allow(dead_code)] // F-GAP-49 — reserved memory health features
// F-GAP-49 — reserved memory health monitor; wire when memory pressure detection is enabled
// F-GAP-49 — reserved for future use
#[derive(Debug, Clone, PartialEq)]
pub enum MemoryHealth {
    /// Memory is sufficient to run normally.
    Healthy,
    /// Memory is low — server will start but with warnings.
    Low { free_mb: u64, message: String },
    /// Memory is critically low — server should refuse to start.
    Critical { free_mb: u64, message: String },
    /// Could not determine memory status.
    Unknown,
}

/// Perform a pre-startup memory health check.
///
/// Returns `MemoryHealth::Healthy` if the system has sufficient memory.
/// Returns `MemoryHealth::Low` if memory is constrained (warns user).
/// Returns `MemoryHealth::Critical` if starting would likely trigger OOM.
// F-GAP-11 — reserved for future startup health-check integration
#[allow(dead_code)] // F-GAP-49 — reserved memory health monitor; wire when memory pressure detection is enabled
                    // F-GAP-49 — reserved for future use
pub fn check_startup_memory() -> MemoryHealth {
    let info = query_system_memory();
    let free_mb = info.free_mb();

    // Check pressure level first (macOS)
    if info.pressure_level >= 4 {
        return MemoryHealth::Critical {
            free_mb,
            message: format!(
                "System memory pressure level is PANIC ({})! System is at imminent risk of crash.",
                info.pressure_level
            ),
        };
    }
    if info.pressure_level >= 3 {
        return MemoryHealth::Critical {
            free_mb,
            message: format!(
                "System memory pressure level is CRITICAL ({}). Server cannot start safely.",
                info.pressure_level
            ),
        };
    }

    // Check available free memory
    if free_mb < MEMORY_JETSAM_RISK_MB && info.total_bytes > 0 {
        return MemoryHealth::Critical {
            free_mb,
            message: format!(
                "Only {} MB free memory available (below {} MB threshold). \
                 The server would likely be killed by the OOM killer immediately.",
                free_mb, MEMORY_JETSAM_RISK_MB,
            ),
        };
    }
    if free_mb < MEMORY_CRITICAL_MB && info.total_bytes > 0 {
        return MemoryHealth::Critical {
            free_mb,
            message: format!(
                "Only {} MB free memory available (below {} MB critical threshold). \
                 Risk of OOM killer (SIGKILL).",
                free_mb, MEMORY_CRITICAL_MB,
            ),
        };
    }
    if free_mb < MEMORY_WARN_MB && info.total_bytes > 0 {
        return MemoryHealth::Low {
            free_mb,
            message: format!(
                "Only {} MB free memory available (below {} MB warning threshold). \
                 Consider closing other applications or using --low-memory mode.",
                free_mb, MEMORY_WARN_MB,
            ),
        };
    }

    // Warn if swap is disabled and memory is tight
    if info.swap_disabled() && info.total_bytes > 0 && free_mb < 2048 {
        return MemoryHealth::Low {
            free_mb,
            message: format!(
                "Swap is disabled and only {} MB free. Without swap, the system has \
                 no fallback when memory is exhausted.",
                free_mb,
            ),
        };
    }

    if info.total_bytes == 0 {
        return MemoryHealth::Unknown;
    }

    MemoryHealth::Healthy
}

/// Print a formatted memory health report to stderr.
pub fn print_memory_health(health: &MemoryHealth) {
    match health {
        MemoryHealth::Healthy => {
            let info = query_system_memory();
            info!(
                "system memory: total={} MB, free={} MB, pressure_level={}",
                info.total_mb(),
                info.free_mb(),
                info.pressure_level,
            );
        }
        MemoryHealth::Low { free_mb, message } => {
            warn!(
                free_mb = %free_mb,
                "{}", message,
            );
            eprintln!(
                "\n⚠️  Low Memory Warning: {} MB free\n   {}\n",
                free_mb, message
            );
        }
        MemoryHealth::Critical { free_mb, message } => {
            error!(
                free_mb = %free_mb,
                "MEMORY CRITICAL: {}", message,
            );
            eprintln!(
                "\n🚫 MEMORY CRITICAL: {} MB free\n   {}\n",
                free_mb, message
            );
        }
        MemoryHealth::Unknown => {
            info!("could not determine system memory state — proceeding without checks");
        }
    }
}

// ── Runtime Memory Monitor ──────────────────────────────────────────────────

/// Runtime memory state, accessed atomically.
static RUNTIME_MEMORY_FREE_MB: AtomicU64 = AtomicU64::new(0);
static RUNTIME_MEMORY_TOTAL_MB: AtomicU64 = AtomicU64::new(0);
static RUNTIME_PRESSURE_LEVEL: AtomicU64 = AtomicU64::new(0);
static MEMORY_MONITOR_INITIALIZED: OnceLock<bool> = OnceLock::new();

#[allow(dead_code)] // F-GAP-49 — reserved memory health monitor; wire when memory pressure detection is enabled
/// Get the last known free memory in MB (from the runtime monitor).
pub fn runtime_free_mb() -> u64 {
    RUNTIME_MEMORY_FREE_MB.load(Ordering::Relaxed)
}

#[allow(dead_code)] // F-GAP-49 — reserved memory health monitor; wire when memory pressure detection is enabled
/// Get the last known total memory in MB.
pub fn runtime_total_mb() -> u64 {
    RUNTIME_MEMORY_TOTAL_MB.load(Ordering::Relaxed)
}

#[allow(dead_code)] // F-GAP-49 — reserved memory health monitor; wire when memory pressure detection is enabled
/// Get the last known macOS memory pressure level.
pub fn runtime_pressure_level() -> u8 {
    RUNTIME_PRESSURE_LEVEL.load(Ordering::Relaxed) as u8
}

/// Start a background task that periodically checks system memory.
///
/// Spawns a tokio task that queries `query_system_memory()` every
/// `MEMORY_MONITOR_INTERVAL_SECS` seconds, logs warnings if
/// memory is critically low, and evaluates AlertManager rules.
pub fn start_memory_monitor() {
    MEMORY_MONITOR_INITIALIZED.set(true).unwrap_or(());

    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(MEMORY_MONITOR_INTERVAL_SECS));
        // Get the global AlertManager for threshold-based alerting
        let alert_manager = crate::observability::alert_manager::alert_manager();

        loop {
            interval.tick().await;
            let info = query_system_memory();
            let free_mb = info.free_mb();
            RUNTIME_MEMORY_FREE_MB.store(free_mb, Ordering::Relaxed);
            RUNTIME_MEMORY_TOTAL_MB.store(info.total_mb(), Ordering::Relaxed);
            RUNTIME_PRESSURE_LEVEL.store(info.pressure_level as u64, Ordering::Relaxed);

            // Evaluate memory thresholds against AlertManager rules
            if let Ok(mut am) = alert_manager.lock() {
                let _jetsam_threshold = (MEMORY_JETSAM_RISK_MB as f64).max(50.0);
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

                // Also check jetsam risk specifically
                if free_mb < MEMORY_JETSAM_RISK_MB && info.total_bytes > 0 {
                    let _jetsam_alerts = am.evaluate("memory_jetsam_risk", free_mb as f64);
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
    });
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

    // Estimate safe values based on free memory
    let (cache_max, vector_max, inflight_max) = if low_memory_mode {
        // Ultra-conservative: absolute minimum
        (500, 500, 8)
    } else if free_mb < 256 {
        // Critical: severely limit
        (500, 500, 8)
    } else if free_mb < 512 {
        // Low
        (1000, 1000, 16)
    } else if free_mb < 1024 {
        // Moderate
        (2000, 2000, 32)
    } else if free_mb < 2048 {
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
