//! AlertManager — real-time alerts based on system metrics thresholds
//!
//! GAP-B49-10: Predefined alert rules that fire webhooks when breached.
//! Supports deduplication (5-minute cooldown between same alert).

// F-GAP-49: Module now wired into production observability pipeline.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tracing::warn;

pub use crate::shared::alert_severity::AlertSeverity;

/// An alert event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    /// Alert rule name
    pub rule: String,
    /// Severity
    pub severity: AlertSeverity,
    /// Human-readable message
    pub message: String,
    /// Current metric value
    pub value: f64,
    /// Threshold value
    pub threshold: f64,
    /// When the alert was created
    pub timestamp: i64,
}

/// An alert rule definition
#[derive(Debug, Clone)]
pub struct AlertRule {
    pub name: &'static str,
    pub description: &'static str,
    pub severity: AlertSeverity,
    /// Check function takes (metric_value, threshold) and returns true if alert should fire
    pub check: fn(f64, f64) -> bool,
    pub threshold: f64,
    /// Cooldown in seconds to prevent alert storms
    pub cooldown_seconds: u64,
}

/// Predefined alert rules
pub fn default_alert_rules() -> Vec<AlertRule> {
    vec![
        AlertRule {
            name: "p95_latency_high",
            description: "P95 request latency exceeds 5 seconds",
            severity: AlertSeverity::Warning,
            check: |value, threshold| value > threshold,
            threshold: 5000.0,
            cooldown_seconds: 300, // 5 min
        },
        AlertRule {
            name: "circuit_breaker_open",
            description: "More than 3 circuit breakers are open",
            severity: AlertSeverity::Critical,
            check: |value, threshold| value > threshold,
            threshold: 3.0,
            cooldown_seconds: 60,
        },
        AlertRule {
            name: "error_rate_high",
            description: "Request error rate exceeds 5%",
            severity: AlertSeverity::Warning,
            check: |value, threshold| value > threshold,
            threshold: 5.0,
            cooldown_seconds: 300,
        },
        AlertRule {
            name: "cache_hit_ratio_low",
            description: "Cache hit ratio below 50%",
            severity: AlertSeverity::Info,
            check: |value, threshold| value < threshold,
            threshold: 50.0,
            cooldown_seconds: 600,
        },
        AlertRule {
            name: "agent_timeout_rate",
            description: "Agent timeout rate exceeds 10%",
            severity: AlertSeverity::Warning,
            check: |value, threshold| value > threshold,
            threshold: 10.0,
            cooldown_seconds: 300,
        },
        // ── O6: Memory health rules ─────────────────────────────────────
        // Thresholds are the single source of truth in memory_health (MB).
        AlertRule {
            name: "memory_critical",
            description: "Free memory below critical threshold",
            severity: AlertSeverity::Critical,
            check: |value, threshold| value < threshold,
            threshold: crate::observability::memory_health::MEMORY_CRITICAL_MB as f64,
            cooldown_seconds: 60,
        },
        AlertRule {
            name: "memory_low",
            description: "Free memory below warning threshold",
            severity: AlertSeverity::Warning,
            check: |value, threshold| value < threshold,
            threshold: crate::observability::memory_health::MEMORY_WARN_MB as f64,
            cooldown_seconds: 120,
        },
        AlertRule {
            name: "memory_jetsam_risk",
            description: "Free memory below jetsam risk threshold",
            severity: AlertSeverity::Critical,
            check: |value, threshold| value < threshold,
            threshold: crate::observability::memory_health::MEMORY_JETSAM_RISK_MB as f64,
            cooldown_seconds: 30,
        },
    ]
}

/// Webhook configuration for alert notification
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebhookConfig {
    pub url: String,
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub enabled: bool,
    /// Timeout in milliseconds for the webhook request.
    pub timeout_ms: u64,
}

/// AlertManager — manages alert rules and fires notifications
#[derive(Debug)]
pub struct AlertManager {
    rules: Vec<AlertRule>,
    /// Tracks last fire time per rule for deduplication
    last_fire: HashMap<String, Instant>,
    webhook: WebhookConfig,
    total_alerts_fired: u64,
    /// Ring buffer of recent alerts (max 100)
    recent_alerts: VecDeque<Alert>,
}

/// Maximum number of recent alerts to retain in the ring buffer.
const MAX_RECENT_ALERTS: usize = 100;

impl AlertManager {
    pub fn new(rules: Vec<AlertRule>) -> Self {
        Self {
            rules,
            last_fire: HashMap::new(),
            webhook: WebhookConfig::default(),
            total_alerts_fired: 0,
            recent_alerts: VecDeque::with_capacity(MAX_RECENT_ALERTS),
        }
    }

    /// Evaluate all alert rules against current metrics
    ///
    /// Only evaluates rules that are semantically relevant to the given metric name.
    /// A rule is relevant if its name shares a keyword prefix with the metric name
    /// (e.g. "memory_free_mb" matches "memory_critical"/"memory_low"/"memory_jetsam_risk").
    /// Rules with "fallback" in name or generic names match all metrics.
    pub fn evaluate(&mut self, metric_name: &str, value: f64) -> Vec<Alert> {
        let mut fired = Vec::new();
        let now = Instant::now();

        for rule in &self.rules {
            // Skip rules whose name doesn't semantically match the metric name.
            // This prevents false positives (e.g. circuit_breaker_open matching memory_free_mb).
            if !rule_matches_metric(rule.name, metric_name) {
                continue;
            }
            let threshold = rule.threshold;
            if (rule.check)(value, threshold) {
                let cooldown = Duration::from_secs(rule.cooldown_seconds);
                let should_fire = self
                    .last_fire
                    .get(rule.name)
                    .is_none_or(|last| now.duration_since(*last) >= cooldown);

                if should_fire {
                    let alert = Alert {
                        rule: rule.name.to_string(),
                        severity: rule.severity,
                        message: format!(
                            "{}: {} = {:.2} (threshold: {:.2})",
                            rule.description, metric_name, value, threshold
                        ),
                        value,
                        threshold,
                        timestamp: crate::shared::timestamps::now_ts(),
                    };

                    self.last_fire.insert(rule.name.to_string(), now);
                    self.total_alerts_fired += 1;

                    // Push to ring buffer, pop oldest if at capacity
                    self.recent_alerts.push_front(alert.clone());
                    while self.recent_alerts.len() > MAX_RECENT_ALERTS {
                        self.recent_alerts.pop_back();
                    }

                    if self.webhook.enabled && !self.webhook.url.is_empty() {
                        self.fire_webhook(&alert);
                    }

                    fired.push(alert);
                }
            }
        }

        fired
    }

    /// Record a direct alert event (no rule matching).
    ///
    /// Security-advisor findings are discrete events (a specific dependency
    /// vulnerability, a leaked secret) rather than continuous metrics, so
    /// routing them through `evaluate` would let `rule_matches_metric` skip
    /// them (no rule name contains the `security.*` metric prefix).
    /// Cooldown is not applied: each finding is a distinct event.
    pub fn report_direct(&mut self, rule_name: &str, message: String, severity: AlertSeverity) {
        let now = Instant::now();
        let alert = Alert {
            rule: rule_name.to_string(),
            severity,
            message,
            value: 0.0,
            threshold: 0.0,
            timestamp: crate::shared::timestamps::now_ts(),
        };
        self.last_fire.insert(rule_name.to_string(), now);
        self.total_alerts_fired += 1;
        self.recent_alerts.push_front(alert.clone());
        while self.recent_alerts.len() > MAX_RECENT_ALERTS {
            self.recent_alerts.pop_back();
        }
        if self.webhook.enabled && !self.webhook.url.is_empty() {
            self.fire_webhook(&alert);
        }
    }

    /// Configure webhook from environment variables.
    /// Reads `GO_ON_ALERT_WEBHOOK_URL`, `GO_ON_ALERT_WEBHOOK_ENABLED`,
    /// and `GO_ON_ALERT_WEBHOOK_TIMEOUT`.
    pub fn configure_from_env(&mut self) -> &mut Self {
        if let Ok(url) = std::env::var("GO_ON_ALERT_WEBHOOK_URL") {
            if !url.is_empty() {
                let enabled = std::env::var("GO_ON_ALERT_WEBHOOK_ENABLED")
                    .map(|v| v == "1" || v.to_lowercase() == "true")
                    .unwrap_or(true);
                let config = WebhookConfig {
                    url,
                    enabled,
                    headers: HashMap::new(),
                    timeout_ms: std::env::var("GO_ON_ALERT_WEBHOOK_TIMEOUT")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(5000),
                };
                self.webhook = config;
                tracing::info!("Alert webhook configured via environment");
            }
        }
        self
    }

    /// Set webhook configuration
    pub fn set_webhook(&mut self, config: WebhookConfig) {
        self.webhook = config;
    }

    /// Get recent alerts (most recent first)
    pub fn get_recent_alerts(&self) -> Vec<Alert> {
        self.recent_alerts.iter().cloned().collect()
    }

    /// Fire webhook notification (non-blocking, logs errors)
    fn fire_webhook(&self, alert: &Alert) {
        let span = tracing::info_span!(
            "alert_webhook_fire",
            rule = %alert.rule,
            severity = %alert.severity,
            url = %self.webhook.url,
        );
        let _guard = span.enter();

        let url = self.webhook.url.clone();
        let timeout_ms = self.webhook.timeout_ms;
        let recent_alerts_count = self.recent_alerts.len();
        let payload = serde_json::json!({
            "alert": alert,
            "recent_alerts_count": recent_alerts_count
        });
        // Capture the current tracing span so it propagates across the async
        // boundary; the webhook send runs inside `span.in_scope` below.
        let span = tracing::Span::current();
        // Spawn a background task to send the webhook
        tokio::spawn(async move {
            // Enter the span around the whole webhook send (the previous
            // `in_scope(|| {})` empty closure was a no-op that dropped the
            // span before the request was built).
            let send = async {
                match crate::shared::http_client::http_client() {
                    Ok(client) => {
                        let mut request = client.post(&url).json(&payload);
                        // Apply the configured webhook timeout (previously the
                        // timeout_ms field was write-only — requests could hang).
                        if timeout_ms > 0 {
                            request = request.timeout(std::time::Duration::from_millis(timeout_ms));
                        }
                        if let Err(e) = request.send().await {
                            warn!("AlertManager webhook send failed: {e}");
                        }
                    }
                    Err(e) => warn!("AlertManager http_client unavailable: {e}"),
                }
            };
            span.in_scope(|| send).await;
        });
    }

    /// Get alert statistics
    pub fn stats(&self) -> AlertManagerStats {
        AlertManagerStats {
            total_rules: self.rules.len() as u64,
            total_alerts_fired: self.total_alerts_fired,
            active_webhook: self.webhook.enabled,
        }
    }
}

/// Check if a rule name is semantically relevant to a metric name.
///
/// Extracts the first segment of the metric name (e.g. "memory" from "memory_free_mb")
/// and checks if that keyword appears in the rule name. For generic metric names that
/// don't match any known prefix, all rules are evaluated (safe fallback).
fn rule_matches_metric(rule_name: &str, metric_name: &str) -> bool {
    // Extract the first keyword segment from the metric name
    let metric_keyword = metric_name.split('_').next().unwrap_or(metric_name);
    // If the metric has no recognizable keyword prefix, evaluate all rules
    if metric_keyword.is_empty() || metric_keyword.len() <= 2 {
        return true;
    }
    // Check if the keyword appears in the rule name
    rule_name.contains(metric_keyword)
}

/// Statistics for the AlertManager
#[derive(Debug, Clone, Serialize)]
pub struct AlertManagerStats {
    pub total_rules: u64,
    pub total_alerts_fired: u64,
    pub active_webhook: bool,
}

/// Global alert manager instance — single authoritative AlertManager shared by
/// the memory monitor (write side) and the server consumers (read side).
static ALERT_MANAGER: std::sync::OnceLock<Arc<StdMutex<AlertManager>>> = std::sync::OnceLock::new();

/// Get or initialize the global AlertManager.
pub fn alert_manager() -> &'static StdMutex<AlertManager> {
    ALERT_MANAGER
        .get_or_init(|| Arc::new(StdMutex::new(AlertManager::new(default_alert_rules()))))
        .as_ref()
}

/// Get an `Arc` handle to the process-global AlertManager.
///
/// `ServerBuilder::build` uses this so memory-monitor writes and server-side
/// alert consumption hit the same instance (previously two separate instances
/// meant memory alerts were written to a manager nobody read).
pub fn shared_alert_manager() -> Arc<StdMutex<AlertManager>> {
    ALERT_MANAGER
        .get_or_init(|| Arc::new(StdMutex::new(AlertManager::new(default_alert_rules()))))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alert_fires_when_threshold_exceeded() {
        let rules = vec![AlertRule {
            name: "test_rule",
            description: "Test alert",
            severity: AlertSeverity::Warning,
            check: |v, t| v > t,
            threshold: 10.0,
            cooldown_seconds: 1,
        }];
        let mut mgr = AlertManager::new(rules);
        let alerts = mgr.evaluate("test_metric", 15.0);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].rule, "test_rule");
    }

    #[test]
    fn test_alert_does_not_fire_below_threshold() {
        let rules = vec![AlertRule {
            name: "test_rule",
            description: "Test alert",
            severity: AlertSeverity::Warning,
            check: |v, t| v > t,
            threshold: 10.0,
            cooldown_seconds: 1,
        }];
        let mut mgr = AlertManager::new(rules);
        let alerts = mgr.evaluate("test_metric", 5.0);
        assert_eq!(alerts.len(), 0);
    }

    #[test]
    fn test_alert_deduplication() {
        let rules = vec![AlertRule {
            name: "test_rule",
            description: "Test alert",
            severity: AlertSeverity::Warning,
            check: |v, t| v > t,
            threshold: 10.0,
            cooldown_seconds: 3600, // 1 hour cooldown
        }];
        let mut mgr = AlertManager::new(rules);
        let first = mgr.evaluate("test", 15.0);
        assert_eq!(first.len(), 1);
        let second = mgr.evaluate("test", 20.0); // Still above threshold
        assert_eq!(second.len(), 0); // Deduplicated
    }

    #[test]
    fn test_default_rules_exist() {
        let rules = default_alert_rules();
        assert!(rules.len() >= 3);
        assert!(rules.iter().any(|r| r.name == "p95_latency_high"));
        assert!(rules.iter().any(|r| r.name == "error_rate_high"));
    }

    #[test]
    fn test_stats() {
        let rules = vec![AlertRule {
            name: "test",
            description: "test",
            severity: AlertSeverity::Info,
            check: |v, t| v > t,
            threshold: 1.0,
            cooldown_seconds: 1,
        }];
        let mut mgr = AlertManager::new(rules);
        mgr.evaluate("m", 2.0);
        let stats = mgr.stats();
        assert_eq!(stats.total_rules, 1);
        assert_eq!(stats.total_alerts_fired, 1);
    }
}
