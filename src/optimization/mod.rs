//! Optimization modules for cost, reliability, speed, and workflow optimization.
//!
//! This module contains components responsible for optimizing various aspects
//! of the ACP proxy system, including:
//!
//! - **Cost Optimization**: Minimizing API costs while maintaining quality
//! - **Reliability Optimization**: Improving system stability and fault tolerance
//! - **Speed Optimization**: Reducing latency and improving response times
//! - **Workflow Optimization**: Streamlining task execution and resource usage
//! - **Failure Prevention**: Proactive measures to prevent system failures

pub mod cost_optimizer;
pub mod failure_prevention;
pub mod reliability_optimizer;
pub mod speed_optimizer;
pub mod workflow_optimizer;
