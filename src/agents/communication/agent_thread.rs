//! AgentThread — non-blocking agent spawn model (BLUE71 §4)
//!
//! Provides the `AgentThread` struct for asynchronous agent spawns,
//! `SpawnGuard` for RAII-protected concurrency slot management, and
//! `AgentStatus` for lifecycle state tracking via watch channel.
//!
//! Design (BLUE71 §4.3):
//! - `spawn_agent_non_blocking()` returns immediately with an `AgentThread` handle.
//! - `agent_main_loop()` runs as a tokio task, consuming from an mpsc input queue.
//! - `SpawnGuard` ensures concurrency slots are released on Drop (panic-safe).
//! - Status changes propagate via `watch::Sender<AgentStatus>` (zero polling).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::agents::agent::{Agent, Message, StreamingSender};

/// Unique thread ID counter.
static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);

/// Generate a new unique thread ID.
pub fn new_thread_id() -> u64 {
    NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed)
}

// ── AgentStatus — lifecycle state visible via watch channel ────────────

/// Observable status of an AgentThread (BLUE71 §4.3).
#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatus {
    /// Thread created but not yet started.
    PendingInit,
    /// Actively processing messages.
    Running,
    /// Completed successfully with a result.
    Completed { result: String },
    /// Terminated with an error.
    Errored { error: String },
    /// Cancelled by user or parent.
    Cancelled { reason: String },
}

impl AgentStatus {
    /// Whether this status represents a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            AgentStatus::Completed { .. }
                | AgentStatus::Errored { .. }
                | AgentStatus::Cancelled { .. }
        )
    }
}

// ── AgentInput — messages that can be sent to an AgentThread's queue ──

/// Input message for an AgentThread's mailbox (BLUE71 §4.3).
#[derive(Debug)]
pub enum AgentInput {
    /// User message with a oneshot reply channel.
    UserMessage {
        content: String,
        reply_to: tokio::sync::oneshot::Sender<String>,
    },

    /// Full chat request with custom messages, options, and reply channel.
    /// Used by SpawnAgentTool integration to pass system prompts + user task.
    ChatRequest {
        messages: Vec<Message>,
        options: Option<std::collections::HashMap<String, serde_json::Value>>,
        reply_to: tokio::sync::oneshot::Sender<String>,
    },

    /// Cancel the agent with a reason.
    Cancel { reason: String },
}

// ── SpawnConfig — parameters for spawning an AgentThread ──────────────

/// Configuration for spawning an AgentThread (BLUE71 §4.3).
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// Maximum depth in the agent tree.
    pub max_depth: u32,
    /// Maximum concurrent children.
    pub max_concurrency: usize,
    /// Token ceiling for this sub-tree.
    pub token_ceiling: Option<u64>,
    /// Timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for SpawnConfig {
    fn default() -> Self {
        Self {
            max_depth: 10,
            max_concurrency: 8,
            token_ceiling: None,
            timeout_secs: 120,
        }
    }
}

// ── AgentThread — non-blocking agent execution handle ─────────────────

/// A handle to a running agent thread (BLUE71 §4.3).
///
/// Created by `spawn_agent_non_blocking()`. The caller can:
/// - Send messages via `input_queue`
/// - Observe status via `status.watch::Receiver`
/// - Await completion via `handle` (JoinHandle)
///
/// The parent is NOT blocked — it can continue processing while
/// the child agent runs independently.
pub struct AgentThread {
    /// Unique thread ID.
    pub thread_id: u64,
    /// Agent instance.
    pub agent: Arc<dyn Agent>,
    /// Spawn configuration.
    pub config: SpawnConfig,
    /// Status watch channel sender.
    pub status_tx: watch::Sender<AgentStatus>,
    /// Input queue sender.
    pub input_tx: mpsc::UnboundedSender<AgentInput>,
    /// Task join handle.
    pub handle: JoinHandle<()>,
}

impl std::fmt::Debug for AgentThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentThread")
            .field("thread_id", &self.thread_id)
            .field("config", &self.config)
            .field("is_finished", &self.is_finished())
            .finish()
    }
}

impl AgentThread {
    /// Send a message to this agent's input queue.
    pub fn send_input(&self, input: AgentInput) -> Result<(), String> {
        self.input_tx
            .send(input)
            .map_err(|e| format!("failed to send input: {}", e))
    }

    /// Get a status receiver for this agent.
    pub fn subscribe(&self) -> watch::Receiver<AgentStatus> {
        self.status_tx.subscribe()
    }

    /// Check if the agent has terminated.
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

// ── SpawnGuard — RAII concurrency slot reservation (BLUE71 §5) ────────

/// RAII guard that reserves a concurrency slot (BLUE71 §5.2).
///
/// On creation, atomically increments the budget counter.
/// On Drop (even during panic), decrements the counter UNLESS
/// `commit()` was called (ownership transferred to the running thread).
///
/// This prevents concurrency slot leaks when agent spawns fail
/// partway through initialization.
#[derive(Debug)]
pub struct SpawnGuard {
    /// Shared atomic budget counter.
    budget: Arc<AtomicU64>,
    /// Maximum concurrency.
    max: u64,
    /// Whether the slot has been committed (ownership transferred).
    committed: bool,
}

impl SpawnGuard {
    /// Try to reserve a slot in the budget.
    ///
    /// Returns `Err(SpawnError::CapacityExceeded)` if at capacity.
    pub fn try_reserve(budget: Arc<AtomicU64>, max: u64) -> Result<Self, SpawnError> {
        let current = budget.fetch_add(1, Ordering::AcqRel);
        if current >= max {
            // Rollback: decrement the counter we just incremented.
            budget.fetch_sub(1, Ordering::AcqRel);
            return Err(SpawnError::CapacityExceeded { current, max });
        }
        Ok(Self {
            budget,
            max,
            committed: false,
        })
    }

    /// Commit the reservation — ownership transfers to the running task.
    ///
    /// After commit, the slot is NOT released on Drop; the running task
    /// is responsible for releasing it via `SpawnGuard::release_slot()`.
    pub fn commit(mut self) {
        self.committed = true;
    }

    /// Release a committed slot (called by the running task on completion).
    pub fn release_slot(budget: &Arc<AtomicU64>) {
        budget.fetch_sub(1, Ordering::AcqRel);
    }

    /// Get the current budget value.
    pub fn current_usage(budget: &Arc<AtomicU64>) -> u64 {
        budget.load(Ordering::Acquire)
    }
}

impl Drop for SpawnGuard {
    fn drop(&mut self) {
        if !self.committed {
            // Slot was reserved but not committed — release on panic/drop.
            self.budget.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

// ── SpawnError — errors that can occur during agent spawn ─────────────

/// Errors that can occur when spawning an AgentThread.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SpawnError {
    /// Concurrency capacity exceeded.
    #[error("capacity exceeded: current={current} max={max}")]
    CapacityExceeded {
        /// Current usage.
        current: u64,
        /// Maximum allowed.
        max: u64,
    },
    /// Agent not found in registry.
    #[error("agent not found: {0}")]
    AgentNotFound(String),
    /// General spawn failure.
    #[error("spawn failed: {0}")]
    Other(String),
}

// ── Non-blocking spawn ───────────────────────────────────────────────

/// Spawn an agent in a non-blocking fashion (BLUE71 §4.3).
///
/// Returns immediately with an `AgentThread` handle. The agent runs
/// independently, consuming messages from its input queue.
///
/// # Arguments
/// * `agent` - The agent instance to run.
/// * `config` - Spawn configuration (depth, concurrency, timeout).
/// * `budget` - Shared atomic budget counter for RAII slot tracking.
/// * `initial_input` - Optional initial input to enqueue.
///
/// # Returns
/// * `Ok(AgentThread)` - Handle to the running agent.
/// * `Err(SpawnError)` - If capacity is exceeded or spawn fails.
pub async fn spawn_agent_non_blocking(
    agent: Arc<dyn Agent>,
    config: SpawnConfig,
    budget: Arc<AtomicU64>,
    initial_input: Option<AgentInput>,
) -> Result<AgentThread, SpawnError> {
    // 1. RAII: Reserve a concurrency slot.
    let guard = SpawnGuard::try_reserve(budget.clone(), config.max_concurrency as u64)?;

    // 2. Create channels.
    let (input_tx, input_rx) = mpsc::unbounded_channel::<AgentInput>();
    let (status_tx, _) = watch::channel(AgentStatus::PendingInit);

    let thread_id = new_thread_id();

    // 3. Enqueue initial input if provided.
    if let Some(input) = initial_input {
        let _ = input_tx.send(input);
    }

    // 4. Clone references for the spawned task.
    let status_tx_clone = status_tx.clone();
    let budget_clone = budget.clone();

    // 5. Commit the guard — ownership transfers to the spawned task.
    guard.commit();

    // 6. Clone agent for the spawned task (AgentThread handle keeps its own ref).
    let agent_for_loop = agent.clone();

    // 7. Spawn the main loop as a tokio task.
    let handle = tokio::spawn(async move {
        agent_main_loop(
            thread_id,
            agent_for_loop,
            input_rx,
            status_tx_clone,
            budget_clone,
        )
        .await;
    });

    Ok(AgentThread {
        thread_id,
        agent,
        config,
        status_tx,
        input_tx,
        handle,
    })
}

// ── Agent main loop — Actor pattern ───────────────────────────────────

/// The main loop for an agent thread (Actor pattern, BLUE71 §4.3).
///
/// Continuously consumes from the input queue in a persistent loop:
/// - Each `UserMessage` or `ChatRequest` is processed and replied to.
/// - The loop continues after each message (persistent agent).
/// - A `Cancel` message terminates the loop.
/// - Channel closure (all senders dropped) terminates the loop.
async fn agent_main_loop(
    thread_id: u64,
    agent: Arc<dyn Agent>,
    mut input_rx: mpsc::UnboundedReceiver<AgentInput>,
    status_tx: watch::Sender<AgentStatus>,
    budget: Arc<AtomicU64>,
) {
    status_tx.send_replace(AgentStatus::Running);

    while let Some(input) = input_rx.recv().await {
        match input {
            AgentInput::UserMessage { content, reply_to } => {
                let messages = vec![Message {
                    role: "user".to_string(),
                    content: content.clone(),
                }];

                // Create a streaming channel for the agent response.
                let (token_tx, mut token_rx) = mpsc::unbounded_channel::<String>();
                let sender = StreamingSender::new(token_tx);

                let result = agent.chat(messages, None, None, sender).await;
                match result {
                    Ok(()) => {
                        // Collect all streamed tokens.
                        let mut response = String::new();
                        while let Some(token) = token_rx.recv().await {
                            response.push_str(&token);
                        }
                        let _ = reply_to.send(response);
                    }
                    Err(e) => {
                        let error = e.to_string();
                        status_tx.send_replace(AgentStatus::Errored {
                            error: error.clone(),
                        });
                        let _ = reply_to.send(format!("error: {}", error));
                    }
                }
                // Continue processing — this is a persistent agent loop
            }
            AgentInput::ChatRequest {
                messages,
                options,
                reply_to,
            } => {
                // Create a streaming channel for the agent response.
                let (token_tx, mut token_rx) = mpsc::unbounded_channel::<String>();
                let sender = StreamingSender::new(token_tx);

                let result = agent.chat(messages, None, options, sender).await;
                match result {
                    Ok(()) => {
                        // Collect all streamed tokens.
                        let mut response = String::new();
                        while let Some(token) = token_rx.recv().await {
                            response.push_str(&token);
                        }
                        let _ = reply_to.send(response);
                    }
                    Err(e) => {
                        let error = e.to_string();
                        status_tx.send_replace(AgentStatus::Errored {
                            error: error.clone(),
                        });
                        let _ = reply_to.send(format!("error: {}", error));
                    }
                }
                // Continue processing — this is a persistent agent loop
            }
            AgentInput::Cancel { reason } => {
                status_tx.send_replace(AgentStatus::Cancelled { reason });
                return;
            }
        }
    }

    // Agent completed or channel closed — release the budget slot.
    SpawnGuard::release_slot(&budget);

    // If we exit without setting a terminal status, mark as cancelled.
    let current = status_tx.borrow().clone();
    if !current.is_terminal() {
        status_tx.send_replace(AgentStatus::Cancelled {
            reason: "input channel closed".to_string(),
        });
    }
}

// ── Wait for completion helper ────────────────────────────────────────

/// Wait for an AgentThread to reach a terminal state (event-driven, zero polling).
///
/// Uses the watch channel's `changed()` method to sleep until the next
/// status update. No busy-waiting or polling.
///
/// # Arguments
/// * `rx` - The status receiver obtained from `AgentThread::subscribe()`.
/// * `timeout` - Maximum time to wait.
///
/// # Returns
/// * `Ok(AgentStatus)` - The terminal status.
/// * `Err(WaitError)` - If timeout or channel closed.
pub async fn wait_for_completion(
    rx: &mut watch::Receiver<AgentStatus>,
    timeout: std::time::Duration,
) -> Result<AgentStatus, WaitError> {
    tokio::time::timeout(timeout, async {
        loop {
            let status = rx.borrow_and_update().clone();
            if status.is_terminal() {
                return Ok(status);
            }
            rx.changed().await.map_err(|_| WaitError::ChannelClosed)?;
        }
    })
    .await
    .map_err(|_| WaitError::Timeout)?
}

/// Errors that can occur while waiting for agent completion.
#[derive(Debug, Clone, thiserror::Error)]
pub enum WaitError {
    /// Timed out waiting for completion.
    #[error("timeout waiting for agent completion")]
    Timeout,
    /// The watch channel was closed (agent dropped).
    #[error("agent channel closed")]
    ChannelClosed,
}

// ── AgentThreadManager — manages all running threads ──────────────────

/// Manages all running AgentThreads with a global budget.
#[derive(Debug)]
pub struct AgentThreadManager {
    /// Global concurrency budget (atomic for RAII).
    global_budget: Arc<AtomicU64>,
    /// Maximum global concurrency.
    max_global_concurrency: u64,
}

impl AgentThreadManager {
    /// Create a new AgentThreadManager.
    pub fn new(max_global_concurrency: u64) -> Self {
        Self {
            global_budget: Arc::new(AtomicU64::new(0)),
            max_global_concurrency,
        }
    }

    /// Get the global budget counter (for RAII guard).
    pub fn budget(&self) -> Arc<AtomicU64> {
        self.global_budget.clone()
    }

    /// Get maximum global concurrency.
    pub fn max_concurrency(&self) -> u64 {
        self.max_global_concurrency
    }

    /// Get current global usage.
    pub fn current_usage(&self) -> u64 {
        SpawnGuard::current_usage(&self.global_budget)
    }

    /// Spawn an agent non-blocking under this manager's budget.
    pub async fn spawn(
        &self,
        agent: Arc<dyn Agent>,
        config: SpawnConfig,
        initial_input: Option<AgentInput>,
    ) -> Result<AgentThread, SpawnError> {
        spawn_agent_non_blocking(agent, config, self.global_budget.clone(), initial_input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::agent::{Agent, ModelInfo};
    use crate::core::error::Result as AppResult;
    use serde_json::Value;
    use std::collections::HashMap;

    /// A simple test agent that completes successfully.
    struct EchoAgent;

    #[async_trait::async_trait]
    impl Agent for EchoAgent {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _principles: Option<Vec<String>>,
            _options: Option<HashMap<String, Value>>,
            _sender: StreamingSender,
        ) -> AppResult<()> {
            Ok(())
        }

        fn available_models(&self) -> Vec<ModelInfo> {
            vec![ModelInfo {
                id: "echo".to_string(),
                name: "Echo Agent".to_string(),
                description: "Test echo agent".to_string(),
                is_default: true,
                capabilities: vec!["chat".to_string()],
                context_window: None,
            }]
        }
    }

    #[tokio::test]
    async fn test_spawn_agent_non_blocking() {
        let agent = Arc::new(EchoAgent);
        let budget = Arc::new(AtomicU64::new(0));
        let config = SpawnConfig::default();

        let thread = spawn_agent_non_blocking(agent, config, budget, None)
            .await
            .unwrap();
        assert_eq!(thread.status_tx.borrow().clone(), AgentStatus::PendingInit);
        assert!(!thread.is_finished());
    }

    #[tokio::test]
    async fn test_spawn_guard_capacity_exceeded() {
        let budget = Arc::new(AtomicU64::new(0)); // start at 0 used
        let max: u64 = 2;

        // First two should succeed.
        let g1 = SpawnGuard::try_reserve(budget.clone(), max).unwrap();
        assert_eq!(SpawnGuard::current_usage(&budget), 1);
        let g2 = SpawnGuard::try_reserve(budget.clone(), max).unwrap();
        assert_eq!(SpawnGuard::current_usage(&budget), 2);

        // Third should fail.
        let g3 = SpawnGuard::try_reserve(budget.clone(), max);
        assert!(g3.is_err());
        assert!(matches!(
            g3.unwrap_err(),
            SpawnError::CapacityExceeded { .. }
        ));

        // Drop g2 — slot released.
        drop(g2);

        // Now we can reserve again.
        let g4 = SpawnGuard::try_reserve(budget.clone(), max);
        assert!(g4.is_ok());
        assert_eq!(SpawnGuard::current_usage(&budget), 2); // g1 + g4

        drop(g1);
        drop(g4);
        assert_eq!(SpawnGuard::current_usage(&budget), 0);
    }

    #[tokio::test]
    async fn test_spawn_guard_commit_transfers_ownership() {
        let budget = Arc::new(AtomicU64::new(0));

        let guard = SpawnGuard::try_reserve(budget.clone(), 2).unwrap();
        assert_eq!(SpawnGuard::current_usage(&budget), 1);

        guard.commit();

        // Budget stays at 1 even after guard drop (ownership transferred).
        assert_eq!(SpawnGuard::current_usage(&budget), 1);

        // Manual release.
        SpawnGuard::release_slot(&budget);
        assert_eq!(SpawnGuard::current_usage(&budget), 0);
    }

    #[tokio::test]
    async fn test_agent_status_terminal() {
        assert!(AgentStatus::Completed {
            result: "ok".into()
        }
        .is_terminal());
        assert!(AgentStatus::Errored {
            error: "err".into()
        }
        .is_terminal());
        assert!(AgentStatus::Cancelled {
            reason: "timeout".into()
        }
        .is_terminal());
        assert!(!AgentStatus::PendingInit.is_terminal());
        assert!(!AgentStatus::Running.is_terminal());
    }

    #[tokio::test]
    async fn test_wait_for_completion_timeout() {
        let (_tx, mut rx) = watch::channel(AgentStatus::Running);

        let result = wait_for_completion(&mut rx, std::time::Duration::from_millis(50)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), WaitError::Timeout));
    }

    #[tokio::test]
    async fn test_wait_for_completion_success() {
        let (tx, mut rx) = watch::channel(AgentStatus::Running);

        // Simulate completion in another task.
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            tx.send_replace(AgentStatus::Completed {
                result: "done".into(),
            });
        });

        let result = wait_for_completion(&mut rx, std::time::Duration::from_secs(5)).await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            AgentStatus::Completed {
                result: "done".into()
            }
        );
    }

    #[tokio::test]
    async fn test_agent_thread_manager() {
        let manager = AgentThreadManager::new(10);
        assert_eq!(manager.max_concurrency(), 10);
        assert_eq!(manager.current_usage(), 0);
    }

    #[tokio::test]
    async fn test_chat_request_variant() {
        let agent = Arc::new(EchoAgent);
        let budget = Arc::new(AtomicU64::new(0));
        let config = SpawnConfig::default();

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let input = AgentInput::ChatRequest {
            messages: vec![
                Message {
                    role: "system".to_string(),
                    content: "You are a helpful assistant.".to_string(),
                },
                Message {
                    role: "user".to_string(),
                    content: "Say hello".to_string(),
                },
            ],
            options: None,
            reply_to: reply_tx,
        };

        let thread = spawn_agent_non_blocking(agent, config, budget, Some(input))
            .await
            .unwrap();

        let status =
            wait_for_completion(&mut thread.subscribe(), std::time::Duration::from_secs(5))
                .await
                .unwrap();
        assert!(matches!(status, AgentStatus::Completed { .. }));

        let reply = reply_rx.await.unwrap();
        assert!(!reply.is_empty());
    }
}
