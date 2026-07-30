//! Default tool governance policy when RBAC/HarnessBus is not configured.
//!
//! This module ensures that even without a HarnessBus, there is an explicit,
//! observable minimum-trust policy instead of default-allow-all.
//!
//! Implements AUTON-05: tool governance default permissions.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::tool_governance::record_tool_policy_denied;
pub use crate::governance::tool_capability::ToolRiskClass;

/// Deployment profile used by default governance when RBAC is unavailable.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum DefaultGovernanceDeployment {
    LocalDev,
    SimpleServer,
    MultiUsersServer,
    ManagedService,
    Unknown,
}

/// Default policy for a given risk class
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultToolPolicy {
    /// Risk class
    pub risk_class: ToolRiskClass,
    /// Default allow state
    pub default_allow: bool,
    /// Whether this operation requires explicit policy configuration
    pub requires_explicit_policy: bool,
    /// Human-readable description
    pub description: &'static str,
}

impl DefaultToolPolicy {
    /// Get the default policy for a risk class
    pub const fn for_class(risk_class: ToolRiskClass) -> Self {
        match risk_class {
            ToolRiskClass::ReadOnly => Self {
                risk_class,
                default_allow: true,
                requires_explicit_policy: false,
                description: "Read-only operations: allowed by default in all profiles",
            },
            ToolRiskClass::LowRiskWrite => Self {
                risk_class,
                default_allow: true,
                requires_explicit_policy: false,
                description:
                    "Low-risk file writes: allowed by default, audited when policy present",
            },
            ToolRiskClass::HighRiskExecute => Self {
                risk_class,
                default_allow: false,
                requires_explicit_policy: true,
                description:
                    "High-risk execution: blocked by default unless explicit policy permits it",
            },
            ToolRiskClass::Admin => Self {
                risk_class,
                default_allow: true,
                requires_explicit_policy: false,
                description:
                    "Administrative operations: allowed by default for workflow management",
            },
        }
    }
}

/// Resolve deployment hint to a bounded default-governance profile.
pub fn resolve_default_governance_deployment(
    deployment_target: Option<&str>,
) -> DefaultGovernanceDeployment {
    let Some(raw) = deployment_target else {
        return DefaultGovernanceDeployment::Unknown;
    };
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.contains("local") || normalized.contains("dev") {
        DefaultGovernanceDeployment::LocalDev
    } else if normalized.contains("multi") || normalized.contains("tenant") {
        DefaultGovernanceDeployment::MultiUsersServer
    } else if normalized.contains("simple") || normalized.contains("single") {
        DefaultGovernanceDeployment::SimpleServer
    } else if normalized.contains("managed") || normalized.contains("prod") {
        DefaultGovernanceDeployment::ManagedService
    } else {
        DefaultGovernanceDeployment::Unknown
    }
}

fn default_allow_for_deployment(
    risk_class: ToolRiskClass,
    deployment: DefaultGovernanceDeployment,
) -> bool {
    match deployment {
        // Local dev keeps productivity-oriented defaults.
        DefaultGovernanceDeployment::LocalDev => {
            !matches!(risk_class, ToolRiskClass::HighRiskExecute)
        }
        // Server profiles are stricter without explicit RBAC.
        DefaultGovernanceDeployment::SimpleServer
        | DefaultGovernanceDeployment::MultiUsersServer
        | DefaultGovernanceDeployment::ManagedService
        | DefaultGovernanceDeployment::Unknown => matches!(risk_class, ToolRiskClass::ReadOnly),
    }
}

/// Classification result for a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolClassification {
    /// The tool name
    pub tool_name: String,
    /// Risk class
    pub risk_class: ToolRiskClass,
    /// Whether the tool is allowed by default policy
    pub allowed: bool,
    /// Reason for the decision
    pub reason: String,
    /// Whether RBAC is configured (if not, we use defaults)
    pub rbac_configured: bool,
    /// Default governance deployment profile used for decision
    pub deployment: DefaultGovernanceDeployment,
}

/// Classify a tool by name into a risk class.
pub fn classify_tool_risk(tool_name: &str) -> ToolRiskClass {
    crate::governance::tool_capability::ToolCapabilityRegistry::risk_class(tool_name)
}

/// Evaluate a tool call against the default governance policy.
///
/// Returns a `ToolClassification` with the decision and reasoning.
/// This should be called when `HarnessBus` is not present or not configured
/// with RBAC, to ensure there is no "default allow all" blind spot.
pub fn evaluate_default_tool_policy(
    tool_name: &str,
    harness_bus_present: bool,
    rbac_configured: bool,
    deployment_target: Option<&str>,
) -> ToolClassification {
    let risk_class = classify_tool_risk(tool_name);
    let policy = DefaultToolPolicy::for_class(risk_class);
    let deployment = resolve_default_governance_deployment(deployment_target);

    // If HarnessBus with RBAC is present, skip default policy evaluation
    // (the caller should use HarnessBus directly)
    if harness_bus_present && rbac_configured {
        return ToolClassification {
            tool_name: tool_name.to_string(),
            risk_class,
            allowed: true, // HarnessBus will handle the real decision
            reason: "delegated to configured HarnessBus RBAC".to_string(),
            rbac_configured: true,
            deployment,
        };
    }

    let allowed = default_allow_for_deployment(risk_class, deployment);
    if !allowed {
        record_tool_policy_denied();
    }

    let reason = if !harness_bus_present {
        if policy.requires_explicit_policy {
            format!(
                "tool '{tool_name}' is class '{:?}' which requires explicit policy configuration, \
                 but no HarnessBus is present. Blocked by default governance policy \
                 (deployment={:?}).",
                risk_class, deployment
            )
        } else {
            format!(
                "tool '{tool_name}' is class '{:?}' — allowed by default governance policy \
                 (no RBAC configured, deployment={:?}, default_allow={})",
                risk_class, deployment, policy.default_allow
            )
        }
    } else if !rbac_configured {
        // HarnessBus present but RBAC not configured
        if policy.requires_explicit_policy {
            format!(
                "tool '{tool_name}' is class '{:?}' which requires explicit RBAC policy. \
                 HarnessBus is present but RBAC is not configured. Blocked by default \
                 (deployment={:?}).",
                risk_class, deployment
            )
        } else {
            format!(
                "tool '{tool_name}' is class '{:?}' — allowed by default (HarnessBus present, \
                 RBAC not configured, deployment={:?}, default_allow={})",
                risk_class, deployment, policy.default_allow
            )
        }
    } else {
        "allowed by policy".to_string()
    };

    ToolClassification {
        tool_name: tool_name.to_string(),
        risk_class,
        allowed,
        reason,
        rbac_configured,
        deployment,
    }
}

/// Snapshot of the default governance policy state.
pub fn default_governance_policy_snapshot() -> Value {
    json!({
        "policy_name": "default-governance-policy-v1",
        "deployment_profiles": {
            "LocalDev": {
                "ReadOnly": true,
                "LowRiskWrite": true,
                "HighRiskExecute": false,
                "Admin": true,
            },
            "SimpleServer": {
                "ReadOnly": true,
                "LowRiskWrite": false,
                "HighRiskExecute": false,
                "Admin": false,
            },
            "MultiUsersServer": {
                "ReadOnly": true,
                "LowRiskWrite": false,
                "HighRiskExecute": false,
                "Admin": false,
            },
            "ManagedService": {
                "ReadOnly": true,
                "LowRiskWrite": false,
                "HighRiskExecute": false,
                "Admin": false,
            },
            "Unknown": {
                "ReadOnly": true,
                "LowRiskWrite": false,
                "HighRiskExecute": false,
                "Admin": false,
            }
        },
        "risk_classes": {
            "ReadOnly": {
                "default_allow": true,
                "requires_explicit_policy": false,
                "description": "read-only file/query operations"
            },
            "LowRiskWrite": {
                "default_allow": true,
                "requires_explicit_policy": false,
                "description": "file write and patch operations"
            },
            "HighRiskExecute": {
                "default_allow": false,
                "requires_explicit_policy": true,
                "description": "shell/test/network execution — blocked by default"
            },
            "Admin": {
                "default_allow": true,
                "requires_explicit_policy": false,
                "description": "workflow/skill management operations"
            }
        },
        "notes": "This policy applies when HarnessBus is absent or RBAC is not configured. \
                  For production deployments, configure HarnessBus with explicit RBAC rules."
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_blocks_high_risk_without_rbac() {
        let result = evaluate_default_tool_policy("run_tests", false, false, Some("local-dev"));
        assert!(!result.allowed);
        assert_eq!(result.risk_class, ToolRiskClass::HighRiskExecute);
    }

    #[test]
    fn default_policy_allows_read_only_without_rbac() {
        let result =
            evaluate_default_tool_policy("read_file", false, false, Some("managed-service"));
        assert!(result.allowed);
    }

    #[test]
    fn default_policy_blocks_high_risk_with_harness_no_rbac() {
        let result = evaluate_default_tool_policy("bash", true, false, Some("multi-users-server"));
        assert!(!result.allowed);
    }

    #[test]
    fn default_policy_delegates_when_rbac_configured() {
        let result = evaluate_default_tool_policy("run_tests", true, true, Some("simple-server"));
        assert!(result.allowed); // Delegated
        assert!(result.rbac_configured);
    }

    #[test]
    fn managed_profile_blocks_low_risk_write_without_rbac() {
        let result =
            evaluate_default_tool_policy("apply_patch", false, false, Some("managed-service"));
        assert!(!result.allowed);
        assert_eq!(result.risk_class, ToolRiskClass::LowRiskWrite);
    }

    #[test]
    fn local_profile_allows_low_risk_write_without_rbac() {
        let result = evaluate_default_tool_policy("apply_patch", false, false, Some("local-dev"));
        assert!(result.allowed);
        assert_eq!(result.risk_class, ToolRiskClass::LowRiskWrite);
    }
}
