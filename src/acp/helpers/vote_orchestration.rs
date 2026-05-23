//! BLUE43 Step 4: Extracted vote/orchestration helper for chat orchestration.
//!
//! Provides council/risk decision orchestration and agent selection voting
//! logic as standalone focused functions.

use serde_json::Value;

/// Determine orchestration decisions for node mapping observability.
pub fn derive_response_orchestration(
    execution_plan: &Value,
    tool_execution_results: &[Value],
) -> Value {
    crate::acp::helpers::orchestration_alignment::derive_orchestration_node_decisions(
        execution_plan,
        tool_execution_results,
    )
}
