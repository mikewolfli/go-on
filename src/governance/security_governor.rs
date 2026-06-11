//! Security Governor — policy audit gate extension for BLUE38 F-GAP-14.
//!
//! Provides capability extension for security policy enforcement beyond the
//! existing `harness_bus.rs`.  Supports audit gate policies (allow / deny /
//! review before execution), capability-based access control, policy
//! composition (AND / OR combinators), and security level escalation.
//!
//! Architecture
//! ------------
//! The [`SecurityGovernor`] wraps interior state in `Arc<Mutex<…>>` so it can
//! be shared across threads and embedded inside a [`CapabilityBus`] or
//! [`HarnessBus`] just like the other governance sub-components.
//!
//! Usage
//! -----
//! ```rust,ignore
//! let config = SecurityGovernorConfig::default();
//! let governor = SecurityGovernor::new(config);
//! governor.register_policy(my_policy);
//! let verdict = governor.evaluate("resource:file://x", "actor:alice", &json!({}))?;
//! println!("{:?}", verdict);
//! ```

/// Maximum number of audit entries retained in memory to prevent unbounded growth.
const MAX_AUDIT_ENTRIES: usize = 10_000;

use crate::i18n::{t, tf};
use anyhow::Result;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Policy severity levels
// ---------------------------------------------------------------------------

/// Severity of a security policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum PolicySeverity {
    Low,
    #[default]
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for PolicySeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

// ---------------------------------------------------------------------------
// Policy actions
// ---------------------------------------------------------------------------

/// Action to take when a policy's conditions match a request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PolicyAction {
    /// Allow the request unconditionally.
    Allow,
    /// Deny the request.
    Deny,
    /// Request requires manual / automated review before execution.
    RequireReview,
    /// Escalate the security level of the request.
    Escalate,
}

impl std::fmt::Display for PolicyAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allow => write!(f, "allow"),
            Self::Deny => write!(f, "deny"),
            Self::RequireReview => write!(f, "require_review"),
            Self::Escalate => write!(f, "escalate"),
        }
    }
}

// ---------------------------------------------------------------------------
// Policy conditions
// ---------------------------------------------------------------------------

/// Comparison operator for a policy condition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConditionOperator {
    Equals,
    NotEquals,
    Contains,
    StartsWith,
    EndsWith,
    Matches, // regex match
    In,
    NotIn,
}

impl std::fmt::Display for ConditionOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Equals => write!(f, "eq"),
            Self::NotEquals => write!(f, "ne"),
            Self::Contains => write!(f, "contains"),
            Self::StartsWith => write!(f, "starts_with"),
            Self::EndsWith => write!(f, "ends_with"),
            Self::Matches => write!(f, "matches"),
            Self::In => write!(f, "in"),
            Self::NotIn => write!(f, "not_in"),
        }
    }
}

/// A single condition that contributes to a policy match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyCondition {
    /// The field name to evaluate (e.g. "resource", "actor", "context.method").
    pub field: String,
    /// Comparison operator.
    pub operator: ConditionOperator,
    /// Value to compare against.
    pub value: String,
}

impl PolicyCondition {
    /// Evaluate this condition against the given resource, actor, and context.
    ///
    /// `context` is a flat map of string keys to string values (e.g. serialised
    /// JSON fields flattened for simple matching).
    pub fn evaluate(&self, resource: &str, actor: &str, context: &HashMap<String, String>) -> bool {
        let field_value = match self.field.as_str() {
            "resource" => Some(resource),
            "actor" => Some(actor),
            other => context.get(other).map(|s| s.as_str()),
        };

        let Some(haystack) = field_value else {
            return false;
        };

        match self.operator {
            ConditionOperator::Equals => haystack == self.value,
            ConditionOperator::NotEquals => haystack != self.value,
            ConditionOperator::Contains => haystack.contains(&self.value),
            ConditionOperator::StartsWith => haystack.starts_with(&self.value),
            ConditionOperator::EndsWith => haystack.ends_with(&self.value),
            ConditionOperator::Matches => regex_match(haystack, &self.value),
            ConditionOperator::In => self.value.split(',').any(|v| v.trim() == haystack),
            ConditionOperator::NotIn => self.value.split(',').all(|v| v.trim() != haystack),
        }
    }
}

/// Simple regex-or-glob matcher (supports `*` glob wildcard and literal regex).
fn regex_match(haystack: &str, pattern: &str) -> bool {
    // If the pattern contains only `*` as a wildcard, treat it as a simple glob.
    if pattern.contains('*') {
        let re_pattern = format!("^{}$", regex::escape(pattern).replace(r"\*", ".*"));
        if let Ok(re) = regex::Regex::new(&re_pattern) {
            return re.is_match(haystack);
        }
    }
    // Otherwise try literal regex.
    if let Ok(re) = regex::Regex::new(pattern) {
        return re.is_match(haystack);
    }
    // Fall back to exact match.
    haystack == pattern
}

// ---------------------------------------------------------------------------
// Policy composition
// ---------------------------------------------------------------------------

/// How multiple conditions are combined.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum PolicyComposition {
    /// All conditions must match.
    #[default]
    And,
    /// Any one condition must match.
    Or,
}

// ---------------------------------------------------------------------------
// SecurityPolicy
// ---------------------------------------------------------------------------

/// A security policy definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityPolicy {
    /// Unique policy identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of what this policy enforces.
    pub description: String,
    /// Severity level.
    pub severity: PolicySeverity,
    /// Action to take when this policy matches.
    pub action: PolicyAction,
    /// Conditions that trigger this policy.
    pub conditions: Vec<PolicyCondition>,
    /// How conditions are combined.
    pub composition: PolicyComposition,
    /// Optional escalation level hint (used when action is `Escalate`).
    pub escalation_level: Option<String>,
}

impl SecurityPolicy {
    /// Evaluate whether this policy matches the given request attributes.
    pub fn matches(&self, resource: &str, actor: &str, context: &HashMap<String, String>) -> bool {
        if self.conditions.is_empty() {
            return false; // no conditions → no match
        }
        match self.composition {
            PolicyComposition::And => self
                .conditions
                .iter()
                .all(|c| c.evaluate(resource, actor, context)),
            PolicyComposition::Or => self
                .conditions
                .iter()
                .any(|c| c.evaluate(resource, actor, context)),
        }
    }
}

// ---------------------------------------------------------------------------
// PolicyVerdict
// ---------------------------------------------------------------------------

/// Result of a policy evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyVerdict {
    /// Whether the request is allowed.
    pub allowed: bool,
    /// Whether the request requires review.
    pub required_review: bool,
    /// The escalation level if action was `Escalate`.
    pub escalation_level: String,
    /// The matched policy ID (if any).
    pub matched_policy: Option<String>,
    /// Human-readable reasons for the verdict.
    pub reasons: Vec<String>,
}

impl PolicyVerdict {
    /// Create an "allow" verdict.
    pub fn allow() -> Self {
        Self {
            allowed: true,
            required_review: false,
            escalation_level: "normal".into(),
            matched_policy: None,
            reasons: vec![t("error.security_governor.allowed_default")],
        }
    }

    /// Create a "deny" verdict.
    pub fn deny(policy: &str, reason: &str) -> Self {
        Self {
            allowed: false,
            required_review: false,
            escalation_level: "normal".into(),
            matched_policy: Some(policy.into()),
            reasons: vec![
                tf(
                    "error.security_governor.denied_by_policy",
                    &[("name", policy)],
                ),
                reason.into(),
            ],
        }
    }

    /// Create a "require review" verdict.
    pub fn require_review(policy: &str, reason: &str) -> Self {
        Self {
            allowed: true,
            required_review: true,
            escalation_level: "normal".into(),
            matched_policy: Some(policy.into()),
            reasons: vec![
                tf(
                    "error.security_governor.review_required",
                    &[("name", policy)],
                ),
                reason.into(),
            ],
        }
    }

    /// Create an "escalate" verdict.
    pub fn escalate(policy: &str, level: &str, reason: &str) -> Self {
        Self {
            allowed: true,
            required_review: false,
            escalation_level: level.into(),
            matched_policy: Some(policy.into()),
            reasons: vec![
                tf(
                    "error.security_governor.escalated_by_policy",
                    &[("name", policy)],
                ),
                reason.into(),
            ],
        }
    }
}

// ---------------------------------------------------------------------------
// AuditEntry
// ---------------------------------------------------------------------------

/// A recorded audit log entry for security policy evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unix timestamp (seconds since epoch).
    pub timestamp: i64,
    /// The policy ID that was evaluated.
    pub policy_id: String,
    /// The verdict reached.
    pub verdict: PolicyVerdict,
    /// The resource being accessed.
    pub resource: String,
    /// The actor / principal making the request.
    pub actor: String,
    /// Additional detail / context.
    pub detail: String,
}

impl AuditEntry {
    /// Create a new audit entry.
    pub fn new(
        policy_id: String,
        verdict: PolicyVerdict,
        resource: String,
        actor: String,
        detail: String,
    ) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        Self {
            timestamp,
            policy_id,
            verdict,
            resource,
            actor,
            detail,
        }
    }
}

// ---------------------------------------------------------------------------
// SecurityGovernorConfig
// ---------------------------------------------------------------------------

/// Configuration for the security governor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityGovernorConfig {
    /// Whether the governor is enabled.
    pub enabled: bool,
    /// Default action when no policy matches.
    pub default_action: PolicyAction,
    /// Policy mode: "enforce", "advisory", or empty (default).
    /// Controls how governance policies are applied during request processing.
    pub policy_mode: String,
}

impl Default for SecurityGovernorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_action: PolicyAction::Allow,
            policy_mode: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// GovernorProfile
// ---------------------------------------------------------------------------

/// Runtime metrics snapshot for the [`SecurityGovernor`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernorProfile {
    /// Whether the governor is enabled.
    pub enabled: bool,
    /// Number of registered policies.
    pub policies_count: u64,
    /// Total evaluations performed.
    pub total_evaluations: u64,
    /// Total denied requests.
    pub total_denials: u64,
    /// Total requests sent to review.
    pub total_reviews: u64,
    /// Number of active escalations.
    pub active_escalations: u64,
}

// ---------------------------------------------------------------------------
// Internal shared state
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Inner {
    config: SecurityGovernorConfig,
    policies: IndexMap<String, SecurityPolicy>,
    // Audit log.
    audit_log: Vec<AuditEntry>,
    // Metrics counters.
    total_evaluations: u64,
    total_denials: u64,
    total_reviews: u64,
    active_escalations: u64,
}

// ---------------------------------------------------------------------------
// SecurityGovernor
// ---------------------------------------------------------------------------

/// Security policy governor — audit gate extension for BLUE38 F-GAP-14.
///
/// Wraps interior state in `Arc<Mutex<…>>` for thread-safe sharing.
#[derive(Debug, Clone)]
pub struct SecurityGovernor {
    inner: Arc<Mutex<Inner>>,
}

impl SecurityGovernor {
    /// Create a new [`SecurityGovernor`] with the given configuration.
    pub fn new(config: SecurityGovernorConfig) -> Self {
        let inner = Inner {
            config,
            policies: IndexMap::new(),
            audit_log: Vec::new(),
            total_evaluations: 0,
            total_denials: 0,
            total_reviews: 0,
            active_escalations: 0,
        };
        let governor = Self {
            inner: Arc::new(Mutex::new(inner)),
        };
        governor.register_default_policies();
        governor
    }

    /// Register default security policies.
    ///
    /// Registers the built-in set of policies that provide baseline protection:
    ///
    /// - `deny-unknown-resource` — catch-all deny when no other policy matches.
    /// - `deny-sensitive-data` — blocks resources containing secrets/passwords.
    /// - `require-review-admin-actions` — requires review for admin/delete actions.
    pub fn register_default_policies(&self) {
        // Catch-all: registered with empty conditions so it never matches via
        // normal first-match iteration. Instead, `evaluate()` treats it as a
        // fallback when no policy matches and the default action is Allow,
        // ensuring unknown resources are denied rather than allowed by default.
        self.register_policy(SecurityPolicy {
            id: "deny-unknown-resource".into(),
            name: "Deny Unknown Resource".into(),
            description: "Denies access when no specific policy matches".into(),
            severity: PolicySeverity::High,
            action: PolicyAction::Deny,
            conditions: vec![],
            composition: PolicyComposition::And,
            escalation_level: None,
        });

        // Deny access to resources containing sensitive keywords.
        self.register_policy(SecurityPolicy {
            id: "deny-sensitive-data".into(),
            name: "Deny Sensitive Data".into(),
            description:
                "Blocks access to resources containing secret, password, credential, or token"
                    .into(),
            severity: PolicySeverity::High,
            action: PolicyAction::Deny,
            conditions: vec![
                PolicyCondition {
                    field: "resource".into(),
                    operator: ConditionOperator::Contains,
                    value: "secret".into(),
                },
                PolicyCondition {
                    field: "resource".into(),
                    operator: ConditionOperator::Contains,
                    value: "password".into(),
                },
                PolicyCondition {
                    field: "resource".into(),
                    operator: ConditionOperator::Contains,
                    value: "credential".into(),
                },
                PolicyCondition {
                    field: "resource".into(),
                    operator: ConditionOperator::Contains,
                    value: "token".into(),
                },
            ],
            composition: PolicyComposition::Or,
            escalation_level: None,
        });

        // Require review for admin or delete actions.
        self.register_policy(SecurityPolicy {
            id: "require-review-admin-actions".into(),
            name: "Require Review for Admin Actions".into(),
            description: "Requires review for actions containing admin or delete".into(),
            severity: PolicySeverity::Medium,
            action: PolicyAction::RequireReview,
            conditions: vec![
                PolicyCondition {
                    field: "action".into(),
                    operator: ConditionOperator::Contains,
                    value: "admin".into(),
                },
                PolicyCondition {
                    field: "action".into(),
                    operator: ConditionOperator::Contains,
                    value: "delete".into(),
                },
            ],
            composition: PolicyComposition::Or,
            escalation_level: None,
        });
    }

    /// Register a new security policy.
    ///
    /// If a policy with the same `id` already exists, it will be **replaced**
    /// and the old policy is returned.
    pub fn register_policy(&self, policy: SecurityPolicy) -> Option<SecurityPolicy> {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("SecurityGovernor lock poisoned in register_policy, recovering");
            poisoned.into_inner()
        });
        inner.policies.insert(policy.id.clone(), policy)
    }

    /// Remove a policy by its ID.
    ///
    /// Returns `true` if the policy existed and was removed.
    pub fn remove_policy(&self, id: &str) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("SecurityGovernor lock poisoned in remove_policy, recovering");
            poisoned.into_inner()
        });
        inner.policies.shift_remove(id).is_some()
    }

    /// Evaluate all registered policies against the given request.
    ///
    /// Policies are evaluated in insertion order (first registered wins, i.e.
    /// **first-match** semantics).  If no policy matches, the configured
    /// `default_action` is applied.
    ///
    /// `context` is a flat string-keyed map of contextual attributes taken
    /// from (e.g.) serialised JSON fields of the request envelope.
    pub fn evaluate(
        &self,
        resource: &str,
        actor: &str,
        context: &HashMap<String, String>,
    ) -> Result<PolicyVerdict> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("SecurityGovernor lock poisoned: {}", e))?;

        inner.total_evaluations += 1;

        // If disabled, always allow.
        if !inner.config.enabled {
            return Ok(PolicyVerdict::allow());
        }

        // Check policy mode: "advisory" logs but does not enforce.
        let policy_mode = inner.config.policy_mode.clone();
        if policy_mode == "advisory" {
            // In advisory mode, run evaluation but always allow (for dry-run/testing).
            let advisory_result = self.do_evaluate(&mut inner, resource, actor, context)?;
            if !advisory_result.allowed || advisory_result.required_review {
                tracing::warn!(
                    target: "security_governor",
                    policy_mode = "advisory",
                    resource = %resource,
                    actor = %actor,
                    would_deny = !advisory_result.allowed,
                    would_review = advisory_result.required_review,
                    "Advisory mode: policy violation detected but not enforced"
                );
            }
            return Ok(PolicyVerdict::allow());
        }

        // Enforce mode (or empty/default): evaluate and return the actual verdict.
        self.do_evaluate(&mut inner, resource, actor, context)
    }

    /// Core evaluation logic shared by enforce and advisory modes.
    /// Runs policy first-match then default-action fallback, mutating counters
    /// on `inner` as side-effects.
    fn do_evaluate(
        &self,
        inner: &mut Inner,
        resource: &str,
        actor: &str,
        context: &HashMap<String, String>,
    ) -> Result<PolicyVerdict> {
        // First-match: iterate over all policies and return on first match.
        // Collect matched policy data first to avoid borrow conflicts.
        let matched: Option<(String, String, PolicyAction, Option<String>)> = inner
            .policies
            .values()
            .find(|p| p.matches(resource, actor, context))
            .map(|p| {
                (
                    p.id.clone(),
                    p.name.clone(),
                    p.action.clone(),
                    p.escalation_level.clone(),
                )
            });

        if let Some((id, name, action, escalation_level)) = matched {
            return Ok(match action {
                PolicyAction::Allow => PolicyVerdict {
                    allowed: true,
                    required_review: false,
                    escalation_level: "normal".into(),
                    matched_policy: Some(id),
                    reasons: vec![tf(
                        "error.security_governor.allowed_by_policy",
                        &[("name", &name)],
                    )],
                },
                PolicyAction::Deny => {
                    inner.total_denials += 1;
                    PolicyVerdict {
                        allowed: false,
                        required_review: false,
                        escalation_level: "normal".into(),
                        matched_policy: Some(id),
                        reasons: vec![tf(
                            "error.security_governor.denied_by_policy",
                            &[("name", &name)],
                        )],
                    }
                }
                PolicyAction::RequireReview => {
                    inner.total_reviews += 1;
                    PolicyVerdict {
                        allowed: true,
                        required_review: true,
                        escalation_level: "normal".into(),
                        matched_policy: Some(id),
                        reasons: vec![tf(
                            "error.security_governor.review_required",
                            &[("name", &name)],
                        )],
                    }
                }
                PolicyAction::Escalate => {
                    let level = escalation_level.unwrap_or_else(|| "elevated".into());
                    inner.active_escalations += 1;
                    PolicyVerdict {
                        allowed: true,
                        required_review: false,
                        escalation_level: level,
                        matched_policy: Some(id),
                        reasons: vec![tf(
                            "error.security_governor.escalated_by_policy",
                            &[("name", &name)],
                        )],
                    }
                }
            });
        }

        // No policy matched — apply default action.
        // If the catch-all "deny-unknown-resource" policy is registered and the
        // configured default is Allow, override to Deny as a safety net.
        let use_catch_all = inner.policies.contains_key("deny-unknown-resource")
            && inner.config.default_action == PolicyAction::Allow;

        Ok(match inner.config.default_action {
            PolicyAction::Allow if use_catch_all => {
                inner.total_denials += 1;
                PolicyVerdict {
                    allowed: false,
                    required_review: false,
                    escalation_level: "normal".into(),
                    matched_policy: Some("deny-unknown-resource".into()),
                    reasons: vec![t("error.security_governor.no_match_denied")],
                }
            }
            PolicyAction::Allow => PolicyVerdict {
                allowed: true,
                required_review: false,
                escalation_level: "normal".into(),
                matched_policy: None,
                reasons: vec![t("error.security_governor.no_match_allowed")],
            },
            PolicyAction::Deny => {
                inner.total_denials += 1;
                PolicyVerdict {
                    allowed: false,
                    required_review: false,
                    escalation_level: "normal".into(),
                    matched_policy: None,
                    reasons: vec![t("error.security_governor.no_match_denied")],
                }
            }
            PolicyAction::RequireReview => {
                inner.total_reviews += 1;
                PolicyVerdict {
                    allowed: true,
                    required_review: true,
                    escalation_level: "normal".into(),
                    matched_policy: None,
                    reasons: vec![t("error.security_governor.no_match_review")],
                }
            }
            PolicyAction::Escalate => {
                inner.active_escalations += 1;
                PolicyVerdict {
                    allowed: true,
                    required_review: false,
                    escalation_level: "elevated".into(),
                    matched_policy: None,
                    reasons: vec![t("error.security_governor.no_match_escalated")],
                }
            }
        })
    }

    /// Record an audit log entry: appends to the internal audit log and
    /// updates governance metric counters (evaluations, denials, reviews).
    pub fn record_audit(&self, entry: AuditEntry) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("SecurityGovernor lock poisoned in record_audit, recovering");
            poisoned.into_inner()
        });
        inner.total_evaluations += 1;
        if !entry.verdict.allowed {
            inner.total_denials += 1;
        }
        if entry.verdict.required_review {
            inner.total_reviews += 1;
        }
        if !entry.verdict.escalation_level.is_empty() {
            inner.active_escalations += 1;
        }
        inner.audit_log.push(entry);
        if inner.audit_log.len() > MAX_AUDIT_ENTRIES {
            inner.audit_log.remove(0);
        }
    }

    /// Return all recorded audit log entries.
    pub fn audit_log(&self) -> Vec<AuditEntry> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("SecurityGovernor lock poisoned in audit_log, recovering");
                poisoned.into_inner()
            })
            .audit_log
            .clone()
    }

    /// Clear the internal audit log.
    pub fn clear_audit(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("SecurityGovernor lock poisoned in clear_audit, recovering");
            poisoned.into_inner()
        });
        inner.audit_log.clear();
    }

    /// Return the policy mode configured for this governor.
    pub fn policy_mode(&self) -> String {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("SecurityGovernor lock poisoned in policy_mode, recovering");
                poisoned.into_inner()
            })
            .config
            .policy_mode
            .clone()
    }

    /// Return a [`GovernorProfile`] snapshot of current metrics.
    pub fn profile(&self) -> GovernorProfile {
        let inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("SecurityGovernor lock poisoned in profile, recovering");
            poisoned.into_inner()
        });
        GovernorProfile {
            enabled: inner.config.enabled,
            policies_count: inner.policies.len() as u64,
            total_evaluations: inner.total_evaluations,
            total_denials: inner.total_denials,
            total_reviews: inner.total_reviews,
            active_escalations: inner.active_escalations,
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_allow_policy(id: &str, field: &str, value: &str) -> SecurityPolicy {
        SecurityPolicy {
            id: id.into(),
            name: format!("Allow {}", id),
            description: format!("Allow policy for {}={}", field, value),
            severity: PolicySeverity::Low,
            action: PolicyAction::Allow,
            conditions: vec![PolicyCondition {
                field: field.into(),
                operator: ConditionOperator::Equals,
                value: value.into(),
            }],
            composition: PolicyComposition::And,
            escalation_level: None,
        }
    }

    fn make_deny_policy(id: &str, field: &str, value: &str) -> SecurityPolicy {
        SecurityPolicy {
            id: id.into(),
            name: format!("Deny {}", id),
            description: format!("Deny policy for {}={}", field, value),
            severity: PolicySeverity::High,
            action: PolicyAction::Deny,
            conditions: vec![PolicyCondition {
                field: field.into(),
                operator: ConditionOperator::Equals,
                value: value.into(),
            }],
            composition: PolicyComposition::And,
            escalation_level: None,
        }
    }

    fn make_review_policy(id: &str, field: &str, value: &str) -> SecurityPolicy {
        SecurityPolicy {
            id: id.into(),
            name: format!("Review {}", id),
            description: format!("Review required for {}={}", field, value),
            severity: PolicySeverity::Medium,
            action: PolicyAction::RequireReview,
            conditions: vec![PolicyCondition {
                field: field.into(),
                operator: ConditionOperator::Equals,
                value: value.into(),
            }],
            composition: PolicyComposition::And,
            escalation_level: None,
        }
    }

    fn make_escalate_policy(id: &str, field: &str, value: &str, level: &str) -> SecurityPolicy {
        SecurityPolicy {
            id: id.into(),
            name: format!("Escalate {}", id),
            description: format!("Escalate for {}={}", field, value),
            severity: PolicySeverity::Critical,
            action: PolicyAction::Escalate,
            conditions: vec![PolicyCondition {
                field: field.into(),
                operator: ConditionOperator::Equals,
                value: value.into(),
            }],
            composition: PolicyComposition::And,
            escalation_level: Some(level.into()),
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// 1. A new governor with default config uses the deny-all catch-all policy
    ///    when no policy matches, denying unknown resources.
    #[test]
    fn test_new_governor_default_action() {
        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());
        let verdict = governor
            .evaluate("resource:x", "actor:y", &HashMap::new())
            .expect("evaluate should succeed");
        assert!(!verdict.allowed, "catch-all should deny unknown resources");
        assert!(!verdict.required_review);
        assert_eq!(verdict.escalation_level, "normal");
        assert_eq!(verdict.matched_policy, Some("deny-unknown-resource".into()));
        assert_eq!(verdict.reasons.len(), 1);
    }

    /// 2. Register an allow policy and verify it matches.
    #[test]
    fn test_register_and_evaluate_allow() {
        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());
        let policy = make_allow_policy("p1", "resource", "db://prod/orders");
        governor.register_policy(policy);

        let verdict = governor
            .evaluate("db://prod/orders", "actor:bob", &HashMap::new())
            .expect("evaluate should succeed");
        assert!(verdict.allowed);
        assert!(!verdict.required_review);
        assert_eq!(verdict.matched_policy, Some("p1".into()));
    }

    /// 3. Register a deny policy and verify denial.
    #[test]
    fn test_register_and_evaluate_deny() {
        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());
        let policy = make_deny_policy("p2", "actor", "role:guest");
        governor.register_policy(policy);

        let verdict = governor
            .evaluate("resource:file", "role:guest", &HashMap::new())
            .expect("evaluate should succeed");
        assert!(!verdict.allowed, "guest should be denied");
        assert_eq!(verdict.matched_policy, Some("p2".into()));
    }

    /// 4. Register a require-review policy.
    #[test]
    fn test_register_and_evaluate_require_review() {
        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());
        let policy = make_review_policy("p3", "resource", "admin/delete-user");
        governor.register_policy(policy);

        let verdict = governor
            .evaluate("admin/delete-user", "actor:admin", &HashMap::new())
            .expect("evaluate should succeed");
        assert!(verdict.allowed);
        assert!(
            verdict.required_review,
            "admin/ access should require review"
        );
        assert_eq!(verdict.matched_policy, Some("p3".into()));
    }

    /// 5. Multiple policies: first-match semantics (order matters).
    #[test]
    fn test_multiple_policies_first_match() {
        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());

        // First register a deny for actor "evil".
        governor.register_policy(make_deny_policy("deny-evil", "actor", "evil"));
        // Then register an allow for actor "evil" (should never be reached).
        governor.register_policy(make_allow_policy("allow-evil", "actor", "evil"));

        let verdict = governor
            .evaluate("resource:any", "evil", &HashMap::new())
            .expect("evaluate should succeed");
        assert!(
            !verdict.allowed,
            "first registered policy (deny) should match first"
        );
        assert_eq!(verdict.matched_policy, Some("deny-evil".into()));
    }

    /// 6. When no policy matches, the default action is used.
    #[test]
    fn test_policy_not_matched_uses_default() {
        let config = SecurityGovernorConfig {
            enabled: true,
            default_action: PolicyAction::Deny,
            ..Default::default()
        };
        let governor = SecurityGovernor::new(config);
        let policy = make_deny_policy("p-deny", "resource", "secret");
        governor.register_policy(policy);

        // Request that doesn't match "resource=secret".
        let verdict = governor
            .evaluate("public", "anyone", &HashMap::new())
            .expect("evaluate should succeed");
        assert!(!verdict.allowed, "default action is deny");
        assert!(verdict.matched_policy.is_none());
    }

    /// 7. Recording and retrieving audit entries.
    /// 7. Record and retrieve audit log entries.
    #[test]
    fn test_record_audit() {
        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());
        let entry = AuditEntry::new(
            "p-audit".into(),
            PolicyVerdict::deny("p-audit", "test denial"),
            "resource:db".into(),
            "actor:alice".into(),
            "integration test".into(),
        );
        governor.record_audit(entry);

        let log = governor.audit_log();
        assert_eq!(log.len(), 1, "audit entry should be stored");
        assert_eq!(log[0].policy_id, "p-audit");
    }

    /// 8. Audit log cap.
    #[test]
    fn test_audit_log_capped() {
        let config = SecurityGovernorConfig {
            enabled: true,
            default_action: PolicyAction::Allow,
            ..Default::default()
        };
        let governor = SecurityGovernor::new(config);

        for i in 0..10 {
            let entry = AuditEntry::new(
                format!("p-{}", i),
                PolicyVerdict::allow(),
                "r".into(),
                "a".into(),
                format!("entry {}", i),
            );
            governor.record_audit(entry);
        }

        let log = governor.audit_log();
        assert_eq!(log.len(), 10, "audit entries should be stored");
    }

    /// 9. Removing a policy.
    #[test]
    fn test_remove_policy() {
        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());
        let policy = make_allow_policy("removable", "resource", "anything");
        governor.register_policy(policy);

        // Policy exists.
        let pre = governor
            .evaluate("anything", "x", &HashMap::new())
            .expect("evaluate");
        assert!(pre.allowed);
        assert_eq!(pre.matched_policy, Some("removable".into()));

        // Remove it.
        let removed = governor.remove_policy("removable");
        assert!(removed, "policy should have been removed");

        // Now it should fall through to the catch-all.
        let post = governor
            .evaluate("anything", "x", &HashMap::new())
            .expect("evaluate");
        assert!(!post.allowed, "catch-all should deny unknown resources");
        assert_eq!(post.matched_policy, Some("deny-unknown-resource".into()));
    }

    /// 10. Profile reflects internal state.
    #[test]
    fn test_profile_reflects_state() {
        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());

        // Register two policies.
        governor.register_policy(make_allow_policy("a1", "resource", "x"));
        governor.register_policy(make_deny_policy("d1", "resource", "y"));

        let profile = governor.profile();
        assert!(profile.enabled);
        assert_eq!(profile.policies_count, 5);
        assert_eq!(profile.total_evaluations, 0);
        assert_eq!(profile.total_denials, 0);
        assert_eq!(profile.total_reviews, 0);

        // Perform evaluations.
        governor.evaluate("x", "u", &HashMap::new()).ok();
        governor.evaluate("y", "u", &HashMap::new()).ok();

        let profile = governor.profile();
        assert_eq!(profile.total_evaluations, 2);
        assert_eq!(profile.total_denials, 1);
        // Note: the evaluations match user-registered policies first, not the
        // catch-all, so the catch-all does not increment denials here.
        assert_eq!(profile.total_reviews, 0);

        // Remove a policy (3 defaults + 2 registered - 1 removed = 4 remaining).
        governor.remove_policy("a1");
        let profile = governor.profile();
        assert_eq!(profile.policies_count, 4);
    }

    // -----------------------------------------------------------------------
    // Additional edge-case tests
    // -----------------------------------------------------------------------

    /// 11. Escalation action sets the escalation level.
    #[test]
    fn test_escalate_action() {
        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());
        let policy = make_escalate_policy("esc1", "actor", "sensitive-role", "critical");
        governor.register_policy(policy);

        let verdict = governor
            .evaluate("r", "sensitive-role", &HashMap::new())
            .expect("evaluate");
        assert!(verdict.allowed);
        assert_eq!(verdict.escalation_level, "critical");
        assert_eq!(verdict.matched_policy, Some("esc1".into()));
    }

    /// 12. Disabled governor always allows regardless of policies.
    #[test]
    fn test_disabled_governor_always_allows() {
        let config = SecurityGovernorConfig {
            enabled: false,
            default_action: PolicyAction::Deny,
            ..Default::default()
        };
        let governor = SecurityGovernor::new(config);
        governor.register_policy(make_deny_policy("deny-all", "resource", "anything"));

        let verdict = governor
            .evaluate("anything", "anyone", &HashMap::new())
            .expect("evaluate");
        assert!(verdict.allowed, "disabled governor should always allow");
        assert!(verdict.matched_policy.is_none());
    }

    /// 13. Policy condition with `contains` operator.
    #[test]
    fn test_condition_contains() {
        let policy = SecurityPolicy {
            id: "contains-test".into(),
            name: "contains".into(),
            description: "test".into(),
            severity: PolicySeverity::Low,
            action: PolicyAction::Deny,
            conditions: vec![PolicyCondition {
                field: "resource".into(),
                operator: ConditionOperator::Contains,
                value: "secret".into(),
            }],
            composition: PolicyComposition::And,
            escalation_level: None,
        };

        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());
        governor.register_policy(policy);

        let verdict = governor
            .evaluate("top-secret-file", "u", &HashMap::new())
            .expect("evaluate");
        assert!(
            !verdict.allowed,
            "should deny because resource contains 'secret'"
        );

        let verdict = governor
            .evaluate("public-file", "u", &HashMap::new())
            .expect("evaluate");
        assert!(
            !verdict.allowed,
            "should deny because catch-all matches when no other policy matches"
        );
        assert_eq!(verdict.matched_policy, Some("deny-unknown-resource".into()));
    }

    /// 14. Policy with OR composition — either condition is sufficient.
    #[test]
    fn test_or_composition() {
        let policy = SecurityPolicy {
            id: "or-test".into(),
            name: "OR".into(),
            description: "matches if actor is admin OR resource is /danger".into(),
            severity: PolicySeverity::High,
            action: PolicyAction::RequireReview,
            conditions: vec![
                PolicyCondition {
                    field: "actor".into(),
                    operator: ConditionOperator::Equals,
                    value: "admin".into(),
                },
                PolicyCondition {
                    field: "resource".into(),
                    operator: ConditionOperator::Equals,
                    value: "/danger".into(),
                },
            ],
            composition: PolicyComposition::Or,
            escalation_level: None,
        };

        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());
        governor.register_policy(policy);

        // Match via actor.
        let v1 = governor
            .evaluate("/safe", "admin", &HashMap::new())
            .expect("evaluate");
        assert!(v1.required_review);

        // Match via resource.
        let v2 = governor
            .evaluate("/danger", "guest", &HashMap::new())
            .expect("evaluate");
        assert!(v2.required_review);

        // No match.
        let v3 = governor
            .evaluate("/safe", "guest", &HashMap::new())
            .expect("evaluate");
        assert!(!v3.required_review);
    }

    /// 15. Clear audit log.
    #[test]
    fn test_clear_audit() {
        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());
        governor.record_audit(AuditEntry::new(
            "p".into(),
            PolicyVerdict::allow(),
            "r".into(),
            "a".into(),
            "d".into(),
        ));
        assert_eq!(governor.audit_log().len(), 1);

        governor.clear_audit();
        assert_eq!(governor.audit_log().len(), 0);
    }
}
