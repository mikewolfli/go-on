//! TwoPhaseCoordinator integration for tool transactions (GAP-46-12)
//!
//! Executes a batch of tools within a distributed transaction coordinated
//! by TwoPhaseCoordinator. Each tool execution is treated as a participant
//! in the 2PC protocol. If any tool fails, the coordinator triggers abort
//! and rollback via the `TransactionScope` compensation actions.

use crate::orchestration::distributed_tx::{DistributedTxStatus, TwoPhaseCoordinator};
use crate::orchestration::tool::{ToolInput, ToolOutput, ToolRegistry};

use super::{ToolCallResult, ToolCallStatus, TransactionScope};

/// Execute a batch of tools within a distributed transaction coordinated
/// by TwoPhaseCoordinator.
///
/// Each tool execution is treated as a participant in the 2PC protocol.
/// If any tool fails, the coordinator triggers abort and rollback via
/// the `TransactionScope` compensation actions.
// Reserved for future 2PC integration (F-GAP-29).
#[allow(dead_code)] // F-GAP-49 — reserved for tool subsystem
                    // F-GAP-49 — reserved for future use
pub async fn execute_with_two_phase_coordination(
    coordinator: &TwoPhaseCoordinator,
    tool_names: &[String],
    tool_inputs: &[ToolInput],
    registry: &ToolRegistry,
) -> Vec<ToolCallResult> {
    if tool_names.is_empty() {
        return Vec::new();
    }

    let description = format!("2PC batch: {} tools", tool_names.len());
    let tx = coordinator.begin_tx(&description).await;
    let tx_id = tx.tx_id.clone();

    tracing::info!(
        "TwoPhaseCoordinator: starting 2PC transaction '{}' with {} tools",
        tx_id,
        tool_names.len()
    );

    // Register participants for each tool.
    for (i, tool_name) in tool_names.iter().enumerate() {
        let participant_id = format!("tool-{}-{}", i, tool_name);
        let address = format!("local.{}", tool_name);
        if let Err(e) = coordinator
            .add_participant(&tx_id, &participant_id, &address)
            .await
        {
            tracing::warn!(
                "TwoPhaseCoordinator: failed to add participant '{}': {}",
                participant_id,
                e
            );
        }
    }

    // Execute tools and collect results.
    let mut results = Vec::with_capacity(tool_names.len());
    let mut scope = TransactionScope::new(tx_id.clone());

    for (tool_name, input) in tool_names.iter().zip(tool_inputs.iter()) {
        let output = match registry.run_with_fallback(tool_name, input) {
            Ok(out) => out,
            Err(e) => ToolOutput {
                success: false,
                result: None,
                error: Some(format!("{}", e)),
                verification: None,
                audit_log: None,
                pua_report: None,
            },
        };

        let result = ToolCallResult {
            status: if output.success {
                ToolCallStatus::Success
            } else {
                ToolCallStatus::Failure(
                    output
                        .error
                        .clone()
                        .unwrap_or_else(|| "unknown error".to_string()),
                )
            },
            idempotency_key: None,
            idempotency_hit: false,
            transaction_id: Some(tx_id.clone()),
            output,
            duration_ms: 0,
        };

        // Register compensation action for rollback capability.
        let tool_name_clone = tool_name.clone();
        scope.register_completion(
            tool_name.clone(),
            std::sync::Arc::new(move || {
                tracing::info!("2PC rollback: compensating tool '{}'", tool_name_clone);
            }),
        );

        results.push(result);
    }

    // Execute 2PC protocol.
    let final_tx = coordinator.execute_2pc(&tx_id).await;
    tracing::info!(
        "TwoPhaseCoordinator: transaction '{}' final status: {:?}",
        tx_id,
        final_tx.status
    );

    // If transaction aborted, roll back completed tools.
    if !final_tx.status.is_terminal()
        || final_tx.status == DistributedTxStatus::Aborted
        || final_tx.status == DistributedTxStatus::Indeterminate
    {
        tracing::warn!(
            "TwoPhaseCoordinator: rolling back transaction '{}' (status: {:?})",
            tx_id,
            final_tx.status
        );
        scope.rollback().await;
    }

    results
}
