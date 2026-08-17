//! RAII registration guards — undo a registration when dropped (M1.6).
//!
//! [`RegistrationGuard`] is the M4 plugin foundation: a plugin registers
//! tools/skills/events during setup and holds the returned guards; if setup
//! fails partway or the plugin is unloaded, dropping the guards rolls back
//! the registrations exactly once, so a half-registered plugin can never
//! leak entries into the registries.

/// RAII guard: undoes a registration when dropped (M1.6 / M4 plugin base).
///
/// The owned `unregister` closure runs exactly once, on drop (or explicitly
/// via [`rollback`](Self::rollback)), so a failed setup can never leave a
/// half-registered tool/skill/event behind.
///
/// The closure is intentionally **not** `Send`: a guard is a drop-scoped RAII
/// object tied to the lifetime of the registry it unregisters from (the
/// closure may hold a scoped pointer to that registry). Requiring `Send` would
/// invite sharing the pointer across threads and outliving the registry, so
/// the guard is deliberately bound to its registering thread.
pub struct RegistrationGuard {
    unregister: Option<Box<dyn FnOnce()>>,
}

impl RegistrationGuard {
    /// Create a guard that runs `unregister` when dropped.
    pub fn new(unregister: impl FnOnce() + 'static) -> Self {
        Self {
            unregister: Some(Box::new(unregister)),
        }
    }

    /// Run the unregister closure now, and disarm the guard so the closure
    /// does not run again when the guard is dropped.
    pub fn rollback(mut self) {
        if let Some(unregister) = self.unregister.take() {
            unregister();
        }
    }
}

impl Drop for RegistrationGuard {
    fn drop(&mut self) {
        if let Some(unregister) = self.unregister.take() {
            unregister();
        }
    }
}

impl std::fmt::Debug for RegistrationGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The owned closure is opaque; expose only whether it is still armed
        // (i.e. whether the rollback will still run on drop).
        f.debug_struct("RegistrationGuard")
            .field("armed", &self.unregister.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn drop_runs_unregister_closure_exactly_once() {
        let runs = Arc::new(AtomicUsize::new(0));
        {
            let runs_in_guard = Arc::clone(&runs);
            let _guard = RegistrationGuard::new(move || {
                runs_in_guard.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rollback_runs_closure_immediately_and_disarms_drop() {
        let runs = Arc::new(AtomicUsize::new(0));
        let runs_in_guard = Arc::clone(&runs);
        let guard = RegistrationGuard::new(move || {
            runs_in_guard.fetch_add(1, Ordering::SeqCst);
        });
        guard.rollback();
        // The closure ran exactly once — the consumed guard cannot re-run it.
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn multiple_guards_run_independently() {
        let runs = Arc::new(AtomicUsize::new(0));
        {
            let runs_in_guard_a = Arc::clone(&runs);
            let _a = RegistrationGuard::new(move || {
                runs_in_guard_a.fetch_add(1, Ordering::SeqCst);
            });
            let runs_in_guard_b = Arc::clone(&runs);
            let _b = RegistrationGuard::new(move || {
                runs_in_guard_b.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    // TEMP DIAGNOSTIC
    #[test]
    fn registries_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<crate::orchestration::tool::ToolRegistry>();
        assert_send::<crate::orchestration::skill::SkillRegistry>();
    }
}
