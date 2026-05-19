//! Observability modules for monitoring, telemetry, and performance tracking.
//!
//! This module contains components responsible for system observability
//! in the ACP proxy system, including:
//!
//! - **Observability**: Core observability infrastructure and utilities
//! - **Performance**: Performance monitoring and optimization tracking
//! - **Telemetry**: Basic telemetry collection and reporting
//! - **Telemetry Enhanced**: Advanced telemetry with additional metrics and insights
//!
//! These modules work together to provide comprehensive visibility into
//! system behavior, performance characteristics, and operational health.

#![allow(clippy::module_inception)]

pub mod memory_health;
pub mod observability;
pub mod performance;
pub mod provenance;
pub mod telemetry;
pub mod telemetry_enhanced;

// Re-exports are not needed; consumers use full crate paths.
// See main.rs for usage: crate::observability::memory_health::*
