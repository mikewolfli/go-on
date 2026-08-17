//! Deterministic skill auto-extraction (M3.1).
//!
//! [`maybe_auto_extract`] turns a completed conversation's task + outcome
//! into a reusable `SKILL.md` draft, registered as a prompt-based skill
//! under a `draft-` name prefix. The extraction is fully deterministic — no
//! LLM call — by design: [`draft_skill_md`] assembles the draft from
//! structured inputs, the draft is validated through the same manifest
//! parser the import pipeline uses ([`parse_skill_md`]), and registration
//! goes through the registry's prompt-skill path — the exact call
//! `SkillImportStore::import_skill` uses for prompt-based manifests.
//!
//! The full `SkillImportStore::import_skill` pipeline is deliberately *not*
//! used here: it is the security-policy gate for *external* sources (it
//! requires a real file/zip on disk, an allowlist entry, and a matching
//! SHA-256 when the policy demands one). An in-memory draft has no source
//! artifact, so routing it through that pipeline would mean fabricating a
//! temp file and digest. An LLM-backed refinement pass can plug in later
//! behind the same entry point without changing callers.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tracing::{info, warn};

use crate::orchestration::skill::registry::{validate_skill_name_rule, SkillRegistry};
use crate::orchestration::skill_import::parse_skill_md;

/// Prefix marking auto-extracted draft skills. Drafts are real, registered
/// skills (invocable, listed) — the prefix is the explicit "not yet
/// curated" marker and keeps drafts from colliding with hand-written names.
const DRAFT_NAME_PREFIX: &str = "draft-";

/// Fixed approach line: `maybe_auto_extract` only receives the task and the
/// outcome summary, so the recorded approach is the honest, generic fact
/// that the originating conversation completed the task successfully.
const DRAFT_APPROACH: &str =
    "reuse the approach that completed this task successfully in the originating conversation";

/// Max chars of the raw task text embedded into a draft's body.
const DRAFT_TASK_MAX_CHARS: usize = 200;

/// Max chars of the approach / outcome text embedded into a draft's body.
const DRAFT_SUMMARY_MAX_CHARS: usize = 400;

/// Deterministic SKILL.md template (go-on / agentskills.io format): YAML
/// frontmatter (`name` / `description` / `version`) plus `When to Use` and
/// `Steps` sections. No LLM call — pure string assembly from structured
/// inputs. The output is guaranteed to round-trip through
/// [`parse_skill_md`], so callers can validate the draft with the same
/// parser the import pipeline uses.
pub(crate) fn draft_skill_md(
    task: &str,
    approach: &str,
    outcome: &str,
    skill_name: &str,
) -> String {
    let task_line = single_line(task, DRAFT_TASK_MAX_CHARS);
    let approach_line = single_line(approach, DRAFT_SUMMARY_MAX_CHARS);
    let outcome_line = single_line(outcome, DRAFT_SUMMARY_MAX_CHARS);
    // The frontmatter description must stay a single line without a colon:
    // `parse_skill_md` splits each frontmatter line on the first `:`.
    let description = format!("Auto-extracted draft skill for: {task_line}");
    let description_fm = description.replace(':', " - ");

    format!(
        "---\n\
         name: {skill_name}\n\
         description: {description_fm}\n\
         version: 0.1.0-draft\n\
         ---\n\
         \n\
         # {skill_name}\n\
         \n\
         ## Description\n\
         {description}\n\
         \n\
         ## When to Use\n\
         Use this skill when a task matches: {task_line}\n\
         \n\
         ## Steps\n\
         1. Restate the task: {task_line}\n\
         2. Apply the recorded approach: {approach_line}\n\
         3. Confirm the expected outcome: {outcome_line}\n\
         \n\
         ## Auto-Extraction Notes\n\
         This draft was generated deterministically from a completed \
         conversation (no LLM refinement). The task and outcome were recorded \
         at extraction time; review and curate before relying on it. An \
         LLM-backed refinement pass can replace this template later.\n"
    )
}

/// Fire-and-forget entry point (M3.1): when `enabled`, derive a draft skill
/// name from `task`, skip if it already exists, and register a
/// deterministic `SKILL.md` draft via the registry's prompt-skill path.
///
/// Never panics and never blocks the caller meaningfully: all work happens
/// synchronously under the registry write lock, and every failure path is
/// logged and swallowed — the intended caller is a detached task.
pub(crate) async fn maybe_auto_extract(
    registry: Arc<RwLock<SkillRegistry>>,
    enabled: bool,
    task: &str,
    summary: &str,
) {
    if !enabled {
        return;
    }
    let task = task.trim();
    if task.is_empty() {
        warn!("skill auto-extract skipped: empty task");
        return;
    }

    let name = format!("{DRAFT_NAME_PREFIX}{}", slugify_skill_name(task));
    if let Err(e) = validate_skill_name_rule(&name) {
        warn!(skill = %name, "skill auto-extract skipped: invalid draft name: {e}");
        return;
    }

    let md = draft_skill_md(task, DRAFT_APPROACH, summary, &name);
    let manifest = match parse_skill_md(md.as_bytes()) {
        Ok(m) => m,
        Err(e) => {
            warn!(skill = %name, "skill auto-extract skipped: draft failed to parse: {e}");
            return;
        }
    };

    // Single write-lock acquisition: existence check + registration are
    // atomic, so a concurrent extractor cannot double-register.
    match registry.write() {
        Ok(mut reg) => {
            if reg.get(&name).is_some() {
                info!(skill = %name, "skill auto-extract skipped: already registered");
                return;
            }
            let prompt_template = manifest.prompt_template.as_deref().unwrap_or(md.as_str());
            match reg.create_skill_from_prompt(
                &name,
                &manifest.description,
                prompt_template,
                HashMap::new(),
            ) {
                Ok(()) => info!(skill = %name, "skill auto-extract: draft registered"),
                Err(e) => warn!(skill = %name, "skill auto-extract failed to register: {e}"),
            }
        }
        Err(e) => {
            warn!("skill auto-extract skipped: registry lock poisoned: {e}");
        }
    }
}

/// Slugify a task description into a valid skill name (lowercase ASCII
/// alphanumerics, separators collapsed to `-`), capped so the final
/// `draft-`-prefixed name stays within the registry's 64-char limit.
fn slugify_skill_name(task: &str) -> String {
    const MAX_BASE_CHARS: usize = 56;
    let mut out = String::new();
    let mut pending_sep = false;
    for ch in task.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('-');
            }
            pending_sep = false;
            out.push(ch.to_ascii_lowercase());
            if out.len() >= MAX_BASE_CHARS {
                break;
            }
        } else if !out.is_empty() {
            pending_sep = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "extracted-task".to_string()
    } else {
        trimmed
    }
}

/// Collapse whitespace/newlines and cap the length of a line embedded into
/// the draft, so arbitrary task/summary text cannot break the YAML
/// frontmatter or balloon the draft size.
fn single_line(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::shared::truncate::truncate_chars(&collapsed, max_chars, "...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_skill_md_has_expected_sections() {
        let md = draft_skill_md(
            "Write a regression test for the rate limiter",
            "Record the failing case, patch the code, rerun the suite",
            "All 42 tests pass",
            "draft-rate-limiter-regression-test",
        );
        assert!(md.contains("name: draft-rate-limiter-regression-test"));
        assert!(md.contains("description:"));
        assert!(md.contains("## When to Use"));
        assert!(md.contains("## Steps"));
        assert!(md.contains("rate limiter"));
    }

    #[test]
    fn draft_skill_md_roundtrips_through_parse_skill_md() {
        let md = draft_skill_md(
            "Task: fix the build",
            "approach with a colon: step one",
            "outcome",
            "draft-fix-build",
        );
        let manifest = parse_skill_md(md.as_bytes()).unwrap();
        assert_eq!(manifest.name, "draft-fix-build");
        // Colons inside the description must not break frontmatter parsing.
        assert!(manifest.description.contains("fix the build"));
        // The full markdown is preserved as the prompt template.
        assert_eq!(manifest.prompt_template.as_deref(), Some(md.as_str()));
    }

    #[tokio::test]
    async fn auto_extract_disabled_does_nothing() {
        let registry = Arc::new(RwLock::new(SkillRegistry::default()));
        maybe_auto_extract(registry.clone(), false, "some task", "some summary").await;
        let reg = registry.read().unwrap();
        assert!(reg.list(true).is_empty());
    }

    #[tokio::test]
    async fn auto_extract_registers_draft_and_skips_existing() {
        let registry = Arc::new(RwLock::new(SkillRegistry::default()));
        maybe_auto_extract(
            registry.clone(),
            true,
            "Review pull requests",
            "Merged 3 PRs",
        )
        .await;
        {
            let reg = registry.read().unwrap();
            let listed = reg.list(true);
            assert_eq!(listed.len(), 1);
            let name = listed[0].name.clone();
            assert!(name.starts_with("draft-"));
            // The draft is a real, registered, invocable skill.
            assert!(reg.get(&name).is_some());
            assert!(!reg.is_hidden(&name));
        }

        // Same task again -> skipped, no duplicate draft.
        maybe_auto_extract(
            registry.clone(),
            true,
            "Review pull requests",
            "Merged 3 PRs",
        )
        .await;
        let reg = registry.read().unwrap();
        assert_eq!(reg.list(true).len(), 1);
    }

    #[tokio::test]
    async fn auto_extract_derives_slug_from_task() {
        let registry = Arc::new(RwLock::new(SkillRegistry::default()));
        maybe_auto_extract(
            registry.clone(),
            true,
            "Fix failing cargo tests",
            "all green",
        )
        .await;
        let reg = registry.read().unwrap();
        let names: Vec<String> = reg.list(true).into_iter().map(|d| d.name).collect();
        assert!(names.iter().any(|n| n == "draft-fix-failing-cargo-tests"));
    }

    #[tokio::test]
    async fn auto_extract_handles_non_ascii_and_empty_tasks() {
        let registry = Arc::new(RwLock::new(SkillRegistry::default()));
        maybe_auto_extract(registry.clone(), true, "修复构建 ✨", "green").await;
        {
            // Scope the read guard so it drops before the await below.
            let reg = registry.read().unwrap();
            assert_eq!(reg.list(true).len(), 1);
            assert!(reg.list(true)[0].name.starts_with("draft-"));
        }

        let registry = Arc::new(RwLock::new(SkillRegistry::default()));
        maybe_auto_extract(registry.clone(), true, "   ", "summary").await;
        let reg = registry.read().unwrap();
        assert!(reg.list(true).is_empty());
    }
}
