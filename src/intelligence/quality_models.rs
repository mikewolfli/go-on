use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum QualityVerdict {
    Approve,
    ApproveWithCaveats,
    Reject,
    Revise,
    InsufficientEvidence,
    Valid,
    Invalid,
    RequiresRepair,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum QualitySignalType {
    Syntax,
    Tests,
    Lint,
    Policy,
    Logic,
    PuaQualityCompass,
    RuntimeVerification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualitySignal {
    pub signal_type: QualitySignalType,
    pub passed: bool,
    pub confidence: f32,
    pub details: Option<String>,
}
