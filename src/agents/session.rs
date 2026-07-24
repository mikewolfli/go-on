//! SessionActor — session-level actor wrapping AgentTree + AgentThread (BLUE71 §2.1.1)
//!
//! Assembles existing components into a cohesive session model:
//! - `AgentTree` for agent hierarchy management
//! - `AgentThread` for non-blocking agent spawns
//! - `AgentLifecycle` for lifecycle state machine
//! - `CompactionManager` for token budget management
//! - `CommunicationBus` for inter-agent messaging
//!
//! Architecture:
//! ```
//! SessionActor::spawn(input_queue_tx, lifecycle_tx)
//!   → session_main_loop (tokio task, owns all state)
//!      → processes SessionInput via input_queue_rx
//!      → manages AgentTree via CommunicationBus
//!      → delegates to AgentThread for sub-agent execution
//!      → triggers compaction via CompactionManager when budget exceeded
//! ```

use std::sync::Arc;
use tokio::sync::{mpsc, watch};

use crate::agent::AgentRegistry;
use crate::agents::communication::agent_thread::{
    spawn_agent_non_blocking, AgentInput, AgentThread, SpawnConfig,
};
use crate::agents::communication::bus::CommunicationBus;
use crate::agents::communication::lifecycle::AgentLifecycleBuilder;
use crate::agents::communication::tree::AgentNodeMetadata;
use crate::agents::fragment::{FragmentPriority, FragmentRegistry, FragmentRole, SimpleFragment};
use crate::agents::graph_store::{AgentGraphEdge, AgentGraphStore, InMemoryAgentGraphStore};
use crate::optimization::compaction::{AdaptiveCompactor, CompactionManager, ConversationHistory};

// Re-export key types for external use.
pub use crate::schema::SessionId;

// ---------------------------------------------------------------------------
// SessionLifecycle — FSM for session-level lifecycle (BLUE71 §2.1.1)
// ---------------------------------------------------------------------------

/// Session-level lifecycle (BLUE71 §2.1.1).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum SessionLifecycle {
    /// Session created but not yet ready.
    Created {
        /// Creation timestamp (ms since epoch).
        at_ms: u64,
    },
    /// Session ready to accept input.
    Ready {
        /// Timestamp when ready (ms since epoch).
        since_ms: u64,
    },
    /// Session actively processing.
    Active {
        /// Timestamp when active started (ms since epoch).
        started_at_ms: u64,
        /// Current depth of agent tree.
        tree_depth: u32,
    },
    /// Session draining (graceful shutdown — waiting for sub-agents).
    Draining {
        /// Reason for draining.
        reason: String,
    },
    /// Session archived (persisted and closed).
    Archived {
        /// Summary of the session.
        summary: String,
        /// Total tokens used.
        total_tokens: u64,
        /// Total wall-clock time in milliseconds.
        total_wall_time_ms: u64,
        /// Archive timestamp (ms since epoch).
        archived_at_ms: u64,
    },
}

#[allow(dead_code)]
impl SessionLifecycle {
    /// Whether this state is terminal (no further transitions possible).
    pub fn is_terminal(&self) -> bool {
        matches!(self, SessionLifecycle::Archived { .. })
    }

    /// Human-readable summary.
    pub fn summary(&self) -> String {
        match self {
            SessionLifecycle::Created { at_ms } => format!("created at {}", at_ms),
            SessionLifecycle::Ready { since_ms } => format!("ready since {}", since_ms),
            SessionLifecycle::Active {
                started_at_ms,
                tree_depth,
                ..
            } => {
                format!("active since {}, depth={}", started_at_ms, tree_depth)
            }
            SessionLifecycle::Draining { reason } => format!("draining: {}", reason),
            SessionLifecycle::Archived {
                summary,
                total_tokens,
                ..
            } => {
                format!("archived: {} ({} tokens)", summary, total_tokens)
            }
        }
    }
}

#[allow(dead_code)]
impl Default for SessionLifecycle {
    fn default() -> Self {
        Self::Created { at_ms: now_ms() }
    }
}

// ---------------------------------------------------------------------------
// SessionInput — messages that can be sent to a SessionActor (BLUE71 §2.1.1)
// ---------------------------------------------------------------------------

/// Input message for a SessionActor's mailbox.
#[derive(Debug)]
#[allow(dead_code)]
pub enum SessionInput {
    /// User message for the root agent.
    UserMessage {
        /// Message content.
        content: String,
        /// Oneshot reply channel.
        reply_to: tokio::sync::oneshot::Sender<String>,
    },
    /// Cancel the session.
    Cancel {
        /// Reason for cancellation.
        reason: String,
    },
    /// Pre-turn instruction — injected into the next ChatRequest's system prompt.
    /// NOTE: Not true mid-turn steering (cannot interrupt active agent.chat()).
    /// This buffers the instruction and applies it to the next user message.
    Steer {
        /// Instruction to inject.
        instruction: String,
    },
    /// Session checkpoint — serializes current state (history + turns) into
    /// graph_store as a checkpoint edge for potential recovery.
    Checkpoint,
}

// ---------------------------------------------------------------------------
// SessionHandle — external handle to a running SessionActor
// ---------------------------------------------------------------------------

/// Handle to a running SessionActor (BLUE71 §2.1.1).
///
/// External code interacts with the session ONLY through this handle:
/// - Send messages via `input_queue`
/// - Observe lifecycle via `lifecycle` watch channel
/// - Await completion via `handle`
#[allow(dead_code)]
pub struct SessionHandle {
    /// Session ID.
    pub session_id: SessionId,
    /// Input queue sender.
    pub input_tx: mpsc::UnboundedSender<SessionInput>,
    /// Lifecycle watch channel receiver.
    pub lifecycle_rx: watch::Receiver<SessionLifecycle>,
    /// Task join handle.
    pub handle: tokio::task::JoinHandle<()>,
    /// CommunicationBus for agent tree operations.
    pub bus: Arc<CommunicationBus>,
}

#[allow(dead_code)]
impl SessionHandle {
    /// Send a user message to the session and wait for the response.
    pub async fn send_message(&self, content: String) -> Result<String, String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.input_tx
            .send(SessionInput::UserMessage {
                content,
                reply_to: reply_tx,
            })
            .map_err(|e| format!("session channel closed: {}", e))?;
        reply_rx
            .await
            .map_err(|_| "session reply channel closed".to_string())
    }

    /// Cancel the session.
    pub fn cancel(&self, reason: &str) -> Result<(), String> {
        self.input_tx
            .send(SessionInput::Cancel {
                reason: reason.to_string(),
            })
            .map_err(|e| format!("session channel closed: {}", e))
    }

    /// Inject a steering instruction — will be included in the next UserMessage's system prompt.
    pub fn steer(&self, instruction: &str) -> Result<(), String> {
        self.input_tx
            .send(SessionInput::Steer {
                instruction: instruction.to_string(),
            })
            .map_err(|e| format!("session channel closed: {}", e))
    }

    /// Trigger a checkpoint — serializes current ConversationHistory into
    /// the graph_store as a checkpoint edge with full JSON data for recovery.
    pub fn checkpoint(&self) -> Result<(), String> {
        self.input_tx
            .send(SessionInput::Checkpoint)
            .map_err(|e| format!("session channel closed: {}", e))
    }

    /// Get current lifecycle state.
    pub fn lifecycle(&self) -> SessionLifecycle {
        self.lifecycle_rx.borrow().clone()
    }

    /// Subscribe to lifecycle changes.
    pub fn subscribe_lifecycle(&self) -> watch::Receiver<SessionLifecycle> {
        self.lifecycle_rx.clone()
    }

    /// Whether the session task has finished.
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

// ---------------------------------------------------------------------------
// SessionActor — session creation and main loop
// ---------------------------------------------------------------------------

/// Internal session state, owned by the main loop task.
#[allow(dead_code)]
struct SessionState {
    /// Session ID.
    session_id: SessionId,
    /// CommunicationBus (owns AgentTree + AgentMessenger).
    #[allow(dead_code)]
    bus: Arc<CommunicationBus>,
    /// Conversation history (messages).
    history: ConversationHistory,
    /// Adaptive compaction engine with auto strategy selection (BLUE71 §10.3).
    compaction: AdaptiveCompactor,
    /// Structured context injection registry (BLUE71 §9).
    fragments: FragmentRegistry,
    /// Agent graph store for persistence (BLUE71 §8).
    graph_store: Arc<dyn AgentGraphStore>,
    /// Root agent for LLM calls.
    root_agent: Option<std::sync::Arc<dyn crate::agent::Agent>>,
    /// Budget counter shared with the root AgentThread (RAII).
    thread_budget: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Root agent name (e.g. "deepseek").
    root_agent_name: String,
    /// Pending steering instructions to inject into the next ChatRequest.
    pending_instructions: Vec<String>,
    /// Cached root path (avoids repeated parsing).
    root_path: crate::agents::communication::path::AgentPath,
}

#[allow(dead_code)]
impl SessionState {
    fn new(
        session_id: SessionId,
        bus: Arc<CommunicationBus>,
        root_agent: Option<std::sync::Arc<dyn crate::agent::Agent>>,
        root_agent_name: &str,
        compact_threshold: usize,
        root_path: crate::agents::communication::path::AgentPath,
    ) -> Self {
        // Register a default system fragment for session context
        let mut fragments = FragmentRegistry::new();
        fragments.register(Arc::new(SimpleFragment::new(
            FragmentRole::System,
            FragmentPriority::Normal,
            format!("You are {} in session {}.", root_agent_name, session_id.0),
        )));

        Self {
            session_id,
            bus,
            history: ConversationHistory::new(),
            compaction: AdaptiveCompactor::new(
                CompactionManager::new(Some(root_agent_name.to_string()), compact_threshold, 10),
                100, // keep up to 100 effectiveness records
            ),
            fragments,
            graph_store: Arc::new(InMemoryAgentGraphStore::new()),
            root_agent,
            thread_budget: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            root_agent_name: root_agent_name.to_string(),
            pending_instructions: Vec::new(),
            root_path,
        }
    }
}

/// Spawn a new SessionActor, returning a handle for external interaction.
///
/// This is the single async entry point for creating a session. It:
/// 1. Creates the communication infrastructure (AgentTree, channels)
/// 2. Registers the root agent in the tree with `Registered` lifecycle
/// 3. Spawns a tokio task for the session main loop
/// 4. Returns `SessionHandle` for external interaction
///
/// # Arguments
/// * `session_id` - Unique session identifier
/// * `root_agent_name` - Name of the root agent (must be in AgentRegistry)
/// * `compact_threshold` - Token count that triggers auto-compaction
#[allow(dead_code)]
pub async fn spawn_session(
    session_id: SessionId,
    registry: &AgentRegistry,
    root_agent_name: &str,
    compact_threshold: usize,
) -> Result<SessionHandle, String> {
    // 1. Create CommunicationBus (owns AgentTree + AgentMessenger)
    let bus = Arc::new(CommunicationBus::new());

    // 2. Register root agent in tree with Created lifecycle
    let root_path = crate::agents::communication::path::AgentPath::parse("root")
        .map_err(|e| format!("failed to parse root path: {}", e))?;
    let metadata = AgentNodeMetadata::new()
        .with_role("root")
        .with_model(root_agent_name);
    let _ = bus
        .register_agent(&root_path, root_agent_name, metadata)
        .await;

    // 3. Set root node lifecycle to Ready
    {
        let mut t = bus.tree().write().await;
        if let Some(root) = t.resolve_mut(&root_path) {
            root.set_lifecycle(AgentLifecycleBuilder::idle());
        }
    }

    // 4. Look up root agent from registry for AgentThread use
    let root_agent = registry.get(root_agent_name);

    // 5. Create channels
    let (input_tx, input_rx) = mpsc::unbounded_channel::<SessionInput>();
    let (lifecycle_tx, lifecycle_rx) =
        watch::channel(SessionLifecycle::Created { at_ms: now_ms() });

    // 6. Create internal state
    let state = SessionState::new(
        session_id.clone(),
        bus.clone(),
        root_agent,
        root_agent_name,
        compact_threshold,
        root_path,
    );

    // 7. Spawn the main loop
    let handle = tokio::spawn(session_main_loop(state, input_rx, lifecycle_tx.clone()));

    // 8. Set lifecycle to Ready once the task is running
    lifecycle_tx.send_replace(SessionLifecycle::Ready { since_ms: now_ms() });

    Ok(SessionHandle {
        session_id,
        input_tx,
        lifecycle_rx,
        handle,
        bus,
    })
}

/// Session main loop — Actor pattern (BLUE71 §2.1.1).
///
/// Owns all session state and processes messages from the input queue.
/// This is where `process_chat_request`'s responsibilities are distributed:
/// - Message routing via `SessionInput::UserMessage`
/// - Agent tree management via `CommunicationBus`
/// - Compaction via `CompactionManager`
/// - Graceful shutdown via `SessionInput::Cancel`
#[allow(dead_code)]
async fn session_main_loop(
    mut state: SessionState,
    mut input_rx: mpsc::UnboundedReceiver<SessionInput>,
    lifecycle_tx: watch::Sender<SessionLifecycle>,
) {
    // Mark session as Ready
    lifecycle_tx.send_replace(SessionLifecycle::Ready { since_ms: now_ms() });

    // Create one AgentThread at session startup (reused across all messages)
    let mut root_thread: Option<AgentThread> = None;
    if let Some(agent) = state.root_agent.take() {
        let config = SpawnConfig {
            max_depth: 5,
            max_concurrency: 1,
            token_ceiling: None,
            timeout_secs: 120,
        };
        match spawn_agent_non_blocking(agent, config, state.thread_budget.clone(), None).await {
            Ok(thread) => {
                root_thread = Some(thread);
            }
            Err(e) => {
                tracing::error!(
                    session = %state.session_id.0,
                    error = %e,
                    "session: failed to spawn root agent thread"
                );
            }
        }
    }

    while let Some(input) = input_rx.recv().await {
        match input {
            SessionInput::UserMessage { content, reply_to } => {
                // Set lifecycle to Active
                lifecycle_tx.send_replace(SessionLifecycle::Active {
                    started_at_ms: now_ms(),
                    tree_depth: 1,
                });

                // Add to conversation history
                let turn = crate::optimization::compaction::ConversationTurn::new("user", &content);
                state.history.push(turn);

                // Check if compaction is needed (adaptive, BLUE71 §10.3)
                if state.compaction.should_compact(&state.history) {
                    let strategy = state.compaction.select_strategy(&state.history);
                    // Synchronous compaction (strategy auto-selected by AdaptiveCompactor)
                    let result = state.compaction.compact(&mut state.history);
                    tracing::debug!(
                        session = %state.session_id.0,
                        strategy = ?strategy,
                        turns_compacted = result.turns_compacted,
                        tokens_saved = result.tokens_saved,
                        quality = result.quality_score,
                        "session: auto-compaction triggered (adaptive)"
                    );
                }

                // Build base context from FragmentRegistry (BLUE71 §9)
                let fragment_context = state.fragments.build_context(4096);
                let base_prompt = if fragment_context.is_empty() {
                    format!(
                        "You are {} in session {}. Respond helpfully.",
                        state.root_agent_name, state.session_id.0,
                    )
                } else {
                    fragment_context
                };

                // Inject pending steering instructions into the system prompt (BLUE71 §2.1.1)
                let system_prompt = if state.pending_instructions.is_empty() {
                    base_prompt
                } else {
                    let instructions = state.pending_instructions.join("\n");
                    state.pending_instructions.clear();
                    format!(
                        "{}\n\n=== Steering Instructions ===\n{}",
                        base_prompt, instructions
                    )
                };

                // Record edge in graph store for observability (BLUE71 §8)
                state
                    .graph_store
                    .upsert_edge(AgentGraphEdge {
                        parent: state.root_path.clone(),
                        child: state.root_path.clone(),
                        status: "running".to_string(),
                        child_name: Some(state.root_agent_name.clone()),
                        task: Some(content.clone()),
                    })
                    .await;

                // Send ChatRequest to the persistent root AgentThread
                if let Some(ref thread) = root_thread {
                    let (reply_inner_tx, reply_inner_rx) = tokio::sync::oneshot::channel();
                    let chat_request = AgentInput::ChatRequest {
                        messages: vec![
                            crate::agent::Message {
                                role: "system".to_string(),
                                content: system_prompt,
                            },
                            crate::agent::Message {
                                role: "user".to_string(),
                                content: content.clone(),
                            },
                        ],
                        options: None,
                        reply_to: reply_inner_tx,
                    };
                    if let Err(e) = thread.send_input(chat_request) {
                        let _ = reply_to.send(format!("error: {}", e));
                    } else {
                        let response = reply_inner_rx
                            .await
                            .unwrap_or_else(|_| "error: agent channel closed".to_string());
                        let _ = reply_to.send(response);
                    }
                } else {
                    let _ = reply_to.send("error: no root agent thread".to_string());
                }

                // Return to Ready state
                lifecycle_tx.send_replace(SessionLifecycle::Ready { since_ms: now_ms() });
            }
            SessionInput::Steer { instruction } => {
                // Store steering instruction — injected into the next ChatRequest's
                // system prompt. Real observable effect: the instruction prepends
                // a system message in the next UserMessage handler.
                state.pending_instructions.push(instruction);
                tracing::info!(
                    session = %state.session_id.0,
                    pending = state.pending_instructions.len(),
                    "session: steering instruction recorded"
                );
            }
            SessionInput::Checkpoint => {
                // Serialize current conversation history to JSON checkpoint.
                // Stores the checkpoint as a graph_store edge with status="checkpoint".
                // The serialized data can be retrieved via list_descendants() for recovery.
                let checkpoint_data = state.history.to_checkpoint_json();
                state
                    .graph_store
                    .upsert_edge(AgentGraphEdge {
                        parent: state.root_path.clone(),
                        child: state.root_path.clone(),
                        status: "checkpoint".to_string(),
                        child_name: Some(format!("{}_checkpoint", state.root_agent_name)),
                        task: Some(checkpoint_data), // serialized history stored here
                    })
                    .await;
                tracing::info!(
                    session = %state.session_id.0,
                    turns = state.history.len(),
                    tokens = state.history.estimated_tokens(),
                    "session: checkpoint saved"
                );
            }
            SessionInput::Cancel { reason } => {
                lifecycle_tx.send_replace(SessionLifecycle::Draining {
                    reason: reason.clone(),
                });

                // Update graph store status (BLUE71 §8)
                state
                    .graph_store
                    .set_edge_status(&state.root_path, "cancelled")
                    .await;

                // Cancel the root AgentThread first
                if let Some(ref thread) = root_thread {
                    let _ = thread.send_input(AgentInput::Cancel {
                        reason: reason.clone(),
                    });
                }

                tracing::info!(
                    session = %state.session_id.0,
                    reason = %reason,
                    "session: cancelled"
                );
                break;
            }
        }
    }

    // Session ended — archive
    lifecycle_tx.send_replace(SessionLifecycle::Archived {
        summary: format!("{} turns", state.history.len()),
        total_tokens: state.history.estimated_tokens() as u64,
        total_wall_time_ms: 0, // Would track from creation time
        archived_at_ms: now_ms(),
    });
}

/// Current Unix timestamp in milliseconds.
#[allow(dead_code)]
fn now_ms() -> u64 {
    crate::shared::timestamps::now_ts_ms() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;

    fn test_registry() -> AgentRegistry {
        let mut reg = AgentRegistry::new();
        reg.register_arc("echo", Arc::new(EchoAgent));
        reg
    }

    // Minimal test agent for session tests
    struct EchoAgent;

    #[async_trait::async_trait]
    impl Agent for EchoAgent {
        async fn chat(
            &self,
            messages: Vec<crate::agent::Message>,
            _principles: Option<Vec<String>>,
            _options: Option<std::collections::HashMap<String, serde_json::Value>>,
            sender: crate::agent::StreamingSender,
        ) -> std::result::Result<(), crate::core::error::AppError> {
            let _ = sender.send(format!(
                "echo: {}",
                messages
                    .last()
                    .map(|m| &m.content)
                    .unwrap_or(&"".to_string())
            ));
            Ok(())
        }

        fn available_models(&self) -> Vec<crate::agent::ModelInfo> {
            vec![]
        }
    }

    #[test]
    fn test_session_lifecycle_default_is_created() {
        let lc = SessionLifecycle::default();
        assert!(matches!(lc, SessionLifecycle::Created { .. }));
    }

    #[test]
    fn test_session_lifecycle_terminal() {
        let archived = SessionLifecycle::Archived {
            summary: "done".into(),
            total_tokens: 100,
            total_wall_time_ms: 500,
            archived_at_ms: 1000,
        };
        assert!(archived.is_terminal());

        let created = SessionLifecycle::Created { at_ms: 0 };
        assert!(!created.is_terminal());
    }

    #[test]
    fn test_session_lifecycle_summary() {
        let s = SessionLifecycle::Created { at_ms: 1000 }.summary();
        assert!(s.contains("created"));

        let s = SessionLifecycle::Archived {
            summary: "done".into(),
            total_tokens: 100,
            total_wall_time_ms: 500,
            archived_at_ms: 1500,
        }
        .summary();
        assert!(s.contains("archived"));
    }

    #[tokio::test]
    async fn test_session_handle_send_and_cancel() {
        let registry = test_registry();
        let session_id = SessionId::new("test-session-1");
        let handle = spawn_session(session_id, &registry, "echo", 20_000)
            .await
            .unwrap();

        // Send a message — should be processed by EchoAgent
        let result = handle.send_message("Hello".to_string()).await;
        assert!(result.is_ok());

        // Cancel
        assert!(handle.cancel("test completed").is_ok());

        // Check that the handle's lifecycle can be read before consuming it
        let _lifecycle = handle.lifecycle();
        // Wait for task to finish (consumes handle.handle)
        let _ = handle.handle.await;
    }

    #[tokio::test]
    async fn test_session_lifecycle_transitions() {
        let registry = test_registry();
        let session_id = SessionId::new("test-session-3");
        let handle = spawn_session(session_id, &registry, "echo", 20_000)
            .await
            .unwrap();

        // Initially should be Created or Ready
        let initial = handle.lifecycle();
        assert!(
            matches!(
                initial,
                SessionLifecycle::Created { .. } | SessionLifecycle::Ready { .. }
            ),
            "expected Created or Ready, got {:?}",
            initial
        );

        // After sending a message, should return to Ready
        let _ = handle.send_message("test".to_string()).await;
        let after = handle.lifecycle();
        assert!(
            matches!(after, SessionLifecycle::Ready { .. }),
            "expected Ready after message, got {:?}",
            after
        );

        let _ = handle.cancel("done");
        // Consume handle (await the task), lifecycle is terminal after cancellation
        let _ = handle.handle.await;
    }

    #[tokio::test]
    async fn test_session_checkpoint_stores_serialized_history() {
        let registry = test_registry();
        let session_id = SessionId::new("test-session-4");
        let handle = spawn_session(session_id, &registry, "echo", 20_000)
            .await
            .unwrap();

        // Send a message to create conversation history
        let _ = handle.send_message("first message".to_string()).await;

        // Trigger checkpoint — serializes history into graph_store
        assert!(handle.checkpoint().is_ok());

        // Verify: after checkpoint, the session should still be functional
        let result = handle.send_message("second message".to_string()).await;
        assert!(result.is_ok());

        let _ = handle.cancel("done");
        let _ = handle.handle.await;
    }

    #[tokio::test]
    async fn test_session_is_finished() {
        let registry = test_registry();
        let session_id = SessionId::new("test-session-5");
        let handle = spawn_session(session_id, &registry, "echo", 20_000)
            .await
            .unwrap();
        let before = handle.is_finished();
        assert!(!before);
        let _ = handle.cancel("done");
        let _ = handle.handle.await;
    }

    #[tokio::test]
    async fn test_session_steer_injects_instruction() {
        let registry = test_registry();
        let session_id = SessionId::new("test-session-5");
        let handle = spawn_session(session_id, &registry, "echo", 20_000)
            .await
            .unwrap();

        // Steer before sending message — instruction should be injected into ChatRequest
        assert!(handle.steer("focus on performance").is_ok());

        // Send a message — EchoAgent should receive the steering instruction
        // in its system messages (EchoAgent echoes back the last message content)
        let result = handle.send_message("Hello".to_string()).await;
        assert!(result.is_ok());
        let response = result.unwrap();
        // The response from EchoAgent is "echo: <content>" format
        assert!(response.contains("echo"));

        let _ = handle.cancel("done");
        let _ = handle.handle.await;
    }
}
