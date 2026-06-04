//! `ExecutionRecorder` trait — trait-object boundary for self-model recording.
//!
//! Extracted to break the circular dependency:
//!   acp → observability → intelligence → acp
//!
//! `LivePerformanceFeed` (observability) stores an optional
//! `Box<dyn ExecutionRecorder>` instead of a concrete `SelfModelCore`
//! (intelligence).  Any type that can record execution results (including
//! `SelfModelCore`) implements this trait.

/// Records a model execution outcome for dynamic capability scoring.
///
/// This trait is the sole interface between `LivePerformanceFeed` and
/// the self-model system.  The concrete implementation lives in
/// `intelligence::self_model::SelfModelCore`.
pub trait ExecutionRecorder: Send + Sync {
    /// Record the result of a model execution.
    ///
    /// * `capability_name` — name of the model or capability.
    /// * `success` — whether the execution succeeded.
    /// * `latency` — observed latency in milliseconds.
    fn record_execution_result(&self, capability_name: &str, success: bool, latency: u64);
}
