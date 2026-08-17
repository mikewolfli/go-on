//! Usage-scoring policy passes for the skill registry (M3.1).
//!
//! Two deterministic, registry-only passes:
//!
//! * [`archive_low_frequency`] — mark under-used skills as archived so they
//!   drop out of model-facing discovery.
//! * [`promote_high_frequency`] — un-archive skills whose recorded usage has
//!   crossed the frequency bar again (inverse policy).
//!
//! The registry records call volume but no per-call timestamps, so
//! "frequency" is approximated by total recorded calls over the registry's
//! lifetime (`SkillDescriptor::total_calls`). These passes are pure policy —
//! they touch nothing outside the registry (no persistence, no LLM) — and a
//! richer usage-history / curator pass can replace them later without
//! changing the signatures.

use crate::orchestration::skill::registry::SkillRegistry;

/// Minimum recorded calls before a skill is eligible for the archiving pass.
/// Below this there is not enough signal to judge a skill as under-used.
pub(crate) const ARCHIVE_MIN_CALLS: u64 = 5;

/// Call-volume ceiling: skills with at least [`ARCHIVE_MIN_CALLS`] recorded
/// calls but fewer than this are archived as low-frequency.
pub(crate) const ARCHIVE_THRESHOLD_CALLS: u64 = 20;

/// Policy pass: archive skills that have enough recorded usage to judge
/// (`total_calls >= min_calls`) but whose call volume still falls below
/// `threshold_calls`. Returns the names archived in this pass.
///
/// Already-archived skills are left untouched, so the pass is idempotent.
/// Archived skills stay registered and invocable via `get()`, but are
/// excluded from model-facing discovery until promoted back.
pub(crate) fn archive_low_frequency(
    registry: &mut SkillRegistry,
    min_calls: u64,
    threshold_calls: u64,
) -> Vec<String> {
    let mut archived = Vec::new();
    for desc in registry.list(true) {
        if desc.total_calls >= min_calls
            && desc.total_calls < threshold_calls
            && !registry.is_archived(&desc.name)
        {
            registry.set_archived(&desc.name, true);
            archived.push(desc.name);
        }
    }
    archived
}

/// Inverse policy pass: un-archive skills whose recorded call volume has
/// crossed `threshold_calls`, so they surface in model-facing discovery
/// again. Returns the names promoted in this pass.
pub(crate) fn promote_high_frequency(
    registry: &mut SkillRegistry,
    threshold_calls: u64,
) -> Vec<String> {
    let mut promoted = Vec::new();
    for desc in registry.list(true) {
        if desc.total_calls >= threshold_calls && registry.is_archived(&desc.name) {
            registry.set_archived(&desc.name, false);
            promoted.push(desc.name);
        }
    }
    promoted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::skill::execution::PromptBasedSkill;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    fn register_skill(registry: &mut SkillRegistry, name: &str) {
        registry
            .register(Arc::new(PromptBasedSkill {
                name: name.to_string(),
                description: format!("test skill {}", name),
                prompt_template: "template".to_string(),
                input_schema: HashMap::new(),
                timeout_secs: 30,
                max_retries: 2,
                disable_model_invocation: false,
                policy: None,
            }))
            .unwrap();
    }

    fn seed_calls(registry: &mut SkillRegistry, name: &str, calls: u64) {
        for _ in 0..calls {
            registry.record_outcome(name, true, Duration::from_millis(10));
        }
    }

    #[test]
    fn archive_low_frequency_archives_only_used_underused_skills() {
        let mut registry = SkillRegistry::default();
        register_skill(&mut registry, "never-used");
        register_skill(&mut registry, "lightly-used");
        register_skill(&mut registry, "well-used");
        seed_calls(&mut registry, "never-used", 0);
        seed_calls(&mut registry, "lightly-used", 10);
        seed_calls(&mut registry, "well-used", 30);

        let archived = archive_low_frequency(&mut registry, 5, 20);

        // never-used: below min_calls -> no signal, left alone.
        // lightly-used: 5 <= 10 < 20 -> archived.
        // well-used: crossed the threshold -> kept active.
        assert_eq!(archived, vec!["lightly-used"]);
        assert!(!registry.is_archived("never-used"));
        assert!(registry.is_archived("lightly-used"));
        assert!(!registry.is_archived("well-used"));
    }

    #[test]
    fn archive_low_frequency_is_idempotent() {
        let mut registry = SkillRegistry::default();
        register_skill(&mut registry, "under-used");
        seed_calls(&mut registry, "under-used", 7);

        let first = archive_low_frequency(&mut registry, 5, 20);
        let second = archive_low_frequency(&mut registry, 5, 20);
        assert_eq!(first, vec!["under-used"]);
        assert!(second.is_empty());
    }

    #[test]
    fn archived_skills_stay_invocable_but_hidden_from_discovery() {
        let mut registry = SkillRegistry::default();
        register_skill(&mut registry, "under-used");
        seed_calls(&mut registry, "under-used", 7);
        archive_low_frequency(&mut registry, 5, 20);

        // Still invocable by exact name.
        assert!(registry.get("under-used").is_some());
        // Excluded from model-facing discovery, included in exhaustive listing.
        assert!(registry.list(false).is_empty());
        assert_eq!(registry.list(true).len(), 1);
    }

    #[test]
    fn promote_high_frequency_unarchives_after_usage_recovers() {
        let mut registry = SkillRegistry::default();
        register_skill(&mut registry, "recovering");
        seed_calls(&mut registry, "recovering", 7);
        archive_low_frequency(&mut registry, 5, 20);
        assert!(registry.is_archived("recovering"));

        // Usage recovers past the threshold.
        seed_calls(&mut registry, "recovering", 20);
        let promoted = promote_high_frequency(&mut registry, 20);

        assert_eq!(promoted, vec!["recovering"]);
        assert!(!registry.is_archived("recovering"));
        // Back in model-facing discovery.
        assert_eq!(registry.list(false).len(), 1);
    }
}
