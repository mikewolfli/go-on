use tokio::sync::OwnedSemaphorePermit;

// ── Permit leak prevention ─────────────────────────────────────────────────

/// RAII guard that holds the semaphore permits for an active task.
///
/// Dropping this guard releases the permits back to their semaphores,
/// preventing resource leaks if a caller drops a dequeued task without
/// calling `complete()` or `fail()`.
pub struct TaskPermitGuard {
    /// Global concurrency permit.
    global_permit: Option<OwnedSemaphorePermit>,
    /// Per-role concurrency permit.
    role_permit: Option<OwnedSemaphorePermit>,
    /// Per-provider bulkhead permit.
    provider_permit: Option<OwnedSemaphorePermit>,
}

impl TaskPermitGuard {
    pub fn new(global_permit: OwnedSemaphorePermit, role_permit: OwnedSemaphorePermit) -> Self {
        Self {
            global_permit: Some(global_permit),
            role_permit: Some(role_permit),
            provider_permit: None,
        }
    }

    pub fn with_provider_permit(
        global_permit: OwnedSemaphorePermit,
        role_permit: OwnedSemaphorePermit,
        provider_permit: OwnedSemaphorePermit,
    ) -> Self {
        Self {
            global_permit: Some(global_permit),
            role_permit: Some(role_permit),
            provider_permit: Some(provider_permit),
        }
    }

    /// Take ownership of the permits (consuming the guard without releasing).
    /// Used by `complete()` and `fail()` to release permits explicitly.
    pub fn into_inner(mut self) -> (OwnedSemaphorePermit, OwnedSemaphorePermit) {
        let global = self.global_permit.take().unwrap();
        let role = self.role_permit.take().unwrap();
        (global, role)
    }
}

impl Drop for TaskPermitGuard {
    fn drop(&mut self) {
        // Just dropping the Option<OwnedSemaphorePermit> values releases
        // the permits back to their semaphores automatically.
        let _ = self.global_permit.take();
        let _ = self.role_permit.take();
        let _ = self.provider_permit.take();
    }
}
