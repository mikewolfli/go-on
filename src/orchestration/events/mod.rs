//! M4.1: named observable event domain with waterfall semantics.
//!
//! [`EventBus`] is a process-wide, thread-safe observer hub for agent
//! lifecycle events. Listeners run in registration order and may return
//! [`EventVerdict::Consume`] to stop later listeners and (for pre-execute
//! events) mark the action as intercepted. Registration is RAII: the
//! returned [`RegistrationGuard`] removes the listener on drop, so a plugin
//! that registers listeners during setup can never leak them.

use crate::orchestration::registration::RegistrationGuard;
use std::sync::{Arc, OnceLock, RwLock};

mod event;
pub use event::{AgentEvent, EventListener, EventVerdict};

/// Thread-safe event hub with waterfall dispatch and RAII registration.
///
/// Listeners are invoked in registration order. [`EventBus::dispatch`] stops
/// at the first [`EventVerdict::Consume`]; the emitter then treats the action
/// as intercepted (for `ToolsPreExecute`).
pub struct EventBus {
    listeners: RwLock<Vec<Arc<dyn EventListener>>>,
}

impl EventBus {
    /// Create an empty bus.
    pub fn new() -> Self {
        Self {
            listeners: RwLock::new(Vec::new()),
        }
    }

    /// Register a listener and return a guard that removes it on drop.
    ///
    /// Removal matches by [`Arc::ptr_eq`], so dropping the guard removes
    /// exactly the listeners registered with this `Arc`.
    ///
    /// The guard's closure captures a raw pointer to this bus. Because the
    /// closure must be `'static` (the guard can outlive the `&self` borrow),
    /// the caller must uphold the scoped-guard contract: **the guard must be
    /// dropped (or rolled back) before the bus itself is dropped or moved**.
    /// This matches the pattern established by
    /// `ToolRegistry::register_guarded`; the guard is intentionally not
    /// `Send` (see `orchestration::registration`).
    pub fn register(&self, listener: Arc<dyn EventListener>) -> RegistrationGuard {
        if let Ok(mut listeners) = self.listeners.write() {
            listeners.push(Arc::clone(&listener));
        }
        let this = std::ptr::from_ref(self);
        RegistrationGuard::new(move || {
            // SAFETY: per the contract above, the guard is dropped (or rolled
            // back) before the bus is dropped or moved, so `this` still points
            // at a live bus whenever the closure runs.
            let bus = unsafe { &*this };
            if let Ok(mut listeners) = bus.listeners.write() {
                listeners.retain(|registered| !Arc::ptr_eq(registered, &listener));
            }
        })
    }

    /// Dispatch an event to all registered listeners in registration order.
    ///
    /// Returns [`EventVerdict::Consume`] as soon as any listener consumes the
    /// event (later listeners are skipped); otherwise [`EventVerdict::Continue`].
    /// If the listener list is poisoned, dispatch degrades to `Continue`.
    pub fn dispatch(&self, event: &AgentEvent) -> EventVerdict {
        let Ok(listeners) = self.listeners.read() else {
            return EventVerdict::Continue;
        };
        for listener in listeners.iter() {
            if listener.on_event(event) == EventVerdict::Consume {
                return EventVerdict::Consume;
            }
        }
        EventVerdict::Continue
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Return the process-wide canonical event bus.
///
/// Mirrors `governance::audit::global_audit_log` and
/// `orchestration::tool::tool_lock_manager`: a single lazily-initialized
/// `OnceLock` static shared by every emitter and listener in the process.
pub fn global_event_bus() -> &'static EventBus {
    static GLOBAL_EVENT_BUS: OnceLock<EventBus> = OnceLock::new();
    GLOBAL_EVENT_BUS.get_or_init(EventBus::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Test listener that records every event it sees and optionally consumes
    /// a specific pre-execute tool call.
    #[derive(Default)]
    struct RecordingListener {
        seen: Mutex<Vec<AgentEvent>>,
        consume_tool: Option<String>,
    }

    impl RecordingListener {
        fn consume_tool(tool_name: &str) -> Self {
            Self {
                seen: Mutex::new(Vec::new()),
                consume_tool: Some(tool_name.to_string()),
            }
        }

        fn count(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }

    impl EventListener for RecordingListener {
        fn on_event(&self, event: &AgentEvent) -> EventVerdict {
            self.seen.lock().unwrap().push(event.clone());
            if let Some(tool) = &self.consume_tool {
                if let AgentEvent::ToolsPreExecute { tool_name, .. } = event {
                    if tool_name == tool {
                        return EventVerdict::Consume;
                    }
                }
            }
            EventVerdict::Continue
        }
    }

    #[test]
    fn dispatch_runs_in_registration_order_and_consume_stops_later_listeners() {
        let bus = EventBus::new();
        let first = Arc::new(RecordingListener::default());
        let consumer = Arc::new(RecordingListener::consume_tool("rm"));
        let last = Arc::new(RecordingListener::default());

        let _guard_first = bus.register(Arc::clone(&first) as Arc<dyn EventListener>);
        let _guard_consumer = bus.register(Arc::clone(&consumer) as Arc<dyn EventListener>);
        let _guard_last = bus.register(Arc::clone(&last) as Arc<dyn EventListener>);

        // Non-pre-execute events pass through every listener.
        let verdict = bus.dispatch(&AgentEvent::AgentRequest {
            request_id: "req-1".to_string(),
        });
        assert_eq!(verdict, EventVerdict::Continue);
        assert_eq!(first.count(), 1);
        assert_eq!(consumer.count(), 1);
        assert_eq!(last.count(), 1);

        // A Consume verdict on tools/pre-execute stops later listeners.
        let verdict = bus.dispatch(&AgentEvent::ToolsPreExecute {
            tool_name: "rm".to_string(),
            input: serde_json::json!({}),
        });
        assert_eq!(verdict, EventVerdict::Consume);
        assert_eq!(first.count(), 2);
        assert_eq!(consumer.count(), 2);
        // `last` never saw the consumed event.
        assert_eq!(last.count(), 1);

        // A non-consumed tool still reaches every listener.
        let verdict = bus.dispatch(&AgentEvent::ToolsPreExecute {
            tool_name: "ls".to_string(),
            input: serde_json::json!({}),
        });
        assert_eq!(verdict, EventVerdict::Continue);
        assert_eq!(last.count(), 2);
    }

    #[test]
    fn dropping_registration_guard_removes_listener() {
        let bus = EventBus::new();
        let listener = Arc::new(RecordingListener::default());

        let guard = bus.register(Arc::clone(&listener) as Arc<dyn EventListener>);
        bus.dispatch(&AgentEvent::AgentTurnStopping {
            request_id: "req-1".to_string(),
        });
        assert_eq!(listener.count(), 1);

        drop(guard);
        bus.dispatch(&AgentEvent::AgentTurnStopping {
            request_id: "req-2".to_string(),
        });
        assert_eq!(listener.count(), 1);
    }

    #[test]
    fn registering_same_listener_twice_dispatches_twice() {
        let bus = EventBus::new();
        let listener = Arc::new(RecordingListener::default());

        let _guard_a = bus.register(Arc::clone(&listener) as Arc<dyn EventListener>);
        let _guard_b = bus.register(Arc::clone(&listener) as Arc<dyn EventListener>);

        bus.dispatch(&AgentEvent::AgentRequest {
            request_id: "req-1".to_string(),
        });
        assert_eq!(listener.count(), 2);
    }
}
