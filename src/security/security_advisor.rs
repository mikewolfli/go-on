//! Security Advisor Agent (GAP-B52-36)
//!
//! Provides an automated security advisory agent that:
//!
//! 1. **Auto-generates fix patches** for known vulnerabilities based on
//!    advisory data (CVE → version bump, config change, code fix).
//!
//! 2. **Push notifications** via WebSocket for real-time security alerts.
//!
//! 3. **Daily security digest** — aggregated report of all security events
//!    over the last 24 hours, pushed to configured channels.
//!
//! # Architecture
//!
//! ```text
//! Vulnerability Scanner / Secret Detector / Permit Analyzer
//!      │                           │
//!      ▼                           ▼
//! SecurityAdvisorAgent
//!      ├── auto_generate_fix(vuln) → FixPatch
//!      ├── notify_ws(alert) → WebSocket push
//!      ├── build_daily_digest() → SecurityDigest
//!      └── digest_schedule (tokio interval)
//! ```

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::security::vulnerability_scan::{
    DependencyScanResult, SecretScanResult, Severity, Vulnerability,
};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum SecurityAdvisorError {
    #[error("fix generation failed for {0}: {1}")]
    FixGenerationFailed(String, String),

    #[error("WebSocket push failed: {0}")]
    WsPushFailed(String),

    #[error("digest build failed: {0}")]
    DigestBuildFailed(String),

    #[error("no fix available for advisory {0}")]
    NoFixAvailable(String),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A generated fix patch for a vulnerability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixPatch {
    /// The advisory ID this patch addresses.
    pub advisory_id: String,
    /// The package / file to patch.
    pub target: String,
    /// Type of fix.
    pub fix_type: FixType,
    /// The diff / patch content (unified diff format).
    pub patch_content: String,
    /// Description of what the patch does.
    pub description: String,
    /// Whether the patch is verified to compile / apply cleanly.
    pub verified: bool,
    /// Recommended action for the user.
    pub recommended_action: String,
    /// Confidence that this fix is correct (0.0 – 1.0).
    pub confidence: f64,
}

/// Type of fix generated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FixType {
    /// Bump a dependency version in Cargo.toml / package.json etc.
    VersionBump,
    /// Change a configuration value.
    ConfigChange,
    /// Replace an insecure code pattern.
    CodeRefactor,
    /// Update file permissions.
    PermissionFix,
    /// Remove or rotate a hardcoded secret.
    SecretRemoval,
    /// Add or update a security control.
    SecurityControl,
    /// Other type of fix.
    Other(String),
}

/// A security alert pushed via WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityAlert {
    /// Unique alert ID.
    pub id: String,
    /// Severity of the alert.
    pub severity: Severity,
    /// Title / summary of the alert.
    pub title: String,
    /// Detailed description.
    pub description: String,
    /// Source of the alert (vulnerability scan, secret detection, permit scan).
    pub source: AlertSource,
    /// Timestamp when the alert was generated.
    pub timestamp: SystemTime,
    /// Whether the alert has been acknowledged.
    pub acknowledged: bool,
    /// Related advisory ID (if applicable).
    pub advisory_id: Option<String>,
    /// Suggested fix patch (if available).
    pub suggested_fix: Option<FixPatch>,
    /// Affected component or file path.
    pub affected_component: Option<String>,
}

/// Source of a security alert.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AlertSource {
    DependencyVulnerability,
    SecretExposure,
    PermitExposure,
    SecurityAdvisor,
    UserReported,
}

/// Daily security digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityDigest {
    /// Date of the digest (ISO 8601 date string).
    pub date: String,
    /// Total number of alerts generated today.
    pub total_alerts: usize,
    /// Number of critical alerts.
    pub critical_count: usize,
    /// Number of high severity alerts.
    pub high_count: usize,
    /// Number of medium severity alerts.
    pub medium_count: usize,
    /// Number of low severity alerts.
    pub low_count: usize,
    /// Summary of dependency scan results (latest scan recorded by
    /// `alert_from_dependency_scan`).
    pub dependency_summary: Option<DependencyScanResult>,
    /// Summary of secret scan results (latest scan recorded by
    /// `alert_from_secret_scan`).
    pub secret_summary: Option<SecretScanResult>,
    /// All alerts generated today.
    pub alerts: Vec<SecurityAlert>,
    /// Top recommendations.
    pub recommendations: Vec<String>,
    /// Number of auto-generated fix patches.
    pub patches_generated: usize,
    /// Number of patches applied automatically.
    pub patches_applied: usize,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the SecurityAdvisorAgent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityAdvisorConfig {
    /// Whether auto-fix is enabled.
    pub auto_fix_enabled: bool,
    /// Whether to push WS alerts.
    pub ws_alerts_enabled: bool,
    /// Whether daily digest is enabled.
    pub digest_enabled: bool,
    /// Interval in seconds for daily digest (default: 86400 = 24h).
    pub digest_interval_secs: u64,
    /// Minimum severity to auto-generate a fix.
    pub min_fix_severity: Severity,
    /// WebHook URL for digest delivery (optional).
    pub digest_webhook_url: Option<String>,
    /// WebSocket endpoint for push alerts (optional).
    pub ws_endpoint: Option<String>,
    /// Path to store digest history.
    pub digest_history_path: Option<String>,
}

impl Default for SecurityAdvisorConfig {
    fn default() -> Self {
        Self {
            auto_fix_enabled: true,
            ws_alerts_enabled: true,
            digest_enabled: true,
            digest_interval_secs: 86400, // 24 hours
            min_fix_severity: Severity::Medium,
            digest_webhook_url: None,
            ws_endpoint: None,
            digest_history_path: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Security Advisor Agent
// ---------------------------------------------------------------------------

/// Autonomous security advisory agent that monitors scan results, generates
/// fix patches, pushes WebSocket alerts, and produces daily digests.
#[derive(Debug)]
pub struct SecurityAdvisorAgent {
    /// Configuration.
    config: SecurityAdvisorConfig,
    /// In-memory alert buffer (for building the daily digest).
    alerts: Arc<Mutex<Vec<SecurityAlert>>>,
    /// Latest dependency scan result (populated by `alert_from_dependency_scan`,
    /// surfaced in the daily digest's `dependency_summary`).
    last_dependency_scan: Arc<Mutex<Option<DependencyScanResult>>>,
    /// Latest secret scan result (populated by `alert_from_secret_scan`,
    /// surfaced in the daily digest's `secret_summary`).
    last_secret_scan: Arc<Mutex<Option<SecretScanResult>>>,
    /// Count of auto-generated patches.
    patches_generated: AtomicU64,
    /// Count of auto-applied patches.
    patches_applied: AtomicU64,
    /// Registered WebSocket senders for push alerts.
    ws_senders: Arc<Mutex<Vec<tokio::sync::mpsc::Sender<SecurityAlert>>>>,
    /// Timestamp of the last digest.
    last_digest_time: Arc<Mutex<Option<SystemTime>>>,
}

impl SecurityAdvisorAgent {
    /// Create a new advisor agent with the given configuration.
    pub fn new(config: SecurityAdvisorConfig) -> Self {
        Self {
            config,
            alerts: Arc::new(Mutex::new(Vec::new())),
            last_dependency_scan: Arc::new(Mutex::new(None)),
            last_secret_scan: Arc::new(Mutex::new(None)),
            patches_generated: AtomicU64::new(0),
            patches_applied: AtomicU64::new(0),
            ws_senders: Arc::new(Mutex::new(Vec::new())),
            last_digest_time: Arc::new(Mutex::new(None)),
        }
    }

    /// Return the current configuration.
    pub fn config(&self) -> &SecurityAdvisorConfig {
        &self.config
    }

    // ── Fix patch generation ────────────────────────────────────────────

    /// Auto-generate a fix patch for a given vulnerability.
    ///
    /// The fix type is inferred from the vulnerability metadata:
    /// - If `patched_version` is set → `VersionBump`
    /// - If the advisory mentions a config change → `ConfigChange`
    /// - Otherwise → `CodeRefactor` with a general recommendation.
    pub async fn auto_generate_fix(
        &self,
        vulnerability: &Vulnerability,
    ) -> Result<FixPatch, SecurityAdvisorError> {
        if !self.config.auto_fix_enabled {
            return Err(SecurityAdvisorError::InvalidConfig(
                "auto-fix is disabled in configuration".into(),
            ));
        }

        if vulnerability.severity < self.config.min_fix_severity {
            return Err(SecurityAdvisorError::NoFixAvailable(format!(
                "severity {:?} below minimum {:?}",
                vulnerability.severity, self.config.min_fix_severity
            )));
        }

        info!(
            "auto_generate_fix: advisory={}, package={}",
            vulnerability.advisory_id, vulnerability.package
        );

        // Determine fix type based on available data.
        let (fix_type, patch_content, description, recommended_action) = if let Some(ref patched) =
            vulnerability.patched_version
        {
            (
                FixType::VersionBump,
                format!(
                    "--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -1,3 +1,3 @@\n-{pkg} = \"{affected}\"\n+{pkg} = \"{patched}\"\n",
                    pkg = vulnerability.package,
                    affected = vulnerability.affected_versions.split(',').next().unwrap_or("?").trim(),
                    patched = patched,
                ),
                format!(
                    "Upgrade {} from {} to {}",
                    vulnerability.package,
                    vulnerability.affected_versions,
                    patched,
                ),
                format!(
                    "Run: cargo update -p {}@{}",
                    vulnerability.package,
                    patched,
                ),
            )
        } else if vulnerability.advisory_url.is_some() {
            (
                FixType::SecurityControl,
                format!(
                    r#"# Advisory: {adv} – {desc}
# See: {url}
#
# Recommended mitigation steps:
# 1. Review the advisory details at the URL above.
# 2. Check if a patched version has been released.
# 3. If no patch exists, apply compensating controls:
#    - Restrict network access to the affected component.
#    - Disable unused features that expose the vulnerability.
#    - Add WAF rules to filter exploit payloads.
# 4. Monitor for new advisories and patch when available.
"#,
                    adv = vulnerability.advisory_id,
                    desc = vulnerability.description,
                    url = vulnerability.advisory_url.as_deref().unwrap_or("N/A"),
                ),
                format!(
                    "Apply mitigation for {}: {}",
                    vulnerability.advisory_id, vulnerability.description,
                ),
                format!(
                    "Review advisory {} and apply recommended mitigation.",
                    vulnerability.advisory_id,
                ),
            )
        } else {
            (
                FixType::CodeRefactor,
                format!(
                    r#"# Manual fix required for {pkg} ({adv})
#
# Problem: {desc}
#
# Action plan:
# 1. Identify the vulnerable code paths in {pkg}.
# 2. Apply input validation, output encoding, or dependency cleanup.
# 3. Run `cargo audit` to confirm the finding is addressed.
# 4. Add a regression test covering the vulnerability scenario.
"#,
                    pkg = vulnerability.package,
                    adv = vulnerability.advisory_id,
                    desc = vulnerability.description,
                ),
                format!(
                    "Code refactor needed for {}: {}",
                    vulnerability.package, vulnerability.description,
                ),
                format!(
                    "Manually review {} for the vulnerability described in {}.",
                    vulnerability.package, vulnerability.advisory_id,
                ),
            )
        };

        // Confidence heuristic: version bumps are high confidence.
        let confidence = match &fix_type {
            FixType::VersionBump => 0.95,
            FixType::ConfigChange => 0.80,
            FixType::SecurityControl => 0.70,
            _ => 0.50,
        };

        let patch = FixPatch {
            advisory_id: vulnerability.advisory_id.clone(),
            target: vulnerability.package.clone(),
            fix_type,
            patch_content,
            description,
            verified: false,
            recommended_action,
            confidence,
        };

        self.patches_generated.fetch_add(1, Ordering::Relaxed);

        Ok(patch)
    }

    /// Mark a patch as applied (increments the applied counter).
    pub async fn record_patch_applied(&self) {
        self.patches_applied.fetch_add(1, Ordering::Relaxed);
    }

    // ── WebSocket alert push ────────────────────────────────────────────

    /// Register a WebSocket sender for push alerts.
    pub async fn register_ws_sender(&self, sender: tokio::sync::mpsc::Sender<SecurityAlert>) {
        let mut senders = self.ws_senders.lock().await;
        senders.push(sender);
        info!(
            "SecurityAdvisorAgent: WebSocket sender registered (total: {})",
            senders.len()
        );
    }

    /// Push a security alert to all registered WebSocket senders.
    pub async fn notify_ws(&self, alert: SecurityAlert) -> Result<(), SecurityAdvisorError> {
        if !self.config.ws_alerts_enabled {
            return Ok(());
        }

        let mut senders = self.ws_senders.lock().await;
        senders.retain(|sender| sender.try_send(alert.clone()).is_ok());

        // Also store the alert for the digest. The buffer is bounded: a
        // long-running server with daily scans would otherwise accumulate
        // every alert for the process lifetime and each digest would
        // serialize the whole history.
        const MAX_ALERTS_BUFFER: usize = 1000;
        {
            let mut alerts = self.alerts.lock().await;
            alerts.push(alert);
            if alerts.len() > MAX_ALERTS_BUFFER {
                let overflow = alerts.len() - MAX_ALERTS_BUFFER;
                alerts.drain(..overflow);
                info!(
                    "SecurityAdvisorAgent: alert buffer capped at {MAX_ALERTS_BUFFER}, dropped {overflow} oldest"
                );
            }
        }

        info!(
            "SecurityAdvisorAgent: alert pushed to {} sender(s)",
            senders.len()
        );
        Ok(())
    }

    /// Create a security alert from a dependency scan result and push it.
    ///
    /// Also records the scan result so the daily digest can surface a real
    /// `dependency_summary` (previously the digest field was always `None`).
    pub async fn alert_from_dependency_scan(
        &self,
        scan_result: &DependencyScanResult,
    ) -> Result<(), SecurityAdvisorError> {
        *self.last_dependency_scan.lock().await = Some(scan_result.clone());

        // Fix generation is independent per vulnerability — generate all fixes
        // concurrently (the daily background task previously serialized them).
        // Individual failures degrade to `None` (no fix) instead of aborting
        // the whole batch.
        let fixes: Vec<Option<FixPatch>> = if self.config.auto_fix_enabled {
            futures_util::future::join_all(
                scan_result
                    .vulnerabilities
                    .iter()
                    .map(|vuln| self.auto_generate_fix(vuln)),
            )
            .await
            .into_iter()
            .map(|result| result.ok())
            .collect()
        } else {
            vec![None; scan_result.vulnerabilities.len()]
        };

        // WebSocket pushes stay sequential so alert ordering matches the scan.
        for (vuln, fix) in scan_result.vulnerabilities.iter().zip(fixes) {
            let alert = SecurityAlert {
                id: format!("dep-{}", vuln.advisory_id),
                severity: vuln.severity.clone(),
                title: format!("Dependency vulnerability: {}", vuln.package),
                description: vuln.description.clone(),
                source: AlertSource::DependencyVulnerability,
                timestamp: SystemTime::now(),
                acknowledged: false,
                advisory_id: Some(vuln.advisory_id.clone()),
                suggested_fix: fix,
                affected_component: Some(vuln.package.clone()),
            };

            self.notify_ws(alert).await?;
        }
        Ok(())
    }

    /// Create a security alert from a secret scan result and push it.
    ///
    /// Also records the scan result so the daily digest can surface a real
    /// `secret_summary` (previously the digest field was always `None`).
    pub async fn alert_from_secret_scan(
        &self,
        scan_result: &SecretScanResult,
    ) -> Result<(), SecurityAdvisorError> {
        *self.last_secret_scan.lock().await = Some(scan_result.clone());

        for secret_match in &scan_result.matches {
            if secret_match.risk < crate::security::vulnerability_scan::SecretRisk::High {
                continue;
            }

            let alert = SecurityAlert {
                id: format!("secret-{}", uuid::Uuid::new_v4()),
                severity: match secret_match.risk {
                    crate::security::vulnerability_scan::SecretRisk::Critical => Severity::Critical,
                    crate::security::vulnerability_scan::SecretRisk::High => Severity::High,
                    _ => Severity::Medium,
                },
                title: format!("Secret exposure: {}", secret_match.pattern_name),
                description: format!(
                    "A {} was detected in {} at line {}.",
                    secret_match.pattern_name, secret_match.file_path, secret_match.line
                ),
                source: AlertSource::SecretExposure,
                timestamp: SystemTime::now(),
                acknowledged: false,
                advisory_id: None,
                suggested_fix: None,
                affected_component: Some(secret_match.file_path.clone()),
            };

            self.notify_ws(alert).await?;
        }
        Ok(())
    }

    // ── Daily digest ───────────────────────────────────────────────────

    /// Build the daily security digest from accumulated alerts.
    ///
    /// The digest includes counts by severity, all alerts grouped by source,
    /// and top recommendations.
    pub async fn build_daily_digest(&self) -> Result<SecurityDigest, SecurityAdvisorError> {
        let alerts = self.alerts.lock().await;

        let mut critical = 0usize;
        let mut high = 0usize;
        let mut medium = 0usize;
        let mut low = 0usize;

        for alert in alerts.iter() {
            match alert.severity {
                Severity::Critical => critical += 1,
                Severity::High => high += 1,
                Severity::Medium => medium += 1,
                Severity::Low => low += 1,
                Severity::Unknown => low += 1,
            }
        }

        let now = iso_date_today();

        let recommendations = self.generate_recommendations(&alerts);

        let patches_gen = self.patches_generated.load(Ordering::Relaxed) as usize;
        let patches_app = self.patches_applied.load(Ordering::Relaxed) as usize;

        // Surface the latest scan results (recorded by
        // alert_from_dependency_scan / alert_from_secret_scan) instead of
        // always leaving the summaries as None.
        let dependency_summary = self.last_dependency_scan.lock().await.clone();
        let secret_summary = self.last_secret_scan.lock().await.clone();

        let digest = SecurityDigest {
            date: now,
            total_alerts: alerts.len(),
            critical_count: critical,
            high_count: high,
            medium_count: medium,
            low_count: low,
            dependency_summary,
            secret_summary,
            alerts: alerts.clone(),
            recommendations,
            patches_generated: patches_gen,
            patches_applied: patches_app,
        };

        info!(
            "SecurityAdvisorAgent: daily digest built — {} alerts ({} critical, {} high)",
            digest.total_alerts, digest.critical_count, digest.high_count
        );

        Ok(digest)
    }

    /// Generate recommendations based on the current alert set.
    fn generate_recommendations(&self, alerts: &[SecurityAlert]) -> Vec<String> {
        let mut recs = Vec::new();

        // Count critical+high by source.
        let vuln_count = alerts
            .iter()
            .filter(|a| {
                a.source == AlertSource::DependencyVulnerability
                    && (a.severity == Severity::Critical || a.severity == Severity::High)
            })
            .count();

        let secret_count = alerts
            .iter()
            .filter(|a| {
                a.source == AlertSource::SecretExposure
                    && (a.severity == Severity::Critical || a.severity == Severity::High)
            })
            .count();

        let permit_count = alerts
            .iter()
            .filter(|a| {
                a.source == AlertSource::PermitExposure
                    && (a.severity == Severity::Critical || a.severity == Severity::High)
            })
            .count();

        if vuln_count > 0 {
            recs.push(format!(
                "Address {} critical/high dependency vulnerabilities by running `cargo audit` and applying the suggested version bumps.",
                vuln_count
            ));
        }

        if secret_count > 0 {
            recs.push(format!(
                "Rotate {} exposed secrets immediately and remove hardcoded credentials from the codebase.",
                secret_count
            ));
        }

        if permit_count > 0 {
            recs.push(format!(
                "Fix {} permission issues by restricting file modes to 0644/0755.",
                permit_count
            ));
        }

        if recs.is_empty() {
            recs.push("No critical or high severity issues found. Continue monitoring.".into());
        }

        recs
    }

    // ── Digest scheduling ───────────────────────────────────────────────

    /// Start the daily digest schedule in the background.
    ///
    /// This spawns a tokio task that produces a digest every
    /// `config.digest_interval_secs` and pushes it to the configured
    /// webhook URL (if set).
    pub fn start_digest_schedule(self: &Arc<Self>) {
        if !self.config.digest_enabled {
            info!("SecurityAdvisorAgent: daily digest is disabled");
            return;
        }

        let interval = Duration::from_secs(self.config.digest_interval_secs);
        let advisor = Arc::clone(self);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // Skip the first immediate tick; wait for the full interval.
            ticker.tick().await;

            loop {
                ticker.tick().await;
                match advisor.build_daily_digest().await {
                    Ok(digest) => {
                        info!(
                            "SecurityAdvisorAgent: daily digest ready ({} alerts)",
                            digest.total_alerts
                        );

                        // Push to webhook if configured.
                        if let Some(ref url) = advisor.config.digest_webhook_url {
                            if let Err(e) = Self::push_digest_to_webhook(url, &digest).await {
                                warn!(
                                    "SecurityAdvisorAgent: failed to push digest to webhook: {}",
                                    e
                                );
                            }
                        }

                        // Store last digest time.
                        let mut last = advisor.last_digest_time.lock().await;
                        *last = Some(SystemTime::now());
                    }
                    Err(e) => {
                        warn!("SecurityAdvisorAgent: digest build failed: {}", e);
                    }
                }
            }
        });
    }

    /// Push the digest to a configured webhook URL.
    async fn push_digest_to_webhook(
        url: &str,
        digest: &SecurityDigest,
    ) -> Result<(), SecurityAdvisorError> {
        let body = serde_json::to_value(digest)
            .map_err(|e| SecurityAdvisorError::DigestBuildFailed(e.to_string()))?;

        let client = crate::shared::http_client::http_client()
            .map_err(|e| SecurityAdvisorError::DigestBuildFailed(e.to_string()))?;
        let resp = client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|e| SecurityAdvisorError::DigestBuildFailed(e.to_string()))?;

        if !resp.status().is_success() {
            warn!("SecurityAdvisorAgent: webhook returned {}", resp.status());
        }

        info!("SecurityAdvisorAgent: digest pushed to webhook");
        Ok(())
    }

    // ── Stats ───────────────────────────────────────────────────────────

    /// Return the number of accumulated alerts.
    pub async fn alert_count(&self) -> usize {
        self.alerts.lock().await.len()
    }

    /// Return the number of auto-generated patches.
    pub async fn patches_generated_count(&self) -> usize {
        self.patches_generated.load(Ordering::Relaxed) as usize
    }

    /// Return the number of auto-applied patches.
    pub async fn patches_applied_count(&self) -> usize {
        self.patches_applied.load(Ordering::Relaxed) as usize
    }

    /// Return the time of the last digest.
    pub async fn last_digest_time(&self) -> Option<SystemTime> {
        *self.last_digest_time.lock().await
    }
}

// ── Helper: ISO-8601 date ─────────────────────────────────────────────

/// Convert a Unix timestamp (seconds) to a (year, month, day) tuple in the
/// proleptic Gregorian calendar.
///
/// **Single canonical epoch→date conversion** for the security, memory and
/// governance layers (previously three independent implementations lived in
/// `security_advisor.rs` (Hinnant), `memory_persistence.rs` (day-loop) and
/// `hardening.rs` (integer division)). Uses floor division so pre-1970
/// timestamps map to the correct civil date.
pub(crate) fn unix_ts_to_ymd(secs: i64) -> (i64, i64, i64) {
    days_to_date(secs.div_euclid(86_400))
}

/// Convert days since the Unix epoch (1970-01-01) to a (year, month, day)
/// tuple. Algorithm adapted from Howard Hinnant's public-domain date
/// algorithms (`civil_from_days`); leap years handled exactly.
fn days_to_date(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Convert a Unix timestamp (seconds) to the day number since the epoch
/// (floor-divided). Used to detect calendar-day changes for daily resets
/// (e.g. `governance::hardening::TenantBudgetEnforcer`).
pub(crate) fn unix_ts_day_number(secs: i64) -> i64 {
    secs.div_euclid(86_400)
}

/// Produce an ISO 8601 date string for the current day.
fn iso_date_today() -> String {
    let secs = crate::shared::timestamps::now_ts();
    let (y, m, d) = unix_ts_to_ymd(secs);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::vulnerability_scan::{Severity, Vulnerability};

    /// Boundary coverage for the canonical epoch→date conversion: century
    /// leap year (2000), a normal year, leap-day 2024-02-29, and month/year
    /// boundaries. Guards against the pre-unification day-loop implementation
    /// disagreeing at month boundaries.
    #[test]
    fn test_unix_ts_to_ymd_known_dates() {
        assert_eq!(unix_ts_to_ymd(0), (1970, 1, 1));
        assert_eq!(unix_ts_to_ymd(86_400), (1970, 1, 2));
        assert_eq!(unix_ts_to_ymd(946_684_800), (2000, 1, 1));
        assert_eq!(unix_ts_to_ymd(1_704_067_200), (2024, 1, 1));
        // 2024 is a leap year: Feb 29 exists.
        assert_eq!(unix_ts_to_ymd(1_709_164_800), (2024, 2, 29));
        assert_eq!(unix_ts_to_ymd(1_709_251_200), (2024, 3, 1));
        assert_eq!(unix_ts_to_ymd(1_735_689_600), (2025, 1, 1));
        // Last second of a month stays in that month.
        assert_eq!(unix_ts_to_ymd(1_709_251_199), (2024, 2, 29));
        // Pre-epoch timestamps map to the correct civil date (floor division).
        assert_eq!(unix_ts_to_ymd(-86_400), (1969, 12, 31));
    }

    #[test]
    fn test_fix_patch_serialize_roundtrip() {
        let patch = FixPatch {
            advisory_id: "RUSTSEC-2024-0001".into(),
            target: "tokio".into(),
            fix_type: FixType::VersionBump,
            patch_content: "--- a/Cargo.toml\n+++ b/Cargo.toml".into(),
            description: "Upgrade tokio".into(),
            verified: false,
            recommended_action: "cargo update".into(),
            confidence: 0.95,
        };
        let json = serde_json::to_string(&patch).unwrap();
        let deserialized: FixPatch = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.advisory_id, "RUSTSEC-2024-0001");
        assert!((deserialized.confidence - 0.95).abs() < 1e-6);
    }

    #[test]
    fn test_security_alert_serialize_roundtrip() {
        let alert = SecurityAlert {
            id: "alert-1".into(),
            severity: Severity::Critical,
            title: "Critical vuln".into(),
            description: "Description".into(),
            source: AlertSource::DependencyVulnerability,
            timestamp: SystemTime::now(),
            acknowledged: false,
            advisory_id: Some("RUSTSEC-2024-0001".into()),
            suggested_fix: None,
            affected_component: Some("tokio".into()),
        };
        let json = serde_json::to_string(&alert).unwrap();
        let deserialized: SecurityAlert = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "alert-1");
        assert_eq!(deserialized.severity, Severity::Critical);
    }

    #[test]
    fn test_security_digest_serialize_roundtrip() {
        let digest = SecurityDigest {
            date: "2026-06-01".into(),
            total_alerts: 5,
            critical_count: 1,
            high_count: 2,
            medium_count: 1,
            low_count: 1,
            dependency_summary: None,
            secret_summary: None,
            alerts: Vec::new(),
            recommendations: vec!["Fix all issues".into()],
            patches_generated: 3,
            patches_applied: 1,
        };
        let json = serde_json::to_string(&digest).unwrap();
        let deserialized: SecurityDigest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.total_alerts, 5);
        assert_eq!(deserialized.patches_generated, 3);
    }

    #[tokio::test]
    async fn test_auto_generate_fix_version_bump() {
        let advisor = SecurityAdvisorAgent::new(SecurityAdvisorConfig {
            auto_fix_enabled: true,
            min_fix_severity: Severity::Low,
            ..Default::default()
        });

        let vuln = Vulnerability {
            advisory_id: "RUSTSEC-2024-0001".into(),
            package: "tokio".into(),
            affected_versions: "< 1.35.0".into(),
            patched_version: Some("1.35.0".into()),
            severity: Severity::High,
            description: "HTTP/2 rapid reset".into(),
            advisory_url: Some("https://rustsec.org".into()),
            cvss_score: Some(7.5),
        };

        let patch = advisor.auto_generate_fix(&vuln).await.unwrap();
        assert_eq!(patch.fix_type, FixType::VersionBump);
        assert!(patch.confidence > 0.9);
        assert_eq!(patch.target, "tokio");
    }

    #[tokio::test]
    async fn test_auto_generate_fix_disabled() {
        let advisor = SecurityAdvisorAgent::new(SecurityAdvisorConfig {
            auto_fix_enabled: false,
            ..Default::default()
        });

        let vuln = Vulnerability {
            advisory_id: "RUSTSEC-2024-0001".into(),
            package: "tokio".into(),
            affected_versions: "< 1.35".into(),
            patched_version: Some("1.35".into()),
            severity: Severity::High,
            description: "test".into(),
            advisory_url: None,
            cvss_score: None,
        };

        let result = advisor.auto_generate_fix(&vuln).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_auto_generate_fix_below_min_severity() {
        let advisor = SecurityAdvisorAgent::new(SecurityAdvisorConfig {
            auto_fix_enabled: true,
            min_fix_severity: Severity::Critical,
            ..Default::default()
        });

        let vuln = Vulnerability {
            advisory_id: "RUSTSEC-2024-0002".into(),
            package: "serde".into(),
            affected_versions: "< 1.0".into(),
            patched_version: None,
            severity: Severity::Low,
            description: "minor".into(),
            advisory_url: None,
            cvss_score: None,
        };

        let result = advisor.auto_generate_fix(&vuln).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_notify_ws_registers_sender() {
        let advisor = SecurityAdvisorAgent::new(SecurityAdvisorConfig::default());
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);

        advisor.register_ws_sender(tx).await;

        let alert = SecurityAlert {
            id: "test".into(),
            severity: Severity::High,
            title: "Test Alert".into(),
            description: "Testing WS push".into(),
            source: AlertSource::SecurityAdvisor,
            timestamp: SystemTime::now(),
            acknowledged: false,
            advisory_id: None,
            suggested_fix: None,
            affected_component: None,
        };

        advisor.notify_ws(alert.clone()).await.unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received.id, "test");
    }

    #[tokio::test]
    async fn test_digest_build_empty() {
        let advisor = SecurityAdvisorAgent::new(SecurityAdvisorConfig::default());
        let digest = advisor.build_daily_digest().await.unwrap();
        assert_eq!(digest.total_alerts, 0);
        assert!(digest.recommendations.contains(
            &"No critical or high severity issues found. Continue monitoring.".to_string()
        ));
    }

    #[tokio::test]
    async fn test_digest_build_with_alerts() {
        let advisor = SecurityAdvisorAgent::new(SecurityAdvisorConfig::default());

        // Inject an alert via notify_ws.
        let alert = SecurityAlert {
            id: "critical-1".into(),
            severity: Severity::Critical,
            title: "Critical Issue".into(),
            description: "Something bad".into(),
            source: AlertSource::DependencyVulnerability,
            timestamp: SystemTime::now(),
            acknowledged: false,
            advisory_id: None,
            suggested_fix: None,
            affected_component: None,
        };

        // Register a discard sender so notify_ws doesn't fail.
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        advisor.register_ws_sender(tx).await;
        advisor.notify_ws(alert).await.unwrap();

        let digest = advisor.build_daily_digest().await.unwrap();
        assert_eq!(digest.total_alerts, 1);
        assert_eq!(digest.critical_count, 1);
    }

    #[test]
    fn test_days_to_date() {
        // Unix epoch: 1970-01-01
        let (y, m, d) = days_to_date(0);
        assert_eq!((y, m, d), (1970, 1, 1));

        // 2026-01-01 -> days since epoch = 20454 (rough)
        let (y, m, d) = days_to_date(20454);
        assert_eq!((y, m, d), (2026, 1, 1));
    }

    #[test]
    fn test_recommendations_generated() {
        let advisor = SecurityAdvisorAgent::new(SecurityAdvisorConfig::default());
        let alerts = vec![
            SecurityAlert {
                id: "v1".into(),
                severity: Severity::Critical,
                title: "Critical dep vuln".into(),
                description: "desc".into(),
                source: AlertSource::DependencyVulnerability,
                timestamp: SystemTime::now(),
                acknowledged: false,
                advisory_id: None,
                suggested_fix: None,
                affected_component: None,
            },
            SecurityAlert {
                id: "s1".into(),
                severity: Severity::High,
                title: "Secret found".into(),
                description: "desc".into(),
                source: AlertSource::SecretExposure,
                timestamp: SystemTime::now(),
                acknowledged: false,
                advisory_id: None,
                suggested_fix: None,
                affected_component: None,
            },
        ];

        let recs = advisor.generate_recommendations(&alerts);
        assert!(
            recs.iter()
                .any(|r| r.contains("dependency vulnerabilities")),
            "no dep vuln rec in {:?}",
            recs
        );
        assert!(
            recs.iter().any(|r| r.contains("exposed secrets")),
            "no secret rec in {:?}",
            recs
        );
    }

    #[test]
    fn test_patches_counters() {
        let advisor = SecurityAdvisorAgent::new(SecurityAdvisorConfig::default());
        advisor.patches_generated.store(5, Ordering::Relaxed);
        advisor.patches_applied.store(3, Ordering::Relaxed);

        assert_eq!(advisor.patches_generated.load(Ordering::Relaxed), 5);
        assert_eq!(advisor.patches_applied.load(Ordering::Relaxed), 3);
    }
}
