//! Action subsystem — stage 3 of the capability bus lifecycle
//!
//! Dispatches tool execution through the ToolBus with HarnessBus validation
//! and ObservabilityBus tracing.
//!
//! Extracted from `core.rs` to isolate the `execute_tool()` method.
//! (BLUE38 ARCH-13)

use super::core::CapabilityBus;
use std::time::Instant;

impl CapabilityBus {
    // ------------------------------------------------------------------
    // Stage 3: Action — dispatch to agent with tool bus awareness
    // ------------------------------------------------------------------

    /// Execute a tool through the ToolBus with HarnessBus validation
    pub fn execute_tool(
        &self,
        tool_name: &str,
        input: &crate::orchestration::tool::ToolInput,
    ) -> anyhow::Result<crate::orchestration::tool::ToolOutput> {
        // Step 1: Validate via HarnessBus
        let tool_verdict = self
            .harness
            .evaluator
            .check_tool_call(tool_name, &input.payload);
        if !tool_verdict.is_allowed() {
            self.record_event(
                "action",
                None,
                None,
                "blocked",
                Self::build_action_blocked_detail(tool_name, "HarnessBus denied"),
            );
            return Err(anyhow::anyhow!(
                "Tool call '{}' denied by HarnessBus policy",
                tool_name
            ));
        }

        // Step 2: Execute via ToolBus
        let start = Instant::now();
        #[cfg(feature = "sub-bus-tool")]
        let result = self.tool_bus.execute_tool(tool_name, input);
        #[cfg(not(feature = "sub-bus-tool"))]
        let result: anyhow::Result<crate::orchestration::tool::ToolOutput> =
            Err(anyhow::anyhow!("ToolBus not available in this profile"));
        let duration_ms = start.elapsed().as_millis() as u64;
        let success = result
            .as_ref()
            .map(|output| output.success)
            .unwrap_or(false);
        let error_text = result
            .as_ref()
            .err()
            .map(|err| err.to_string())
            .or_else(|| result.as_ref().ok().and_then(|output| output.error.clone()));

        // Step 3: Record execution in ObservabilityBus
        #[cfg(feature = "sub-bus-observability")]
        self.observability_bus.record_trace(
            "capability_bus",
            "tool_call",
            duration_ms,
            success,
            error_text.clone(),
            0,
        );

        // Step 4: Record event
        let outcome = Self::action_outcome_label(success);
        self.record_event(
            "action",
            None,
            None,
            outcome,
            Self::build_action_event_detail(tool_name, duration_ms, success, error_text),
        );

        result
    }
}
