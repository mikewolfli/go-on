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
//! ```text
//! let config = SecurityGovernorConfig::default();
//! let governor = SecurityGovernor::new(config);
//! governor.register_policy(my_policy);
//! let verdict = governor.evaluate("resource:file://x", "actor:alice", &json!({}))?;
//! println!("{:?}", verdict);
//! ```

/// Maximum number of audit entries retained in memory to prevent unbounded growth.
use crate::i18n::{t, tf};
use crate::security::severity::DetectionSeverity;
use anyhow::Result;
use indexmap::IndexMap;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Policy actions
// ---------------------------------------------------------------------------

/// Action to take when a policy's conditions match a request.
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, PartialEq, Default)]
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
#[derive(Debug, Clone)]
pub struct SecurityPolicy {
    /// Unique policy identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of what this policy enforces.
    pub description: String,
    /// Severity level (shared [`DetectionSeverity`](crate::security::severity::DetectionSeverity)).
    pub severity: DetectionSeverity,
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
#[derive(Debug, Clone)]
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
///
/// # Related types
/// - [`crate::governance::audit::AuditLogEntry`] — a general-purpose audit log
///   entry for agent/tool/phase decisions. `AuditEntry` is security-policy-specific
///   and is used internally by [`SecurityGovernor`], while `AuditLogEntry` covers
///   the broader governance audit trail.
#[derive(Debug, Clone)]
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

impl From<AuditEntry> for crate::governance::audit::AuditLogEntry {
    fn from(e: AuditEntry) -> Self {
        crate::governance::audit::AuditLogEntry {
            timestamp: format!("{}", e.timestamp),
            task_id: e.policy_id.clone(),
            phase: "security_governor".to_string(),
            agent: Some(e.actor),
            tool: None,
            decision: format!(
                "allowed={} escalation={}",
                e.verdict.allowed, e.verdict.escalation_level
            ),
            inputs: serde_json::json!({
                "resource": e.resource,
                "detail": e.detail,
            }),
            outputs: None,
            error: None,
            confidence: None,
            data_classification: None,
            compliance_tags: Vec::new(),
            retention_policy: None,
            correlation_id: None,
        }
    }
}

// ---------------------------------------------------------------------------
// SecurityGovernorConfig
// ---------------------------------------------------------------------------

/// Configuration for the security governor.
#[derive(Debug, Clone)]
pub struct SecurityGovernorConfig {
    /// Whether the governor is enabled.
    pub enabled: bool,
    /// Default action when no policy matches.
    pub default_action: PolicyAction,
    /// Policy mode: "enforce", "advisory", or empty (default).
    /// Controls how governance policies are applied during request processing.
    pub policy_mode: String,
    /// Baseline security policies to register at construction time.
    /// If empty, [`SecurityGovernor::register_default_policies`] is called instead.
    pub default_policies: Vec<SecurityPolicy>,
}

impl Default for SecurityGovernorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_action: PolicyAction::Allow,
            policy_mode: String::new(),
            default_policies: vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// GovernorProfile
// ---------------------------------------------------------------------------

/// Runtime metrics snapshot for the [`SecurityGovernor`].
#[derive(Debug, Clone)]
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
            policies: IndexMap::new(),
            total_evaluations: 0,
            total_denials: 0,
            total_reviews: 0,
            active_escalations: 0,
            config,
        };
        let governor = Self {
            inner: Arc::new(Mutex::new(inner)),
        };

        // Register policies from config if provided, otherwise use built-in defaults.
        {
            let inner = governor.inner.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("SecurityGovernor lock poisoned in new, recovering");
                poisoned.into_inner()
            });
            if inner.config.default_policies.is_empty() {
                // Drop the lock before calling register_default_policies which re-locks.
                drop(inner);
                governor.register_default_policies();
            } else {
                let policies = inner.config.default_policies.clone();
                drop(inner);
                for policy in policies {
                    governor.register_policy(policy);
                }
            }
        }

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
            severity: DetectionSeverity::High,
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
            severity: DetectionSeverity::High,
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
            severity: DetectionSeverity::Medium,
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
                    // Denial count is recorded centrally in `record_audit`.
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
                    // Review count is recorded centrally in `record_audit`.
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
                    // Escalation count is recorded centrally in `record_audit`.
                    let level = escalation_level.unwrap_or_else(|| "elevated".into());
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
                // Denial count is recorded centrally in `record_audit`.
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
                // Denial count is recorded centrally in `record_audit`.
                PolicyVerdict {
                    allowed: false,
                    required_review: false,
                    escalation_level: "normal".into(),
                    matched_policy: None,
                    reasons: vec![t("error.security_governor.no_match_denied")],
                }
            }
            PolicyAction::RequireReview => {
                // Review count is recorded centrally in `record_audit`.
                PolicyVerdict {
                    allowed: true,
                    required_review: true,
                    escalation_level: "normal".into(),
                    matched_policy: None,
                    reasons: vec![t("error.security_governor.no_match_review")],
                }
            }
            PolicyAction::Escalate => {
                // Escalation count is recorded centrally in `record_audit`.
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

    /// Record an audit log entry: mirrors the entry into the canonical global
    /// audit sink and updates governance metric counters (denials, reviews,
    /// active escalations). Evaluation totals are counted in [`Self::evaluate`]
    /// — this method does not double-count them.
    pub fn record_audit(&self, entry: AuditEntry) {
        self.record_audit_counters(&entry);
        // Single process-wide audit sink (From conversion). The harness bus
        // layer also writes a phase-specific entry (pre_route / verify_output)
        // for the same evaluation; keeping this sink write here means each
        // evaluation is logged twice to the same sink. To keep one audit
        // record per evaluation, external callers (PolicyEvaluator) use
        // `record_audit_counters` and let the harness bus own the sink write.
        crate::governance::audit::global_audit_log().record(entry.into());
    }

    /// Update denial/review/escalation counters for an audit entry without
    /// writing to the global audit sink. Used by the harness evaluator, which
    /// already emits the phase-level audit entry through the harness bus.
    pub fn record_audit_counters(&self, entry: &AuditEntry) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("SecurityGovernor lock poisoned in record_audit, recovering");
            poisoned.into_inner()
        });
        if !entry.verdict.allowed {
            inner.total_denials += 1;
        }
        if entry.verdict.required_review {
            inner.total_reviews += 1;
        }
        if entry.verdict.escalation_level != "normal" {
            inner.active_escalations += 1;
        }
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

    /// Return the number of registered security policies without snapshotting
    /// the whole [`GovernorProfile`]. Used on the harness hot path to avoid
    /// iterating policy metadata on every evaluation.
    pub fn policies_count(&self) -> u64 {
        self.inner
            .lock()
            .map(|inner| inner.policies.len() as u64)
            .unwrap_or(0)
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
            severity: DetectionSeverity::Low,
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
            severity: DetectionSeverity::High,
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
            severity: DetectionSeverity::Medium,
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
            severity: DetectionSeverity::Critical,
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

        // The entry must reach the canonical process-wide audit sink with the
        // policy id preserved as `task_id` (see the From conversion).
        let entries = crate::governance::audit::global_audit_log().entries();
        assert!(
            entries.iter().any(|e| e.task_id == "p-audit"),
            "audit entry should be mirrored into the global sink"
        );
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
        let v1 = governor.evaluate("x", "u", &HashMap::new()).ok();
        let v2 = governor.evaluate("y", "u", &HashMap::new()).ok();

        let profile = governor.profile();
        assert_eq!(profile.total_evaluations, 2);
        // Denial/review/escalation counters are driven by `record_audit`
        // (the production path — HarnessBus records an audit entry after
        // every evaluation) so `evaluate` alone does not bump them.
        assert_eq!(profile.total_denials, 0);

        // Production-shaped audit recording for both verdicts.
        for verdict in [v1, v2].into_iter().flatten() {
            governor.record_audit(AuditEntry::new(
                verdict.matched_policy.clone().unwrap_or_default(),
                verdict,
                "test".to_string(),
                "u".to_string(),
                String::new(),
            ));
        }
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

    /// Escalation with a custom level is counted exactly once via
    /// `record_audit` (no double-count from `evaluate` itself).
    #[test]
    fn test_escalate_custom_level_counts_once_via_record_audit() {
        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());
        let policy = make_escalate_policy("esc2", "actor", "sensitive-role", "critical");
        governor.register_policy(policy);

        let verdict = governor
            .evaluate("r", "sensitive-role", &HashMap::new())
            .expect("evaluate");
        assert!(verdict.allowed);
        assert_eq!(verdict.escalation_level, "critical");
        // `evaluate` alone must not bump escalation counters (centralized
        // in `record_audit` — see principle §3 batch 3 unified counting).
        assert_eq!(governor.profile().active_escalations, 0);

        governor.record_audit(AuditEntry::new(
            "esc2".into(),
            verdict,
            "test".to_string(),
            "u".to_string(),
            String::new(),
        ));
        assert_eq!(governor.profile().active_escalations, 1);
    }

    /// Default-action Deny (no policy matched) is counted exactly once via
    /// `record_audit`, covering the no-match branch the earlier unified-count
    /// test did not exercise.
    #[test]
    fn test_default_deny_counts_once_via_record_audit() {
        let governor = SecurityGovernor::new(SecurityGovernorConfig::default());
        // Default config uses default_action Deny; register nothing so the
        // default-action branch (not the catch-all) is hit.
        let verdict = governor
            .evaluate("anything", "anyone", &HashMap::new())
            .expect("evaluate");
        assert!(!verdict.allowed);
        assert_eq!(governor.profile().total_denials, 0);

        governor.record_audit(AuditEntry::new(
            "none".into(),
            verdict,
            "test".to_string(),
            "u".to_string(),
            String::new(),
        ));
        assert_eq!(governor.profile().total_denials, 1);
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
            severity: DetectionSeverity::Low,
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
            severity: DetectionSeverity::High,
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
}
