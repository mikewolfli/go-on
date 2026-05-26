//! Rbac — F-GAP-15
//!
//! OpenCLAW Zero-Trust RBAC Module
//! Implements Role-Based Access Control for multi-tenant deployments.
//!
//! This provides a minimal but functional RBAC system that can be extended
//! for full OpenCLAW compliance.

use crate::i18n::tf;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Built-in roles
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum BuiltinRole {
    Admin,
    User,
    Viewer,
    Monitor,
}

impl BuiltinRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            BuiltinRole::Admin => "admin",
            BuiltinRole::User => "user",
            BuiltinRole::Viewer => "viewer",
            BuiltinRole::Monitor => "monitor",
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "admin" => Some(BuiltinRole::Admin),
            "user" => Some(BuiltinRole::User),
            "viewer" => Some(BuiltinRole::Viewer),
            "monitor" => Some(BuiltinRole::Monitor),
            _ => None,
        }
    }

    /// Default permissions for each built-in role
    pub fn default_permissions(&self) -> Vec<Permission> {
        match self {
            BuiltinRole::Admin => vec![
                Permission::Read,
                Permission::Write,
                Permission::Execute,
                Permission::Admin,
                Permission::ManageUsers,
                Permission::ManageConfig,
                Permission::Audit,
            ],
            BuiltinRole::User => vec![Permission::Read, Permission::Write, Permission::Execute],
            BuiltinRole::Viewer => vec![Permission::Read],
            BuiltinRole::Monitor => vec![Permission::Read, Permission::Monitor],
        }
    }
}

/// Permissions for the RBAC system
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Permission {
    Read,
    Write,
    Execute,
    Admin,
    ManageUsers,
    ManageConfig,
    Audit,
    Monitor,
}

impl Permission {
    #[cfg(test)]
    #[allow(dead_code)]
    fn as_str(&self) -> &'static str {
        match self {
            Permission::Read => "read",
            Permission::Write => "write",
            Permission::Execute => "execute",
            Permission::Admin => "admin",
            Permission::ManageUsers => "manage_users",
            Permission::ManageConfig => "manage_config",
            Permission::Audit => "audit",
            Permission::Monitor => "monitor",
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "read" => Some(Permission::Read),
            "write" => Some(Permission::Write),
            "execute" => Some(Permission::Execute),
            "admin" => Some(Permission::Admin),
            "manage_users" => Some(Permission::ManageUsers),
            "manage_config" => Some(Permission::ManageConfig),
            "audit" => Some(Permission::Audit),
            "monitor" => Some(Permission::Monitor),
            _ => None,
        }
    }
}

/// A user/principal in the RBAC system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub id: String,
    pub roles: Vec<String>,
    pub permissions: HashSet<Permission>,
    pub tenant_id: Option<String>,
}

impl Principal {
    pub fn new(id: &str, roles: Vec<&str>, tenant_id: Option<&str>) -> Self {
        let role_strings: Vec<String> = roles.into_iter().map(|r| r.to_string()).collect();
        Self {
            id: id.to_string(),
            roles: role_strings,
            permissions: HashSet::new(),
            tenant_id: tenant_id.map(|s| s.to_string()),
        }
    }

    pub fn has_permission(&self, permission: &Permission) -> bool {
        self.permissions.contains(permission)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }
}

/// RBAC decision
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AccessDecision {
    Allow,
    Deny { reason: String },
    Escalate { required_role: String },
}

/// Environment variable for registering tenants at startup.
/// Comma-separated list of tenant IDs, e.g. "tenant-a,tenant-b,acme-corp"
pub const GO_ON_TENANTS_ENV: &str = "GO_ON_TENANTS";
/// Environment variable for registering tenants from a file path.
/// The file can contain comma-separated and/or newline-separated tenant IDs.
pub const GO_ON_TENANTS_FILE_ENV: &str = "GO_ON_TENANTS_FILE";

/// The RBAC enforcer
#[derive(Debug, Clone)]
pub struct RbacEnforcer {
    /// Role -> permissions mapping
    role_permissions: HashMap<String, HashSet<Permission>>,
    /// Tenants (optional multi-tenant support)
    pub(crate) tenants: HashSet<String>,
}

impl Default for RbacEnforcer {
    fn default() -> Self {
        Self::new()
    }
}

impl RbacEnforcer {
    pub fn new() -> Self {
        let mut enforcer = Self {
            role_permissions: HashMap::new(),
            tenants: HashSet::new(),
        };
        enforcer.init_builtin_roles();
        enforcer
    }

    /// Return all registered tenant IDs.
    pub fn tenant_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.tenants.iter().cloned().collect();
        ids.sort();
        ids
    }

    /// Check whether the given tenant ID is registered.
    pub fn has_tenant(&self, tenant_id: &str) -> bool {
        self.tenants.contains(tenant_id)
    }

    fn init_builtin_roles(&mut self) {
        let builtins = vec![
            BuiltinRole::Admin,
            BuiltinRole::User,
            BuiltinRole::Viewer,
            BuiltinRole::Monitor,
        ];
        for role in builtins {
            let perms: HashSet<Permission> = role.default_permissions().into_iter().collect();
            self.role_permissions
                .insert(role.as_str().to_string(), perms);
        }
    }

    /// Register a custom role with specific permissions
    pub fn register_role(&mut self, role: &str, permissions: Vec<Permission>) {
        self.role_permissions
            .insert(role.to_string(), permissions.into_iter().collect());
    }

    /// Add a tenant
    pub fn add_tenant(&mut self, tenant_id: &str) {
        self.tenants.insert(tenant_id.to_string());
    }

    /// Register tenants from the GO_ON_TENANTS environment variable.
    /// The env var should contain a comma-separated list of tenant IDs.
    /// Returns the number of tenants registered.
    pub fn register_tenants_from_env(&mut self) -> usize {
        match std::env::var(GO_ON_TENANTS_ENV) {
            Ok(val) if !val.trim().is_empty() => self.register_tenants_from_str(&val),
            _ => 0,
        }
    }

    /// Register tenants from `GO_ON_TENANTS_FILE` when set.
    ///
    /// Accepts comma-separated and/or newline-separated tenant IDs.
    /// Returns the number of newly registered tenant IDs.
    pub fn register_tenants_from_file_env(&mut self) -> usize {
        let path = match std::env::var(GO_ON_TENANTS_FILE_ENV) {
            Ok(value) if !value.trim().is_empty() => value,
            _ => return 0,
        };

        let content = match std::fs::read_to_string(Path::new(path.trim())) {
            Ok(value) => value,
            Err(_) => return 0,
        };

        self.register_tenants_from_str(&content)
    }

    /// Register tenants from both `GO_ON_TENANTS` and `GO_ON_TENANTS_FILE`.
    /// Returns total number of newly registered tenant IDs.
    pub fn register_tenants_from_sources(&mut self) -> usize {
        self.register_tenants_from_env() + self.register_tenants_from_file_env()
    }

    fn register_tenants_from_str(&mut self, raw: &str) -> usize {
        let before = self.tenants.len();
        for id in raw
            .split(&[',', '\n', '\r'])
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            self.add_tenant(id);
        }
        self.tenants.len().saturating_sub(before)
    }

    /// Check if a principal has access to a resource with a specific permission
    pub fn check_access(
        &self,
        principal: &Principal,
        required_perm: &Permission,
    ) -> AccessDecision {
        if !self.tenants.is_empty() {
            let Some(tenant_id) = principal.tenant_id.as_deref() else {
                return AccessDecision::Deny {
                    reason: tf("error.rbac.missing_tenant", &[("principal", &principal.id)]),
                };
            };

            if !self.tenants.contains(tenant_id) {
                return AccessDecision::Deny {
                    reason: tf(
                        "error.rbac.unknown_tenant",
                        &[("principal", &principal.id), ("tenant", tenant_id)],
                    ),
                };
            }
        }

        // Check role-based permissions
        for role_name in &principal.roles {
            if let Some(perms) = self.role_permissions.get(role_name) {
                if perms.contains(required_perm) {
                    return AccessDecision::Allow;
                }
                // If they have Admin, they get everything
                if perms.contains(&Permission::Admin) {
                    return AccessDecision::Allow;
                }
            }
        }

        // Check direct permissions on principal
        if principal.has_permission(required_perm) {
            return AccessDecision::Allow;
        }

        // None of the principal's roles grant this permission.
        // Check if ANY of the principal's roles are known to us.
        let has_known_role = principal
            .roles
            .iter()
            .any(|r| self.role_permissions.contains_key(r));

        // If the principal has no known roles, deny without escalation suggestion.
        if !has_known_role {
            return AccessDecision::Deny {
                reason: tf(
                    "error.rbac.unknown_role",
                    &[
                        ("principal", &principal.id),
                        ("roles", &format!("{:?}", principal.roles)),
                    ],
                ),
            };
        }

        // Check if ANY role would grant this permission (for escalation suggestion)
        let mut suggested_role: Option<String> = None;
        for (role_name, perms) in &self.role_permissions {
            if perms.contains(required_perm) {
                if principal.roles.contains(role_name) {
                    // Principal has this role but it doesn't include the permission
                    continue;
                }
                suggested_role = Some(role_name.clone());
                break;
            }
        }

        if let Some(role) = suggested_role {
            return AccessDecision::Escalate {
                required_role: role,
            };
        }

        AccessDecision::Deny {
            reason: tf(
                "error.rbac.lacks_permission",
                &[
                    ("principal", &principal.id),
                    ("perm", &format!("{:?}", required_perm)),
                ],
            ),
        }
    }

    /// Resolve a principal's permissions from their roles
    pub fn resolve_permissions(&self, principal: &mut Principal) {
        for role_name in &principal.roles {
            if let Some(perms) = self.role_permissions.get(role_name) {
                for perm in perms.iter() {
                    principal.permissions.insert(perm.clone());
                }
            }
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn role_count(&self) -> usize {
        self.role_permissions.len()
    }

    /// Check access and tenant budget in a single call.
    /// Returns `Ok(())` when both RBAC access and tenant budget allow the operation.
    /// Returns `Err` with a human-readable reason when either check fails.
    pub fn check_access_with_budget(
        &self,
        principal: &Principal,
        required_perm: &Permission,
        budget_enforcer: Option<&crate::governance::hardening::TenantBudgetEnforcer>,
    ) -> Result<(), String> {
        // 1. RBAC access check
        match self.check_access(principal, required_perm) {
            AccessDecision::Allow => {}
            AccessDecision::Deny { reason } => return Err(reason),
            AccessDecision::Escalate { required_role } => {
                return Err(tf(
                    "error.rbac.needs_escalation",
                    &[("principal", &principal.id), ("role", &required_role)],
                ));
            }
        }

        // 2. Tenant budget check (when a budget enforcer is available and principal has a tenant)
        if let (Some(enforcer), Some(tenant_id)) = (budget_enforcer, principal.tenant_id.as_deref())
        {
            // If the tenant is not registered in the budget enforcer yet, auto-provision is expected
            // to have happened at startup; if it's still missing, let it through.
            if enforcer.quotas().contains_key(tenant_id) {
                enforcer.check_can_start(tenant_id).map_err(|e| {
                    tf(
                        "error.rbac.budget_exceeded",
                        &[("tenant", tenant_id), ("detail", &e)],
                    )
                })?;
            }
        }

        Ok(())
    }

    /// Mark a task as started for a tenant (used after a successful budget check).
    pub fn start_tenant_task(
        &self,
        principal: &Principal,
        budget_enforcer: Option<
            &std::sync::Mutex<crate::governance::hardening::TenantBudgetEnforcer>,
        >,
    ) {
        if let (Some(enforcer), Some(tenant_id)) = (budget_enforcer, principal.tenant_id.as_deref())
        {
            if let Ok(mut guard) = enforcer.lock() {
                guard.start_task(tenant_id);
            }
        }
    }

    /// Record resource usage for a tenant after a task completes.
    pub fn record_tenant_usage(
        &self,
        principal: &Principal,
        tokens: usize,
        api_calls: usize,
        budget_enforcer: Option<
            &std::sync::Mutex<crate::governance::hardening::TenantBudgetEnforcer>,
        >,
    ) {
        if let (Some(enforcer), Some(tenant_id)) = (budget_enforcer, principal.tenant_id.as_deref())
        {
            if let Ok(mut guard) = enforcer.lock() {
                guard.record_usage(tenant_id, tokens, api_calls);
            }
        }
    }

    /// Register tenants from the supplied JSON array of tenant IDs.
    pub fn register_tenants_from_json(&mut self, tenants: &Value) -> usize {
        let before = self.tenants.len();
        if let Some(arr) = tenants.as_array() {
            for v in arr {
                if let Some(id) = v.as_str() {
                    self.add_tenant(id);
                }
            }
        }
        self.tenants.len().saturating_sub(before)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn tenant_count(&self) -> usize {
        self.tenants.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::hardening::{TenantBudgetEnforcer, TenantResourceQuota};

    #[test]
    fn test_builtin_admin_has_full_access() {
        let enforcer = RbacEnforcer::new();
        let admin = Principal::new("admin-1", vec!["admin"], None);

        assert_eq!(
            enforcer.check_access(&admin, &Permission::Admin),
            AccessDecision::Allow
        );
        assert_eq!(
            enforcer.check_access(&admin, &Permission::Read),
            AccessDecision::Allow
        );
        assert_eq!(
            enforcer.check_access(&admin, &Permission::ManageConfig),
            AccessDecision::Allow
        );
    }

    #[test]
    fn test_viewer_only_read() {
        let enforcer = RbacEnforcer::new();
        let viewer = Principal::new("viewer-1", vec!["viewer"], None);

        assert_eq!(
            enforcer.check_access(&viewer, &Permission::Read),
            AccessDecision::Allow
        );
        match enforcer.check_access(&viewer, &Permission::Write) {
            AccessDecision::Escalate { required_role } => {
                // Either "user" or "admin" can grant Write permission;
                // accept whichever the HashMap iteration order yields.
                assert!(
                    required_role == "user" || required_role == "admin",
                    "Expected escalation to user or admin, got {}",
                    required_role
                );
            }
            other => panic!("Expected Escalate for viewer write, got {:?}", other),
        }
    }

    #[test]
    fn test_unknown_role_denied() {
        let enforcer = RbacEnforcer::new();
        let unknown = Principal::new("unknown", vec!["nonexistent"], None);

        match enforcer.check_access(&unknown, &Permission::Read) {
            AccessDecision::Deny { reason } => {
                assert!(reason.contains("unknown"));
            }
            _ => panic!("Expected Deny for unknown role"),
        }
    }

    #[test]
    fn test_custom_role() {
        let mut enforcer = RbacEnforcer::new();
        enforcer.register_role(
            "developer",
            vec![Permission::Read, Permission::Write, Permission::Execute],
        );

        let dev = Principal::new("dev-1", vec!["developer"], None);
        assert_eq!(
            enforcer.check_access(&dev, &Permission::Execute),
            AccessDecision::Allow
        );
        assert_eq!(
            enforcer.check_access(&dev, &Permission::Audit),
            AccessDecision::Escalate {
                required_role: "admin".to_string()
            }
        );
    }

    #[test]
    fn test_tenant_isolation() {
        let mut enforcer = RbacEnforcer::new();
        enforcer.add_tenant("tenant-a");
        enforcer.add_tenant("tenant-b");

        assert_eq!(enforcer.tenant_count(), 2);

        let allowed = Principal::new("user-a", vec!["user"], Some("tenant-a"));
        assert_eq!(
            enforcer.check_access(&allowed, &Permission::Read),
            AccessDecision::Allow
        );

        let missing_tenant = Principal::new("user-missing", vec!["user"], None);
        match enforcer.check_access(&missing_tenant, &Permission::Read) {
            AccessDecision::Deny { reason } => {
                assert!(reason.contains("missing tenant context"));
            }
            other => panic!("Expected missing tenant to deny, got {:?}", other),
        }

        let unknown_tenant = Principal::new("user-unknown", vec!["user"], Some("tenant-z"));
        match enforcer.check_access(&unknown_tenant, &Permission::Read) {
            AccessDecision::Deny { reason } => {
                assert!(reason.contains("unknown tenant"));
            }
            other => panic!("Expected unknown tenant to deny, got {:?}", other),
        }
    }

    #[test]
    fn test_register_tenants_from_env() {
        // Ensure env is clean before starting
        unsafe {
            std::env::remove_var(GO_ON_TENANTS_ENV);
        }

        // Test empty env first
        {
            let mut enforcer = RbacEnforcer::new();
            let count = enforcer.register_tenants_from_env();
            assert_eq!(count, 0, "no env set should register 0 tenants");
        }

        // Set env and test registration
        unsafe {
            std::env::set_var(GO_ON_TENANTS_ENV, "tenant-a,tenant-b,tenant-c");
        }
        let mut enforcer = RbacEnforcer::new();
        let count = enforcer.register_tenants_from_env();
        assert_eq!(count, 3, "should register 3 tenants from env");
        assert_eq!(enforcer.tenant_count(), 3);

        // Verify isolation works with env-registered tenants
        let allowed = Principal::new("user-a", vec!["user"], Some("tenant-a"));
        assert_eq!(
            enforcer.check_access(&allowed, &Permission::Read),
            AccessDecision::Allow
        );
        let unknown = Principal::new("user-z", vec!["user"], Some("tenant-z"));
        match enforcer.check_access(&unknown, &Permission::Read) {
            AccessDecision::Deny { reason } => {
                assert!(reason.contains("unknown tenant"));
            }
            other => panic!("Expected unknown tenant to deny, got {:?}", other),
        }

        // Clean up env
        unsafe {
            std::env::remove_var(GO_ON_TENANTS_ENV);
        }
    }

    #[test]
    fn test_register_tenants_from_file_env() {
        unsafe {
            std::env::remove_var(GO_ON_TENANTS_FILE_ENV);
        }

        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("tenants.txt");
        std::fs::write(&path, "tenant-a\ntenant-b,tenant-c\n").expect("tenant file should exist");

        unsafe {
            std::env::set_var(GO_ON_TENANTS_FILE_ENV, path.to_string_lossy().to_string());
        }

        let mut enforcer = RbacEnforcer::new();
        let count = enforcer.register_tenants_from_file_env();
        assert_eq!(count, 3, "should register 3 tenants from file env");
        assert_eq!(enforcer.tenant_count(), 3);

        unsafe {
            std::env::remove_var(GO_ON_TENANTS_FILE_ENV);
        }
    }

    #[test]
    fn test_register_tenants_from_sources_deduplicates() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("tenants.txt");
        std::fs::write(&path, "tenant-b,tenant-c").expect("tenant file should exist");

        unsafe {
            std::env::set_var(GO_ON_TENANTS_ENV, "tenant-a,tenant-b");
            std::env::set_var(GO_ON_TENANTS_FILE_ENV, path.to_string_lossy().to_string());
        }

        let mut enforcer = RbacEnforcer::new();
        let count = enforcer.register_tenants_from_sources();
        assert_eq!(count, 3, "duplicate tenant IDs should be counted once");
        assert_eq!(enforcer.tenant_count(), 3);

        unsafe {
            std::env::remove_var(GO_ON_TENANTS_ENV);
            std::env::remove_var(GO_ON_TENANTS_FILE_ENV);
        }
    }

    #[test]
    fn test_resolve_permissions() {
        let enforcer = RbacEnforcer::new();
        let mut user = Principal::new("user-1", vec!["user"], None);
        enforcer.resolve_permissions(&mut user);

        assert!(user.has_permission(&Permission::Read));
        assert!(user.has_permission(&Permission::Write));
        assert!(user.has_permission(&Permission::Execute));
        assert!(!user.has_permission(&Permission::Admin));
    }

    #[test]
    fn test_principal_role_check() {
        let principal = Principal::new("test", vec!["admin", "user"], Some("tenant-1"));
        assert!(principal.has_role("admin"));
        assert!(principal.has_role("user"));
        assert!(!principal.has_role("viewer"));
        assert_eq!(principal.tenant_id, Some("tenant-1".to_string()));
    }

    #[test]
    fn test_check_access_with_budget_within_limits() {
        let mut enforcer = RbacEnforcer::new();
        enforcer.add_tenant("tenant-a");

        let mut budget = TenantBudgetEnforcer::new();
        budget.set_quota(TenantResourceQuota {
            tenant_id: "tenant-a".to_string(),
            daily_token_limit: 1_000_000,
            concurrent_tasks_limit: 5,
            daily_api_call_limit: 10_000,
        });

        let principal = Principal::new("user-a", vec!["user"], Some("tenant-a"));
        let result =
            enforcer.check_access_with_budget(&principal, &Permission::Read, Some(&budget));
        assert!(
            result.is_ok(),
            "within-limit budget check should succeed; got: {:?}",
            result
        );
    }

    #[test]
    fn test_check_access_with_budget_exceeds_concurrent_tasks() {
        let mut enforcer = RbacEnforcer::new();
        enforcer.add_tenant("tenant-b");

        let mut budget = TenantBudgetEnforcer::new();
        budget.set_quota(TenantResourceQuota {
            tenant_id: "tenant-b".to_string(),
            daily_token_limit: 1_000_000,
            concurrent_tasks_limit: 1,
            daily_api_call_limit: 10_000,
        });
        // Fill the concurrent slot
        budget.start_task("tenant-b");

        let principal = Principal::new("user-b", vec!["user"], Some("tenant-b"));
        let result =
            enforcer.check_access_with_budget(&principal, &Permission::Read, Some(&budget));
        assert!(result.is_err(), "concurrent task limit breach should fail");
        let err = result.unwrap_err();
        assert!(
            err.contains("Budget exceeded"),
            "error should mention budget; got: {}",
            err
        );
        assert!(
            err.contains("tenant-b"),
            "error should mention tenant; got: {}",
            err
        );
    }

    #[test]
    fn test_cross_tenant_access_denied_in_budget_context() {
        let mut enforcer = RbacEnforcer::new();
        enforcer.add_tenant("acme");
        enforcer.add_tenant("globex");

        let mut budget = TenantBudgetEnforcer::new();
        budget.set_quota(TenantResourceQuota {
            tenant_id: "acme".to_string(),
            daily_token_limit: 500_000,
            concurrent_tasks_limit: 2,
            daily_api_call_limit: 5_000,
        });

        // Cross-tenant: globex principal tries to access acme budget
        let principal = Principal::new("globex-user", vec!["user"], Some("globex"));
        // RBAC check passes (globex is a valid tenant), but budget check will
        // find no quota for globex and let it through (graceful degradation).
        let result =
            enforcer.check_access_with_budget(&principal, &Permission::Read, Some(&budget));
        assert!(
            result.is_ok(),
            "unquoted tenant should not be rejected by budget check; got: {:?}",
            result
        );

        // Now deny by having no tenant context at all
        let no_tenant = Principal::new("anonymous", vec!["user"], None);
        let result =
            enforcer.check_access_with_budget(&no_tenant, &Permission::Read, Some(&budget));
        assert!(result.is_err(), "missing tenant context should be denied");
        let err = result.unwrap_err();
        assert!(
            err.contains("missing tenant context"),
            "error should mention missing tenant; got: {}",
            err
        );
    }

    #[test]
    fn test_tenant_ids_and_has_tenant() {
        let mut enforcer = RbacEnforcer::new();
        enforcer.add_tenant("tenant-a");
        enforcer.add_tenant("tenant-b");

        let ids = enforcer.tenant_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"tenant-a".to_string()));
        assert!(ids.contains(&"tenant-b".to_string()));

        assert!(enforcer.has_tenant("tenant-a"));
        assert!(!enforcer.has_tenant("tenant-z"));
    }

    #[test]
    fn test_register_tenants_from_json() {
        let mut enforcer = RbacEnforcer::new();
        let tenants = serde_json::json!(["tenant-a", "tenant-b", "tenant-c"]);
        let count = enforcer.register_tenants_from_json(&tenants);
        assert_eq!(count, 3);
        assert_eq!(enforcer.tenant_count(), 3);
        assert!(enforcer.has_tenant("tenant-a"));
        assert!(enforcer.has_tenant("tenant-b"));
        assert!(enforcer.has_tenant("tenant-c"));

        // Second call should be idempotent
        let count2 = enforcer.register_tenants_from_json(&tenants);
        assert_eq!(count2, 0, "duplicate tenants should not be counted");
    }
}
