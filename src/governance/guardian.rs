//! GuardianReviewer — independent model review for action safety (BLUE71 §11)
//!
//! Uses a separate model instance (typically a cheaper/faster model) to review
//! tool actions before execution. Fail-closed: any error, timeout, or parse failure
//! results in a Deny decision. A circuit breaker prevents repeated denials from
//! overwhelming the review system.
//!
//! Architecture:
//! - `GuardianReviewer` — the main review orchestrator
//! - `GuardianCircuitBreaker` — prevents review cycles on persistent denials
//! - `GuardianDecision` — structured review outcome
//!
//! Integration: plugs into the governance tool chain via `check_tool_action()`.
//! Feed-closed: GuardianReviewer::new() returns None when no review agent is configured,
//! allowing callers to skip review when the feature is not available.
//!
//! Convenience: `GuardianReviewer::from_registry()` looks up an agent by name from
//! an `AgentRegistry`, returning `None` if the agent is not found.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::agent::{Agent, AgentRegistry};
use crate::orchestration::tool::ToolInput;

// ---------------------------------------------------------------------------
// GuardianDecision — structured review outcome
// ---------------------------------------------------------------------------

/// Decision returned by the GuardianReviewer.
#[derive(Debug, Clone, PartialEq)]
pub enum GuardianDecision {
    /// Action is allowed to proceed.
    Allow {
        /// Confidence level (0.0 - 1.0).
        confidence: f64,
    },
    /// Action is denied.
    Deny {
        /// Reason for denial.
        reason: String,
    },
    /// Escalate to human (circuit breaker tripped or uncertainty).
    EscalateToUser {
        /// Reason for escalation.
        reason: String,
    },
}

impl GuardianDecision {
    /// Whether the action is allowed.
    pub fn is_allowed(&self) -> bool {
        matches!(self, GuardianDecision::Allow { .. })
    }
}

// ---------------------------------------------------------------------------
// GuardianCircuitBreaker — prevents repeated denials from overwhelming system
// ---------------------------------------------------------------------------

/// Circuit breaker for the GuardianReviewer (BLUE71 §11.2).
///
/// Tracks consecutive denials and recent denial rate. When thresholds are
/// exceeded, `should_skip_review()` returns true, forcing escalation.
pub struct GuardianCircuitBreaker {
    /// Maximum consecutive denials before tripping.
    max_consecutive_denials: u32,
    /// Maximum recent denials in the window.
    max_recent_denials: u32,
    /// Recent decisions ring buffer (true = deny).
    denials: VecDeque<bool>,
    /// Current consecutive denial count.
    consecutive_denials: u32,
}

impl GuardianCircuitBreaker {
    /// Create a new circuit breaker with the given thresholds.
    pub fn new(max_consecutive_denials: u32, max_recent_denials: u32, window_size: usize) -> Self {
        Self {
            max_consecutive_denials,
            max_recent_denials,
            denials: VecDeque::with_capacity(window_size),
            consecutive_denials: 0,
        }
    }

    /// Whether review should be skipped (circuit breaker tripped).
    pub fn should_skip_review(&self) -> bool {
        if self.consecutive_denials >= self.max_consecutive_denials {
            return true;
        }
        let recent_denials: u32 = self.denials.iter().map(|&d| d as u32).sum();
        recent_denials >= self.max_recent_denials
    }

    /// Record a decision outcome.
    pub fn record_decision(&mut self, denied: bool) {
        if denied {
            self.consecutive_denials += 1;
        } else {
            self.consecutive_denials = 0;
        }
        if self.denials.len() >= self.denials.capacity() {
            self.denials.pop_front();
        }
        self.denials.push_back(denied);
    }
}

impl Default for GuardianCircuitBreaker {
    fn default() -> Self {
        Self::new(3, 10, 50)
    }
}

// ---------------------------------------------------------------------------
// GuardianReviewer — main review orchestrator
// ---------------------------------------------------------------------------

/// Independent model reviewer for tool action safety (BLUE71 §11).
///
/// Uses a separate agent (typically a cheaper, faster model) to review
/// tool actions before they are executed. Fail-closed: any error results
/// in a Deny decision.
pub struct GuardianReviewer {
    /// The review agent — typically a cheap/fast model.
    review_agent: Arc<dyn Agent>,
    /// Circuit breaker state (behind Mutex for interior mutability).
    circuit_breaker: Mutex<GuardianCircuitBreaker>,
    /// Maximum time to wait for a review response.
    timeout: Duration,
}

impl GuardianReviewer {
    /// Create a new GuardianReviewer.
    ///
    /// `review_agent` should be a cheap/fast model used for review.
    /// `timeout` defaults to 90 seconds if not specified.
    pub fn new(review_agent: Arc<dyn Agent>, timeout: Option<Duration>) -> Self {
        Self {
            review_agent,
            circuit_breaker: Mutex::new(GuardianCircuitBreaker::default()),
            timeout: timeout.unwrap_or(Duration::from_secs(90)),
        }
    }

    /// Create a new GuardianReviewer from an AgentRegistry lookup.
    ///
    /// Looks up `agent_name` in the registry. Returns `None` if the agent
    /// is not found, enabling graceful fallback when the review agent
    /// is not configured.
    pub fn from_registry(
        registry: &AgentRegistry,
        agent_name: &str,
        timeout: Option<Duration>,
    ) -> Option<Self> {
        registry
            .get(agent_name)
            .map(|agent| Self::new(agent, timeout))
    }

    /// Create a new GuardianReviewer with custom circuit breaker config.
    pub fn with_circuit_breaker(
        review_agent: Arc<dyn Agent>,
        max_consecutive_denials: u32,
        max_recent_denials: u32,
        window_size: usize,
        timeout: Option<Duration>,
    ) -> Self {
        Self {
            review_agent,
            circuit_breaker: Mutex::new(GuardianCircuitBreaker::new(
                max_consecutive_denials,
                max_recent_denials,
                window_size,
            )),
            timeout: timeout.unwrap_or(Duration::from_secs(90)),
        }
    }

    /// Get a reference to the review agent.
    pub fn review_agent(&self) -> &Arc<dyn Agent> {
        &self.review_agent
    }

    /// Get circuit breaker state for diagnostics.
    pub async fn circuit_breaker_status(&self) -> GuardianBreakerStatus {
        let cb = self.circuit_breaker.lock().await;
        GuardianBreakerStatus {
            consecutive_denials: cb.consecutive_denials,
            max_consecutive_denials: cb.max_consecutive_denials,
            recent_denials: cb.denials.iter().map(|&d| d as u32).sum::<u32>(),
            max_recent_denials: cb.max_recent_denials,
            tripped: cb.should_skip_review(),
        }
    }

    /// Review a tool action using the independent model (BLUE71 §11.2).
    ///
    /// `tool_name` is the name of the tool being reviewed.
    /// `action` is the tool input parameters.
    /// `summary` is a brief conversation context summary.
    ///
    /// Builds a review prompt, sends it to the review agent, and parses response.
    /// Fail-closed: timeout, error, or parse failure → Deny.
    pub async fn review_action(
        &self,
        tool_name: &str,
        action: &ToolInput,
        summary: &str,
    ) -> GuardianDecision {
        // Check circuit breaker first
        {
            let mut cb = self.circuit_breaker.lock().await;
            if cb.should_skip_review() {
                warn!(
                    tool = tool_name,
                    consecutive = cb.consecutive_denials,
                    "Guardian: circuit breaker tripped, escalating to user"
                );
                cb.record_decision(false); // escalation counts as not denied
                return GuardianDecision::EscalateToUser {
                    reason: format!(
                        "circuit breaker tripped ({} consecutive denials)",
                        cb.consecutive_denials
                    ),
                };
            }
        }

        // Build review prompt
        let prompt = Self::build_review_prompt(tool_name, action, summary);
        let messages = vec![crate::agent::Message {
            role: "system".to_string(),
            content: prompt,
        }];

        // Call the review agent with timeout
        let (token_tx, mut token_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let sender = crate::agent::StreamingSender::new(token_tx);

        let result = tokio::time::timeout(self.timeout, async {
            self.review_agent.chat(messages, None, None, sender).await
        })
        .await;

        match result {
            Ok(Ok(())) => {
                // Collect the full response
                let mut response = String::new();
                while let Some(token) = token_rx.recv().await {
                    response.push_str(&token);
                }
                let decision = Self::parse_decision(&response);
                {
                    let mut cb = self.circuit_breaker.lock().await;
                    cb.record_decision(!decision.is_allowed());
                }
                debug!(
                    tool = tool_name,
                    allowed = decision.is_allowed(),
                    "Guardian: review completed"
                );
                decision
            }
            Ok(Err(e)) => {
                warn!(
                    tool = tool_name,
                    error = %e,
                    "Guardian: review agent chat failed"
                );
                let mut cb = self.circuit_breaker.lock().await;
                cb.record_decision(true);
                GuardianDecision::Deny {
                    reason: format!("Guardian review chat failed: {}", e),
                }
            }
            Err(_elapsed) => {
                warn!(
                    tool = tool_name,
                    timeout_ms = self.timeout.as_millis(),
                    "Guardian: review timed out"
                );
                let mut cb = self.circuit_breaker.lock().await;
                cb.record_decision(true);
                GuardianDecision::Deny {
                    reason: format!(
                        "Guardian review timed out after {}ms",
                        self.timeout.as_millis()
                    ),
                }
            }
        }
    }

    /// Build the review prompt from action and conversation summary.
    fn build_review_prompt(tool_name: &str, action: &ToolInput, summary: &str) -> String {
        format!(
            r#"You are a security review gate. Your task is to determine if the following
tool action is consistent with the user's intent and conversation context.

Conversation summary:
{}

Tool action to review:
- Tool: {}
- Task: {}
- Objective: {}
- Phase: {}
- Agent role: {}

Reply with exactly one of the following on the first line:
ALLOW — the action is consistent with user intent
DENY — the action is NOT consistent with user intent

On the second line, provide a brief explanation (1-2 sentences).
If unsure, reply DENY (fail closed).
"#,
            summary, tool_name, action.task_id, action.objective, action.phase, action.agent_role,
        )
    }

    /// Parse the review agent's response into a GuardianDecision.
    fn parse_decision(response: &str) -> GuardianDecision {
        let first_line = response
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_uppercase())
            .unwrap_or_default();

        if first_line == "ALLOW" {
            GuardianDecision::Allow { confidence: 0.8 }
        } else if first_line == "DENY" {
            let reason = response
                .lines()
                .skip(1)
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .unwrap_or_else(|| "denied by Guardian review".to_string());
            GuardianDecision::Deny { reason }
        } else {
            // Parse failure — fail closed
            GuardianDecision::Deny {
                reason: format!(
                    "Guardian: could not parse decision from response: '{}'",
                    first_line
                ),
            }
        }
    }
}

/// Diagnostic status of the Guardian circuit breaker.
#[derive(Debug, Clone)]
pub struct GuardianBreakerStatus {
    pub consecutive_denials: u32,
    pub max_consecutive_denials: u32,
    pub recent_denials: u32,
    pub max_recent_denials: u32,
    pub tripped: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, Message, ModelInfo, StreamingSender};
    use async_trait::async_trait;
    use serde_json::json;

    /// A test agent that always allows.
    struct AllowAgent;

    #[async_trait]
    impl Agent for AllowAgent {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _principles: Option<Vec<String>>,
            _options: Option<std::collections::HashMap<String, serde_json::Value>>,
            sender: StreamingSender,
        ) -> std::result::Result<(), crate::core::error::AppError> {
            let _ = sender.send("ALLOW\nAction is consistent with user intent.".to_string());
            Ok(())
        }

        fn available_models(&self) -> Vec<ModelInfo> {
            vec![ModelInfo {
                id: "allow-agent".to_string(),
                name: "Allow Agent".to_string(),
                description: "Test agent that always allows".to_string(),
                is_default: true,
                capabilities: vec!["chat".to_string()],
                context_window: None,
            }]
        }
    }

    /// A test agent that always denies.
    struct DenyAgent;

    #[async_trait]
    impl Agent for DenyAgent {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _principles: Option<Vec<String>>,
            _options: Option<std::collections::HashMap<String, serde_json::Value>>,
            sender: StreamingSender,
        ) -> std::result::Result<(), crate::core::error::AppError> {
            let _ = sender.send("DENY\nAction not requested by user.".to_string());
            Ok(())
        }

        fn available_models(&self) -> Vec<ModelInfo> {
            vec![ModelInfo {
                id: "deny-agent".to_string(),
                name: "Deny Agent".to_string(),
                description: "Test agent that always denies".to_string(),
                is_default: true,
                capabilities: vec!["chat".to_string()],
                context_window: None,
            }]
        }
    }

    /// A test agent that returns invalid output.
    struct InvalidAgent;

    #[async_trait]
    impl Agent for InvalidAgent {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _principles: Option<Vec<String>>,
            _options: Option<std::collections::HashMap<String, serde_json::Value>>,
            sender: StreamingSender,
        ) -> std::result::Result<(), crate::core::error::AppError> {
            let _ = sender.send("MAYBE\nI'm not sure.".to_string());
            Ok(())
        }

        fn available_models(&self) -> Vec<ModelInfo> {
            vec![ModelInfo {
                id: "invalid-agent".to_string(),
                name: "Invalid Agent".to_string(),
                description: "Test agent that returns invalid output".to_string(),
                is_default: true,
                capabilities: vec!["chat".to_string()],
                context_window: None,
            }]
        }
    }

    fn make_tool_input() -> ToolInput {
        ToolInput {
            task_id: "test-task".to_string(),
            phase: "execute".to_string(),
            agent_role: "general".to_string(),
            objective: "Write a file".to_string(),
            constraints: None,
            evidence: None,
            payload: json!({}),
            allowed_base_dir: None,
        }
    }

    #[test]
    fn test_circuit_breaker_default_not_tripped() {
        let cb = GuardianCircuitBreaker::default();
        assert!(!cb.should_skip_review());
    }

    #[test]
    fn test_circuit_breaker_trips_on_consecutive_denials() {
        let mut cb = GuardianCircuitBreaker::new(3, 10, 50);
        assert!(!cb.should_skip_review());
        cb.record_decision(true);
        cb.record_decision(true);
        cb.record_decision(true);
        assert!(cb.should_skip_review());
    }

    #[test]
    fn test_circuit_breaker_resets_on_allow() {
        let mut cb = GuardianCircuitBreaker::new(3, 10, 50);
        cb.record_decision(true);
        cb.record_decision(true);
        cb.record_decision(false); // allow resets consecutive count
        assert!(!cb.should_skip_review());
    }

    #[test]
    fn test_parse_decision_allow() {
        let decision = GuardianReviewer::parse_decision("ALLOW\nSafe to proceed");
        assert_eq!(decision, GuardianDecision::Allow { confidence: 0.8 });
    }

    #[test]
    fn test_parse_decision_deny() {
        let decision = GuardianReviewer::parse_decision("DENY\nAction not requested");
        assert_eq!(
            decision,
            GuardianDecision::Deny {
                reason: "Action not requested".to_string()
            }
        );
    }

    #[test]
    fn test_parse_decision_invalid_fails_closed() {
        let decision = GuardianReviewer::parse_decision("MAYBE\nNot sure");
        assert!(!decision.is_allowed());
        assert!(matches!(decision, GuardianDecision::Deny { .. }));
    }

    #[test]
    fn test_parse_decision_empty_fails_closed() {
        let decision = GuardianReviewer::parse_decision("");
        assert!(!decision.is_allowed());
    }

    #[test]
    fn test_build_review_prompt_contains_action_details() {
        let action = make_tool_input();
        let prompt = GuardianReviewer::build_review_prompt(
            "write_file",
            &action,
            "User wants to edit a file",
        );
        assert!(prompt.contains("Write a file"));
        assert!(prompt.contains("ALLOW"));
        assert!(prompt.contains("DENY"));
    }

    #[tokio::test]
    async fn test_guardian_review_allow() {
        let agent = Arc::new(AllowAgent);
        let reviewer = GuardianReviewer::new(agent, None);
        let action = make_tool_input();
        let decision = reviewer
            .review_action("write_file", &action, "User asks to write a file")
            .await;
        assert!(decision.is_allowed());
    }

    #[tokio::test]
    async fn test_guardian_review_deny() {
        let agent = Arc::new(DenyAgent);
        let reviewer = GuardianReviewer::new(agent, None);
        let action = make_tool_input();
        let decision = reviewer
            .review_action("write_file", &action, "User asks to write a file")
            .await;
        assert_eq!(
            decision,
            GuardianDecision::Deny {
                reason: "Action not requested by user.".to_string()
            }
        );
    }

    #[tokio::test]
    async fn test_guardian_review_invalid_response_fails_closed() {
        let agent = Arc::new(InvalidAgent);
        let reviewer = GuardianReviewer::new(agent, None);
        let action = make_tool_input();
        let decision = reviewer
            .review_action("write_file", &action, "User asks to write a file")
            .await;
        assert!(!decision.is_allowed());
        assert!(matches!(decision, GuardianDecision::Deny { .. }));
    }

    #[tokio::test]
    async fn test_circuit_breaker_trips_reviewer() {
        let agent = Arc::new(DenyAgent);
        let reviewer = GuardianReviewer::with_circuit_breaker(agent, 3, 10, 50, None);
        let action = make_tool_input();

        // Three denials should trip the breaker
        for _ in 0..3 {
            let decision = reviewer.review_action("write_file", &action, "test").await;
            assert!(!decision.is_allowed());
        }

        // Fourth review should escalate
        let decision = reviewer.review_action("write_file", &action, "test").await;
        assert!(matches!(decision, GuardianDecision::EscalateToUser { .. }));
    }

    #[tokio::test]
    async fn test_circuit_breaker_status() {
        let agent = Arc::new(DenyAgent);
        let reviewer = GuardianReviewer::new(agent, None);

        let status = reviewer.circuit_breaker_status().await;
        assert!(!status.tripped);
    }
}
