//! Platform adapter registry (M3.4).
//!
//! Thread-safe registry of [`PlatformAdapter`]s keyed by `platform_name()`.
//! Registration returns the crate's [`RegistrationGuard`], so an adapter is
//! removed by dropping the guard (or calling `rollback()` on it) — the same
//! RAII unregister pattern used by tool/skill registries.

use std::sync::{Arc, RwLock};

use crate::gateway::adapter::PlatformAdapter;
use crate::orchestration::registration::RegistrationGuard;

/// Registry of registered platform adapters (M3.4).
///
/// Adapters are resolved by name; when several share a name, the most recently
/// registered wins (mirrors overlay semantics). Removal is by pointer identity,
/// so dropping a guard never unregisters a different same-named instance.
pub struct PlatformRegistry {
    platforms: Arc<RwLock<Vec<Arc<dyn PlatformAdapter>>>>,
}

impl Default for PlatformRegistry {
    fn default() -> Self {
        Self {
            platforms: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl PlatformRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an adapter. The returned guard removes exactly this adapter
    /// when dropped (or via [`RegistrationGuard::rollback`]); keep it alive for
    /// the lifetime the adapter should stay registered.
    ///
    /// The guard is intentionally `!Send` (see `orchestration::registration`),
    /// so callers must hold it on the registering thread.
    pub fn register(&self, adapter: Arc<dyn PlatformAdapter>) -> RegistrationGuard {
        self.platforms
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::clone(&adapter));
        let platforms = Arc::clone(&self.platforms);
        RegistrationGuard::new(move || {
            platforms
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|candidate| !Arc::ptr_eq(candidate, &adapter));
        })
    }

    /// Resolve the most recently registered adapter with `name`.
    pub fn adapter(&self, name: &str) -> Option<Arc<dyn PlatformAdapter>> {
        self.platforms
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .rev()
            .find(|platform| platform.platform_name() == name)
            .cloned()
    }

    /// Names of all registered platforms, in registration order, deduplicated.
    pub fn platform_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = Vec::new();
        for platform in self
            .platforms
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            let name = platform.platform_name();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::adapter::InboundMessage;

    struct TestAdapter {
        name: &'static str,
    }

    impl PlatformAdapter for TestAdapter {
        fn platform_name(&self) -> &'static str {
            self.name
        }

        fn parse_inbound(
            &self,
            _raw: &[u8],
            _content_type: &str,
        ) -> anyhow::Result<Vec<InboundMessage>> {
            Ok(Vec::new())
        }

        fn render_reply(
            &self,
            _reply: &str,
            _original: &InboundMessage,
        ) -> anyhow::Result<Vec<u8>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn register_makes_adapter_resolvable_until_guard_drops() {
        let registry = PlatformRegistry::new();
        let guard = registry.register(Arc::new(TestAdapter { name: "telegram" }));

        assert_eq!(registry.platform_names(), vec!["telegram"]);
        assert!(registry.adapter("telegram").is_some());
        assert!(registry.adapter("wecom").is_none());

        drop(guard);
        assert!(registry.adapter("telegram").is_none());
        assert!(registry.platform_names().is_empty());
    }

    #[test]
    fn guard_rollback_unregisters_exactly_once() {
        let registry = PlatformRegistry::new();
        let first = registry.register(Arc::new(TestAdapter { name: "telegram" }));
        let second = registry.register(Arc::new(TestAdapter { name: "telegram" }));

        // Same-named re-registration resolves to the newest adapter.
        assert!(registry.adapter("telegram").is_some());

        drop(first);
        // Pointer-identity removal: the second instance is still registered.
        assert!(
            registry.adapter("telegram").is_some(),
            "dropping one guard must not unregister the other same-named adapter"
        );

        drop(second);
        assert!(registry.adapter("telegram").is_none());
    }

    #[test]
    fn rollback_removes_adapter_immediately() {
        let registry = PlatformRegistry::new();
        let guard = registry.register(Arc::new(TestAdapter { name: "wecom" }));
        guard.rollback();
        assert!(registry.adapter("wecom").is_none());
        assert!(registry.platform_names().is_empty());
    }
}
