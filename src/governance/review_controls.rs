//! F-GAP-14: Review controls, verdict parsing
//!
//! # Status
//! Complete implementation ready for CapabilityBus integration (ARCH-13).
//!
//! The canonical verdict type is [`crate::intelligence::quality_models::QualityVerdict`].
//! This module re-exports it and provides governance-specific helpers.

use std::time::Duration;

use crate::config::PhaseOptions;
use crate::i18n::t;
use crate::intelligence::quality_models::QualityVerdict;

/// Governance-internal verdict, unified with [`QualityVerdict`].
///
/// Only a subset of [`QualityVerdict`] variants are used in the review-verdict
/// parsing path: `Approve`, `Reject`, and `Invalid`. The type alias is used
/// for semantic clarity within the governance module.
pub(crate) type ReviewVerdict = QualityVerdict;

/// Convert a [`ReviewVerdict`] (i.e. [`QualityVerdict`]) to a translated string.
pub(crate) fn verdict_as_str(verdict: ReviewVerdict) -> String {
    match verdict {
        QualityVerdict::Approve => t("status.review_controls.approve"),
        QualityVerdict::Reject => t("status.review_controls.reject"),
        QualityVerdict::Invalid => t("status.review_controls.invalid"),
        _ => t("status.review_controls.invalid"),
    }
}

/// Returns `true` for approve-level verdicts.
pub(crate) fn verdict_is_approved(verdict: ReviewVerdict) -> bool {
    matches!(verdict, QualityVerdict::Approve)
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

pub(crate) fn review_verdict(response: &str, min_response_chars: usize) -> QualityVerdict {
    if response.trim().chars().count() < min_response_chars {
        return QualityVerdict::Invalid;
    }

    let first_line = response.lines().find(|line| !line.trim().is_empty());
    match first_line.map(|line| line.trim().to_ascii_uppercase()) {
        Some(value) if value == "APPROVE" => QualityVerdict::Approve,
        Some(value) if value == "REJECT" => QualityVerdict::Reject,
        // Allow optional colon separator: "APPROVE:" or "REJECT:"
        Some(value) if value.starts_with("APPROVE:") => {
            // Require at least one valid reason character after the colon
            let rest = &value["APPROVE:".len()..];
            if !rest.trim().is_empty() {
                QualityVerdict::Approve
            } else {
                QualityVerdict::Invalid
            }
        }
        Some(value) if value.starts_with("REJECT:") => {
            let rest = &value["REJECT:".len()..];
            if !rest.trim().is_empty() {
                QualityVerdict::Reject
            } else {
                QualityVerdict::Invalid
            }
        }
        _ => QualityVerdict::Invalid,
    }
}
