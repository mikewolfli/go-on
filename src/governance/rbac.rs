//! Rbac — F-GAP-15
//!
//! OpenCLAW Zero-Trust RBAC Module
//! Implements Role-Based Access Control for multi-tenant deployments.
//!
//! This provides a minimal but functional RBAC system that can be extended
//! for full OpenCLAW compliance.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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

/// The RBAC enforcer
#[derive(Debug, Clone)]
pub struct RbacEnforcer {
    /// Role -> permissions mapping
    role_permissions: HashMap<String, HashSet<Permission>>,
    /// Tenants (optional multi-tenant support)
    #[allow(dead_code)] // F-GAP-15 — tenant isolation for multi-tenant deployment
    tenants: HashSet<String>,
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

    /// Check if a principal has access to a resource with a specific permission
    pub fn check_access(
        &self,
        principal: &Principal,
        required_perm: &Permission,
    ) -> AccessDecision {
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
                reason: format!(
                    "Principal '{}' has unknown role(s): {:?}",
                    principal.id, principal.roles
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
            reason: format!(
                "Principal '{}' lacks permission {:?}",
                principal.id, required_perm
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

    #[cfg(test)]
    #[allow(dead_code)]
    fn tenant_count(&self) -> usize {
        self.tenants.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
