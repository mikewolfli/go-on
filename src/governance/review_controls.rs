//! Review controls — verdict parsing, timeout policy, gate outcomes.
//!
//! # Status
//! Complete implementation ready for CapabilityBus integration (ARCH-13).

use std::time::Duration;

use serde::Serialize;

use crate::config::PhaseOptions;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReviewDecision {
    pub(crate) reviewer: String,
    pub(crate) verdict: String,
    pub(crate) response: String,
}

/// Governance-internal review verdict.
/// This enum uses `Approve`/`Reject` semantics (distinct from
/// the public `acp::prelude::ReviewVerdict` which uses `Pass`/`Fail`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewVerdict {
    Approve,
    Reject,
    Invalid,
}

impl ReviewVerdict {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "APPROVE",
            Self::Reject => "REJECT",
            Self::Invalid => "INVALID",
        }
    }

    pub(crate) fn is_approved(self) -> bool {
        matches!(self, Self::Approve)
    }
}

pub(crate) enum ReviewGateOutcome {
    Approved(Vec<ReviewDecision>),
    Rejected(Vec<ReviewDecision>),
    Degraded(Vec<ReviewDecision>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Governance-internal timeout policy kind.
/// This is an enum (not a struct) used to express policy decisions
/// during review-timeout evaluation in the governance layer.
pub(crate) enum ReviewTimeoutPolicyKind {
    Reject,
    DegradeSingle,
}

impl ReviewTimeoutPolicyKind {
    pub(crate) fn from_options(options: Option<&PhaseOptions>) -> Self {
        let value = options
            .and_then(|opts| opts.extra.get("review_timeout_policy"))
            .and_then(|v| v.as_str())
            .unwrap_or("reject");

        if value.eq_ignore_ascii_case("degrade_single") {
            Self::DegradeSingle
        } else {
            Self::Reject
        }
    }
}

pub(crate) fn review_timeout(
    review_options: Option<&PhaseOptions>,
    primary_phase_options: Option<&PhaseOptions>,
) -> Option<Duration> {
    review_options
        .and_then(|opts| opts.review_timeout_seconds.or(opts.request_timeout_seconds))
        .or_else(|| {
            primary_phase_options
                .and_then(|opts| opts.review_timeout_seconds.or(opts.request_timeout_seconds))
        })
        .map(Duration::from_secs)
}

pub(crate) fn review_verdict(response: &str, min_response_chars: usize) -> ReviewVerdict {
    if response.trim().chars().count() < min_response_chars {
        return ReviewVerdict::Invalid;
    }

    let first_line = response.lines().find(|line| !line.trim().is_empty());
    match first_line.map(|line| line.trim().to_ascii_uppercase()) {
        Some(value) if value.starts_with("APPROVE") => ReviewVerdict::Approve,
        Some(value) if value.starts_with("REJECT") => ReviewVerdict::Reject,
        _ => ReviewVerdict::Invalid,
    }
}
