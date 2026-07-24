//! Optimization modules for reliability, speed, and workflow optimization.
//!
//! This module contains components responsible for optimizing various aspects
//! of the ACP proxy system, including:
//!
//! - **Failure Prevention**: Proactive measures to prevent system failures
//! - **Conversation Compaction**: Token-efficient history management (BLUE71 §10)

pub mod compaction;
pub mod failure_prevention;
