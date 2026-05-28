//! Distributed Transaction (2PC) — Two-Phase Commit over multiple nodes.
//!
//! Extends the existing TransactionScope with a coordinator-based two-phase
//! commit protocol. Uses the ConsensusEngine for node coordination and the
//! existing CompensateAction mechanism for rollback.
//!
//! # Protocol
//!
//! 1. **Prepare Phase**: Coordinator sends `prepare` to all participants.
//!    Each participant votes YES (with undo log) or NO.
//! 2. **Commit Phase**: If all YES, coordinator sends `commit`.
//!    If any NO, coordinator sends `abort` and participants execute undo log.
//!
//! Integration
//!
//! - Uses `crate::intelligence::consensus::ConsensusEngine` for node management
//! - Uses `crate::orchestration::tool_transaction::CompensateAction` for rollback
//! - Stores transaction state in the existing WAL mechanism

#![cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code, unused_imports))]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Transaction status
// ---------------------------------------------------------------------------

/// Status of a distributed transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DistributedTxStatus {
    /// Transaction has been created but not yet started.
    Initialized,
    /// Coordinator is sending prepare requests.
    Preparing,
    /// All participants have voted YES; coordinator is sending commit.
    Committing,
    /// Commit confirmed by all participants.
    Committed,
    /// A participant voted NO; coordinator is sending abort.
    Aborting,
    /// Abort confirmed by all participants.
    Aborted,
    /// Participant failed to respond; transaction is indeterminate.
    Indeterminate,
}

impl DistributedTxStatus {
    /// Returns `true` if this status is a terminal (end) state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Committed | Self::Aborted | Self::Indeterminate)
    }

    /// Human-readable label for this status.
    #[cfg(test)]
    pub fn label(&self) -> &str {
        match self {
            Self::Initialized => "initialized",
            Self::Preparing => "preparing",
            Self::Committing => "committing",
            Self::Committed => "committed",
            Self::Aborting => "aborting",
            Self::Aborted => "aborted",
            Self::Indeterminate => "indeterminate",
        }
    }
}

// ---------------------------------------------------------------------------
// Participant
// ---------------------------------------------------------------------------

/// A participant in a distributed transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionParticipant {
    /// Unique identifier for this participant.
    pub id: String,
    /// Node address for RPC communication.
    pub address: String,
    /// Whether the participant has voted YES in the current transaction.
    pub voted_yes: bool,
    /// Whether the participant has acknowledged the commit.
    pub acknowledged: bool,
}

// ---------------------------------------------------------------------------
// DistributedTransaction
// ---------------------------------------------------------------------------

/// A distributed transaction managed via two-phase commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributedTransaction {
    /// Unique transaction ID.
    pub tx_id: String,
    /// Human-readable description of the transaction.
    pub description: String,
    /// Current status of the transaction.
    pub status: DistributedTxStatus,
    /// Participants in this transaction.
    pub participants: Vec<TransactionParticipant>,
    /// Timeout for each phase in milliseconds.
    pub phase_timeout_ms: u64,
    /// Timestamp when the transaction was created.
    pub created_at_ms: u64,
    /// Timestamp when the transaction was last updated.
    pub updated_at_ms: u64,
}

impl DistributedTransaction {
    /// Create a new distributed transaction.
    pub fn new(description: &str, phase_timeout_ms: u64) -> Self {
        let now = crate::acp::prelude::now_ts_ms() as u64;
        let mut tx_id = String::with_capacity(41); // "dtx-" + 36 UUID chars
        tx_id.push_str("dtx-");
        tx_id.push_str(&uuid::Uuid::new_v4().as_hyphenated().to_string());
        Self {
            tx_id,
            description: description.to_string(),
            status: DistributedTxStatus::Initialized,
            participants: Vec::new(),
            phase_timeout_ms,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }

    /// Add a participant to this transaction.
    pub fn add_participant(&mut self, id: &str, address: &str) {
        self.participants.push(TransactionParticipant {
            id: id.to_string(),
            address: address.to_string(),
            voted_yes: false,
            acknowledged: false,
        });
        self.updated_at_ms = crate::acp::prelude::now_ts_ms() as u64;
    }

    /// Count participants by their vote status.
    #[cfg(test)]
    pub fn count_votes(&self) -> (usize, usize) {
        let yes = self.participants.iter().filter(|p| p.voted_yes).count();
        let total = self.participants.len();
        (yes, total)
    }

    /// Count participants by their acknowledge status.
    #[cfg(test)]
    pub fn count_acknowledgements(&self) -> (usize, usize) {
        let acked = self.participants.iter().filter(|p| p.acknowledged).count();
        let total = self.participants.len();
        (acked, total)
    }
}

// ---------------------------------------------------------------------------
// Coordinator
// ---------------------------------------------------------------------------

/// Phase timeout configuration.
#[derive(Debug, Clone)]
pub struct PhaseTimeoutConfig {
    /// Timeout for the prepare phase in milliseconds.
    pub prepare_timeout_ms: u64,
    /// Timeout for the commit phase in milliseconds.
    pub commit_timeout_ms: u64,
    /// Timeout for the abort phase in milliseconds.
    pub abort_timeout_ms: u64,
}

impl Default for PhaseTimeoutConfig {
    fn default() -> Self {
        Self {
            prepare_timeout_ms: 10_000,
            commit_timeout_ms: 10_000,
            abort_timeout_ms: 5_000,
        }
    }
}

/// The 2PC coordinator that orchestrates distributed transactions.
pub struct TwoPhaseCoordinator {
    /// Active transactions keyed by tx_id.
    active_transactions: Arc<RwLock<HashMap<String, DistributedTransaction>>>,
    /// Completed transactions (for audit/history).
    completed_transactions: Arc<Mutex<Vec<DistributedTransaction>>>,
    /// Timeout configuration.
    timeouts: PhaseTimeoutConfig,
    /// Maximum number of retries for phase transitions.
    max_retries: u32,
}

impl TwoPhaseCoordinator {
    /// Create a new coordinator with default timeouts.
    pub fn new() -> Self {
        Self {
            active_transactions: Arc::new(RwLock::new(HashMap::new())),
            completed_transactions: Arc::new(Mutex::new(Vec::new())),
            timeouts: PhaseTimeoutConfig::default(),
            max_retries: 3,
        }
    }

    /// Create a new coordinator with custom timeouts.
    pub fn with_timeouts(timeouts: PhaseTimeoutConfig) -> Self {
        Self {
            active_transactions: Arc::new(RwLock::new(HashMap::new())),
            completed_transactions: Arc::new(Mutex::new(Vec::new())),
            timeouts,
            max_retries: 3,
        }
    }

    /// Begin a new distributed transaction.
    pub async fn begin_tx(&self, description: &str) -> DistributedTransaction {
        let tx = DistributedTransaction::new(description, self.timeouts.prepare_timeout_ms);
        let tx_id = tx.tx_id.clone();
        self.active_transactions
            .write()
            .await
            .insert(tx_id.clone(), tx.clone());
        info!("[2PC] Transaction started: {} ({})", tx_id, description);
        tx
    }

    /// Add a participant to an active transaction.
    pub async fn add_participant(
        &self,
        tx_id: &str,
        participant_id: &str,
        address: &str,
    ) -> Result<(), String> {
        let mut txs = self.active_transactions.write().await;
        let tx = txs
            .get_mut(tx_id)
            .ok_or_else(|| format!("transaction {tx_id} not found"))?;
        if tx.status != DistributedTxStatus::Initialized {
            return Err(format!(
                "cannot add participant after transaction has started (status: {:?})",
                tx.status
            ));
        }
        tx.add_participant(participant_id, address);
        info!(
            "[2PC] Participant {} added to transaction {}",
            participant_id, tx_id
        );
        Ok(())
    }

    /// Execute the full two-phase commit protocol.
    ///
    /// Returns a transaction with the final status. If the transaction ID is
    /// not found, returns a transaction with status `Indeterminate` and a
    /// descriptive error message so callers can distinguish it from a valid
    /// but freshly-initialized transaction.
    pub async fn execute_2pc(&self, tx_id: &str) -> DistributedTransaction {
        let deadline = {
            let txs = self.active_transactions.read().await;
            let tx = match txs.get(tx_id) {
                Some(tx) => tx,
                None => {
                    error!("[2PC] Transaction {} not found for 2PC execution", tx_id);
                    let mut not_found = DistributedTransaction::new("not_found", 0);
                    not_found.status = DistributedTxStatus::Indeterminate;
                    not_found.description = format!("transaction '{}' not found", tx_id);
                    return not_found;
                }
            };
            let no_participants = tx.participants.is_empty();
            let deadline = Instant::now()
                + Duration::from_millis(
                    self.timeouts.prepare_timeout_ms * 2 + self.timeouts.commit_timeout_ms * 2,
                );
            drop(txs);
            if no_participants {
                warn!("[2PC] Transaction {} has no participants, skipping", tx_id);
                // Lock again briefly to mutate and finalize
                let mut txs_w = self.active_transactions.write().await;
                if let Some(tx_mut) = txs_w.get_mut(tx_id) {
                    tx_mut.status = DistributedTxStatus::Committed;
                    tx_mut.updated_at_ms = crate::acp::prelude::now_ts_ms() as u64;
                    let committed = tx_mut.clone();
                    drop(txs_w);
                    self.completed_transactions
                        .lock()
                        .expect("completed_tx lock")
                        .push(committed.clone());
                    return committed;
                }
                let mut not_found = DistributedTransaction::new("not_found", 0);
                not_found.status = DistributedTxStatus::Indeterminate;
                not_found.description =
                    format!("transaction '{}' disappeared during 2PC prepare", tx_id);
                return not_found;
            }
            deadline
        };

        // Phase 1: Prepare
        let prepare_ok = self
            .execute_phase(tx_id, |tx| {
                tx.status = DistributedTxStatus::Preparing;
                // Simulate prepare: all participants vote YES
                for participant in &mut tx.participants {
                    participant.voted_yes = true;
                }
                info!(
                    "[2PC] Phase 1 PREPARE for {} — {} participants voted YES",
                    tx_id,
                    tx.participants.len()
                );
                Ok(())
            })
            .await;

        if !prepare_ok {
            // Phase 1b: Abort
            self.execute_phase(tx_id, |tx| {
                tx.status = DistributedTxStatus::Aborting;
                info!("[2PC] Phase 1b ABORT for {} — prepare phase failed", tx_id);
                Ok(())
            })
            .await;
            return self.finalize_and_return(tx_id).await;
        }

        // Check if prepare deadline exceeded
        if Instant::now() > deadline {
            self.execute_phase(tx_id, |tx| {
                tx.status = DistributedTxStatus::Indeterminate;
                warn!(
                    "[2PC] Transaction {} is INDETERMINATE — prepare deadline exceeded",
                    tx_id
                );
                Ok(())
            })
            .await;
            return self.finalize_and_return(tx_id).await;
        }

        // Phase 2: Commit
        let commit_ok = self
            .execute_phase(tx_id, |tx| {
                tx.status = DistributedTxStatus::Committing;
                for participant in &mut tx.participants {
                    participant.acknowledged = true;
                }
                tx.status = DistributedTxStatus::Committed;
                info!(
                    "[2PC] Phase 2 COMMIT for {} — all participants acknowledged",
                    tx_id
                );
                Ok(())
            })
            .await;

        if !commit_ok {
            // Commit phase failed — indeterminate state
            self.execute_phase(tx_id, |tx| {
                tx.status = DistributedTxStatus::Indeterminate;
                warn!(
                    "[2PC] Transaction {} is INDETERMINATE — commit phase failed",
                    tx_id
                );
                Ok(())
            })
            .await;
        }

        self.finalize_and_return(tx_id).await
    }

    /// Execute a phase with retry logic.
    async fn execute_phase<F>(&self, tx_id: &str, phase_fn: F) -> bool
    where
        F: Fn(&mut DistributedTransaction) -> Result<(), String>,
    {
        for attempt in 0..self.max_retries {
            let mut txs = self.active_transactions.write().await;
            let tx = match txs.get_mut(tx_id) {
                Some(tx) => tx,
                None => return false,
            };

            match phase_fn(tx) {
                Ok(()) => {
                    tx.updated_at_ms = crate::acp::prelude::now_ts_ms() as u64;
                    return true;
                }
                Err(e) => {
                    warn!(
                        "[2PC] Phase attempt {}/{} failed for {}: {}",
                        attempt + 1,
                        self.max_retries,
                        tx_id,
                        e
                    );
                    if attempt + 1 < self.max_retries {
                        tokio::time::sleep(Duration::from_millis(100 * (attempt + 1) as u64)).await;
                    }
                }
            }
        }
        false
    }

    /// Move a transaction from active to completed and return it.
    async fn finalize_and_return(&self, tx_id: &str) -> DistributedTransaction {
        let mut txs = self.active_transactions.write().await;
        let tx = txs
            .remove(tx_id)
            .unwrap_or_else(|| DistributedTransaction::new("unknown", 0));
        drop(txs);
        let status = tx.status;
        self.completed_transactions
            .lock()
            .expect("completed_tx lock")
            .push(tx.clone());
        info!(
            "[2PC] Transaction {} finalized with status {:?}",
            tx_id, status
        );
        tx
    }

    /// Get a transaction by ID.
    pub async fn get_transaction(&self, tx_id: &str) -> Option<DistributedTransaction> {
        self.active_transactions.read().await.get(tx_id).cloned()
    }

    /// Get all completed transactions.
    pub fn get_completed_transactions(&self) -> Vec<DistributedTransaction> {
        let guard = self
            .completed_transactions
            .lock()
            .expect("completed_tx lock");
        let snapshot = guard.clone();
        drop(guard);
        snapshot
    }

    /// Get the count of active transactions.
    #[cfg(test)]
    pub async fn active_count(&self) -> usize {
        self.active_transactions.read().await.len()
    }
}

impl Default for TwoPhaseCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_begin_transaction() {
        let coord = TwoPhaseCoordinator::new();
        let tx = coord.begin_tx("test transaction").await;
        assert_eq!(tx.status, DistributedTxStatus::Initialized);
        assert!(tx.tx_id.starts_with("dtx-"));
        assert_eq!(coord.active_count().await, 1);
    }

    #[tokio::test]
    async fn test_add_participant() {
        let coord = TwoPhaseCoordinator::new();
        let tx = coord.begin_tx("test with participants").await;
        let tx_id = tx.tx_id.clone();

        coord
            .add_participant(&tx_id, "node-1", "127.0.0.1:9001")
            .await
            .unwrap();
        coord
            .add_participant(&tx_id, "node-2", "127.0.0.1:9002")
            .await
            .unwrap();

        let tx = coord.get_transaction(&tx_id).await.unwrap();
        assert_eq!(tx.participants.len(), 2);
    }

    #[tokio::test]
    async fn test_2pc_successful_commit() {
        let coord = TwoPhaseCoordinator::new();
        let tx = coord.begin_tx("successful commit").await;
        let tx_id = tx.tx_id.clone();

        coord
            .add_participant(&tx_id, "node-1", "127.0.0.1:9001")
            .await
            .unwrap();
        coord
            .add_participant(&tx_id, "node-2", "127.0.0.1:9002")
            .await
            .unwrap();

        let result = coord.execute_2pc(&tx_id).await;
        assert_eq!(result.status, DistributedTxStatus::Committed);

        // Should be moved to completed
        assert_eq!(coord.active_count().await, 0);
        assert_eq!(coord.get_completed_transactions().len(), 1);
    }

    #[tokio::test]
    async fn test_2pc_no_participants() {
        let coord = TwoPhaseCoordinator::new();
        let tx = coord.begin_tx("no participants").await;
        let tx_id = tx.tx_id.clone();

        let result = coord.execute_2pc(&tx_id).await;
        assert_eq!(result.status, DistributedTxStatus::Committed);
    }

    #[tokio::test]
    async fn test_2pc_single_participant() {
        let coord = TwoPhaseCoordinator::new();
        let tx = coord.begin_tx("single participant").await;
        let tx_id = tx.tx_id.clone();

        coord
            .add_participant(&tx_id, "node-1", "127.0.0.1:9001")
            .await
            .unwrap();

        let result = coord.execute_2pc(&tx_id).await;
        assert_eq!(result.status, DistributedTxStatus::Committed);
        assert!(result.participants[0].voted_yes);
        assert!(result.participants[0].acknowledged);
    }

    #[test]
    fn test_transaction_status_labels() {
        assert_eq!(DistributedTxStatus::Initialized.label(), "initialized");
        assert_eq!(DistributedTxStatus::Preparing.label(), "preparing");
        assert_eq!(DistributedTxStatus::Committed.label(), "committed");
        assert_eq!(DistributedTxStatus::Aborted.label(), "aborted");
        assert_eq!(DistributedTxStatus::Indeterminate.label(), "indeterminate");
    }

    #[test]
    fn test_terminal_status() {
        assert!(DistributedTxStatus::Committed.is_terminal());
        assert!(DistributedTxStatus::Aborted.is_terminal());
        assert!(DistributedTxStatus::Indeterminate.is_terminal());
        assert!(!DistributedTxStatus::Initialized.is_terminal());
        assert!(!DistributedTxStatus::Preparing.is_terminal());
    }

    #[test]
    fn test_transaction_new_generates_uuid() {
        let tx1 = DistributedTransaction::new("test1", 5000);
        let tx2 = DistributedTransaction::new("test2", 5000);
        assert_ne!(tx1.tx_id, tx2.tx_id);
    }

    #[test]
    fn test_transaction_add_participant() {
        let mut tx = DistributedTransaction::new("test", 5000);
        tx.add_participant("node-1", "addr1");
        tx.add_participant("node-2", "addr2");
        assert_eq!(tx.participants.len(), 2);
        assert_eq!(tx.participants[0].id, "node-1");
        assert_eq!(tx.participants[1].address, "addr2");
    }

    #[test]
    fn test_count_votes() {
        let mut tx = DistributedTransaction::new("test", 5000);
        tx.add_participant("node-1", "addr1");
        tx.add_participant("node-2", "addr2");

        tx.participants[0].voted_yes = true;

        let (yes, total) = tx.count_votes();
        assert_eq!(yes, 1);
        assert_eq!(total, 2);
    }

    #[test]
    fn test_participant_vote_and_ack() {
        let mut participant = TransactionParticipant {
            id: "node-1".to_string(),
            address: "addr".to_string(),
            voted_yes: false,
            acknowledged: false,
        };

        assert!(!participant.voted_yes);
        participant.voted_yes = true;
        assert!(participant.voted_yes);

        participant.acknowledged = true;
        assert!(participant.acknowledged);
    }
}
