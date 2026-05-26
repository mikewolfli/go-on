//! Drift Protection (F-GAP-26)
//!
//! Detects and prevents goal drift, capability drift, and behavioral drift
//! by comparing measured metrics against established baselines and evaluating
//! deviation against configured policy thresholds.

pub mod drift_protection;

#[allow(unused_imports)]
pub use drift_protection::{
    DriftAlert, DriftMetric, DriftPolicy, DriftProfile, DriftProtectionConfig,
    DriftProtectionEngine, DriftSeverity, DriftType, // re-exported for public API surface
};
