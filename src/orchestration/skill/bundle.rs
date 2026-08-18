//! M4.2: skills as plugins — installable capability bundles.
//!
//! A [`SkillBundle`] is the plugin form of a skill: a `SKILL.md` plus a
//! configuration section (配置段), an allowed-tools whitelist, context
//! fragments, and event listeners. Installing a bundle registers its skill on
//! the [`SkillRegistry`] and its listeners on the [`EventBus`]; dropping the
//! returned [`BundleRegistration`] rolls every registration back via the
//! M1.6 guards, so uninstall is exactly the inverse of install and a
//! half-installed bundle can never be left behind.
//!
//! # Bundle format
//!
//! A bundle is distributed as a single `SKILL.md` following the
//! agentskills.io convention, extended with a go-on `[goon.bundle]` TOML
//! table:
//!
//! ```markdown
//! ---
//! name: my-bundle
//! description: Does something useful
//! when_to_use:
//!   - When the user asks for X
//!   - During Y
//! ---
//!
//! # Body
//!
//! Detailed instructions for the skill...
//!
//! [goon.bundle]
//! allowed_tools = ["read_file", "search_files"]
//! context_fragments = ["fragment one", "fragment two"]
//! ```
//!
//! * `name`, `description`, and `when_to_use` come from the YAML frontmatter
//!   (the standard agentskills.io fields; `name` is required and is the
//!   registration key).
//! * `[goon.bundle]` is the go-on extension table. Any of the five
//!   [`SkillBundleConfig`] fields may appear there; a field present in the
//!   table takes precedence over the frontmatter. The table must be the last
//!   section of the file (everything from the `[goon.bundle]` line to the end
//!   of the file is parsed as TOML).
//!
//! # `allowed_tools`: whitelist semantics
//!
//! `allowed_tools` is the bundle's tool whitelist: the skill may only call the
//! listed tools. It is enforced at execution time by the layer that dispatches
//! the bundle's tool calls — not by this module, which carries it as
//! configuration on [`SkillBundleConfig`] (see [`SkillBundleConfig::allows_tool`]).
//!
//! Today there is no natural per-skill filter hook in the execution path:
//! `PromptBasedSkill::execute` hands the prompt to the global LLM agent and
//! tool calls are dispatched outside the skill's scope (the `ToolsPreExecute`
//! event carries no skill identity), so enforcing the whitelist here would
//! require touching the tool-execution pipeline. The whitelist is therefore
//! surfaced as configuration for consumers that constrain tool calls (a mode
//! policy, or a future per-skill tool gate) and is documented as such.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::execution::PromptBasedSkill;
use super::registry::{validate_skill_name_rule, SkillRegistry};
use super::Skill;
use crate::orchestration::events::{EventBus, EventListener};
use crate::orchestration::registration::RegistrationGuard;
use crate::orchestration::skill_import::parse_skill_md;

/// The configuration section (配置段) of a bundle: name/description/
/// when_to_use plus the allowed-tools whitelist and the context fragments.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillBundleConfig {
    /// Registration name — validated by the shared skill-name rule at install
    /// time (length, charset, no path-traversal components).
    pub name: String,
    /// Human-readable summary of what the bundle does.
    pub description: String,
    /// Trigger descriptions for when the skill should be used (implicit
    /// invocation hints, mirroring agentskills.io's `when_to_use`).
    pub when_to_use: Vec<String>,
    /// Tool whitelist: an empty list allows every tool (no constraint);
    /// a non-empty list restricts the skill to exactly these tools.
    pub allowed_tools: Vec<String>,
    /// Context fragments injected into the model's context when the bundle's
    /// skill is active (usage is decided by the consuming layer).
    pub context_fragments: Vec<String>,
}

impl SkillBundleConfig {
    /// Whether the whitelist permits `tool_name`.
    ///
    /// An empty whitelist allows every tool (no constraint); a non-empty
    /// whitelist requires the tool to be listed. This is the primitive a
    /// tool-call gate uses to enforce the bundle's whitelist at execution time.
    pub fn allows_tool(&self, tool_name: &str) -> bool {
        self.allowed_tools.is_empty() || self.allowed_tools.iter().any(|t| t == tool_name)
    }
}

/// A capability bundle: the `SKILL.md` body plus its configuration section and
/// event listeners.
///
/// Tools are not carried by the bundle itself — they come from the whitelist
/// (`config.allowed_tools`), which constrains which already-registered tools
/// the bundle's skill may call.
pub struct SkillBundle {
    /// The full raw `SKILL.md` (frontmatter + body). The body is the skill's
    /// prompt template; the frontmatter drives registration metadata.
    pub skill_md: String,
    /// The bundle's 配置段 — name/description/when_to_use/whitelist/fragments.
    pub config: SkillBundleConfig,
    /// Event listeners registered on the event bus while the bundle is
    /// installed (e.g. `ToolsPreExecute` observers/interceptors).
    pub listeners: Vec<Arc<dyn EventListener>>,
}

/// The combined rollback for an installed bundle.
///
/// Dropping it unregisters the skill and removes every listener, so uninstall
/// is the exact inverse of install. The guards are intentionally not `Send`
/// (see `orchestration::registration`); they must be dropped before the
/// registries they point at.
#[derive(Debug)]
pub struct BundleRegistration {
    /// Unregisters the bundle's skill from the registry when dropped.
    pub skill_guard: RegistrationGuard,
    /// Removes each bundle listener from the event bus when dropped.
    pub listener_guards: Vec<RegistrationGuard>,
}

/// The `[goon.bundle]` TOML extension table. Every field is optional; absent
/// fields fall back to the frontmatter-derived value (or an empty default).
#[derive(Debug, Default, Deserialize, Serialize)]
struct BundleTomlSection {
    name: Option<String>,
    description: Option<String>,
    when_to_use: Option<Vec<String>>,
    allowed_tools: Option<Vec<String>>,
    context_fragments: Option<Vec<String>>,
}

/// The TOML table header that introduces the go-on bundle extension section.
const BUNDLE_TABLE_HEADER: &str = "[goon.bundle]";

/// Extract the raw YAML frontmatter (the text between the leading `---` and
/// the closing `---` line) from a SKILL.md, if present. Mirrors the delimiter
/// handling of `skill_import::parse_skill_md`.
fn frontmatter(skill_md: &str) -> Option<&str> {
    let after_prefix = skill_md.strip_prefix("---")?;
    let end = after_prefix.find("\n---")?;
    Some(&after_prefix[..end])
}

/// Parse the `when_to_use` list from YAML frontmatter.
///
/// Supports the block-list form (`when_to_use:` followed by indented `- item`
/// lines) and the scalar form (`when_to_use: item`). Quoted items are
/// unquoted. Unknown keys and blank lines inside a block list are ignored; any
/// other top-level key ends a block list.
fn parse_when_to_use(front: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut collecting = false;
    for line in front.lines() {
        let trimmed = line.trim();
        if let Some((key, value)) = trimmed.split_once(':') {
            if key.trim() == "when_to_use" {
                let value = value.trim();
                if value.is_empty() {
                    // A block list follows on the next indented lines.
                    collecting = true;
                } else {
                    out.push(unquote(value));
                }
            } else {
                // Any other top-level key ends a block list.
                collecting = false;
            }
        } else if collecting {
            if let Some(item) = trimmed.strip_prefix("- ") {
                out.push(unquote(item.trim()));
            } else if !trimmed.is_empty() {
                // A non-list, non-blank line ends the block list.
                collecting = false;
            }
        }
    }
    out
}

/// Strip a single layer of surrounding single/double quotes, matching the
/// frontmatter value handling in `skill_import::parse_skill_md`.
fn unquote(value: &str) -> String {
    value.trim_matches('"').trim_matches('\'').to_string()
}

/// Parse the `[goon.bundle]` TOML extension table from the SKILL.md.
///
/// The table runs from the `[goon.bundle]` header line to the end of the file,
/// so it must be the final section of the document. Returns an empty section
/// when no table is present.
fn parse_bundle_toml(skill_md: &str) -> Result<BundleTomlSection> {
    let Some(start) = skill_md
        .lines()
        .position(|line| line.trim() == BUNDLE_TABLE_HEADER)
    else {
        return Ok(BundleTomlSection::default());
    };
    let section = skill_md
        .lines()
        .skip(start + 1)
        .collect::<Vec<_>>()
        .join("\n");
    // Skip the `[goon.bundle]` header line itself: `toml::from_str` expects
    // the section's fields at the document root, and an unknown nested table
    // would be silently ignored (yielding all-default values) instead of
    // surfacing the fields below.
    toml::from_str(&section).with_context(|| {
        format!(
            "failed to parse {} table at line {} (the table must be the last section of the SKILL.md)",
            BUNDLE_TABLE_HEADER,
            start + 1
        )
    })
}

/// Parse a SKILL.md into the bundle's [`SkillBundleConfig`].
///
/// The frontmatter supplies `name`/`description`/`when_to_use` (via the
/// standard skill-document parser, which also sanitizes the name); the
/// optional `[goon.bundle]` TOML table supplies the go-on extension fields
/// (`allowed_tools`, `context_fragments`) and may override any frontmatter
/// field. `name` is required and must satisfy the shared skill-name rule
/// (validated at install time, not here).
pub fn parse_bundle(skill_md: &str) -> Result<SkillBundleConfig> {
    let manifest = parse_skill_md(skill_md.as_bytes())
        .context("bundle SKILL.md is not a valid skill document")?;
    let section = parse_bundle_toml(skill_md)?;
    let front_when_to_use = frontmatter(skill_md)
        .map(parse_when_to_use)
        .unwrap_or_default();

    Ok(SkillBundleConfig {
        name: section.name.unwrap_or(manifest.name),
        description: section.description.unwrap_or(manifest.description),
        when_to_use: section.when_to_use.unwrap_or(front_when_to_use),
        allowed_tools: section.allowed_tools.unwrap_or_default(),
        context_fragments: section.context_fragments.unwrap_or_default(),
    })
}

/// Build the registry `Skill` for a bundle: a `PromptBasedSkill` whose prompt
/// template is the raw SKILL.md and whose metadata comes from the config
/// (falling back to the SKILL.md frontmatter).
fn build_bundle_skill(bundle: &SkillBundle) -> Result<Arc<dyn Skill>> {
    let manifest = parse_skill_md(bundle.skill_md.as_bytes())
        .context("bundle SKILL.md is not a valid skill document")?;
    let input_schema = match &manifest.input_schema {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| (k.clone(), v.to_string()))
            .collect::<HashMap<String, String>>(),
        _ => HashMap::new(),
    };
    Ok(Arc::new(PromptBasedSkill {
        name: bundle.config.name.clone(),
        description: if bundle.config.description.is_empty() {
            manifest.description
        } else {
            bundle.config.description.clone()
        },
        prompt_template: bundle.skill_md.clone(),
        input_schema,
        timeout_secs: 120,
        max_retries: 2,
        disable_model_invocation: false,
        policy: None,
    }))
}

/// Install a bundle: register its skill on the registry and its listeners on
/// the event bus, returning the combined rollback guards.
///
/// Order of operations:
/// 1. Validate the config name against the shared skill-name rule (the
///    security gate shared with the import pipeline).
/// 2. Register the skill via the registry's guarded path — on validation or
///    duplicate-name error nothing is registered and the error is returned.
/// 3. Register each listener on the event bus (infallible).
///
/// Because every registration after the first succeeds and all guards are
/// returned together, dropping (or rolling back) the returned
/// [`BundleRegistration`] undoes the whole install; a failed install never
/// leaves a half-registered bundle behind.
pub async fn install_bundle(
    registry: Arc<RwLock<SkillRegistry>>,
    bundle: SkillBundle,
    event_bus: &EventBus,
) -> Result<BundleRegistration> {
    // Security gate: the config name is the registration key (and a path
    // component), so it must pass the same rule every other registration path
    // enforces.
    validate_skill_name_rule(&bundle.config.name)?;

    let skill = build_bundle_skill(&bundle)?;

    // Register the skill behind the shared registry lock. The guard's closure
    // holds a raw pointer to the registry value inside the lock, so the guard
    // must be dropped before the `Arc<RwLock<SkillRegistry>>` is (the
    // scoped-guard contract documented on `register_guarded`).
    let skill_guard = {
        let mut registry = registry
            .write()
            .map_err(|_| anyhow::anyhow!("skill registry lock poisoned"))?;
        registry.register_guarded(skill)?
    };

    // `EventBus::register` is infallible, so once the skill guard exists the
    // remaining steps cannot fail; all guards are returned together.
    let listener_guards = bundle
        .listeners
        .iter()
        .map(|listener| event_bus.register(Arc::clone(listener)))
        .collect::<Vec<_>>();

    Ok(BundleRegistration {
        skill_guard,
        listener_guards,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::events::AgentEvent;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Test listener that counts every event it sees without consuming any.
    struct CountingListener {
        seen: Arc<AtomicUsize>,
    }

    impl EventListener for CountingListener {
        fn on_event(&self, _event: &AgentEvent) -> crate::orchestration::events::EventVerdict {
            self.seen.fetch_add(1, Ordering::SeqCst);
            crate::orchestration::events::EventVerdict::Continue
        }
    }

    /// A SKILL.md exercising every bundle field: frontmatter
    /// name/description/when_to_use plus a `[goon.bundle]` extension table.
    fn sample_skill_md() -> String {
        "---\n\
         name: my-bundle\n\
         description: Does something useful\n\
         when_to_use:\n\
         \x20 - When the user asks for X\n\
         \x20 - During Y\n\
         ---\n\
         \n\
         # My Bundle\n\
         \n\
         Detailed instructions.\n\
         \n\
         [goon.bundle]\n\
         allowed_tools = [\"read_file\", \"search_files\"]\n\
         context_fragments = [\"fragment one\", \"fragment two\"]\n"
            .to_string()
    }

    fn test_registry() -> Arc<RwLock<SkillRegistry>> {
        Arc::new(RwLock::new(SkillRegistry::default()))
    }

    /// Build a minimal valid bundle with the given name and listeners.
    fn bundle_with(name: &str, listeners: Vec<Arc<dyn EventListener>>) -> SkillBundle {
        let md = format!(
            "---\nname: {name}\ndescription: Test bundle\n---\n\n# Body\n\nInstructions.\n"
        );
        let config = parse_bundle(&md).expect("sample bundle parses");
        SkillBundle {
            skill_md: md,
            config,
            listeners,
        }
    }

    #[test]
    fn parse_bundle_round_trip() {
        let config = parse_bundle(&sample_skill_md()).unwrap();
        assert_eq!(
            config,
            SkillBundleConfig {
                name: "my-bundle".to_string(),
                description: "Does something useful".to_string(),
                when_to_use: vec![
                    "When the user asks for X".to_string(),
                    "During Y".to_string()
                ],
                allowed_tools: vec!["read_file".to_string(), "search_files".to_string()],
                context_fragments: vec!["fragment one".to_string(), "fragment two".to_string()],
            }
        );
    }

    #[test]
    fn parse_bundle_toml_table_overrides_frontmatter() {
        let md = "---\n\
                  name: base-name\n\
                  description: base description\n\
                  when_to_use:\n\
                  \x20 - Frontmatter trigger\n\
                  ---\n\
                  \n\
                  # Body\n\
                  \n\
                  [goon.bundle]\n\
                  name = \"toml-name\"\n\
                  description = \"toml description\"\n\
                  when_to_use = [\"TOML trigger\"]\n\
                  allowed_tools = [\"search_files\"]\n";
        let config = parse_bundle(md).unwrap();
        assert_eq!(config.name, "toml-name");
        assert_eq!(config.description, "toml description");
        assert_eq!(config.when_to_use, vec!["TOML trigger".to_string()]);
        assert_eq!(config.allowed_tools, vec!["search_files".to_string()]);
        assert!(config.context_fragments.is_empty());
    }

    #[test]
    fn parse_bundle_without_toml_table_defaults_to_empty_lists() {
        let md = "---\nname: plain\n---\n\n# Plain\n\nJust body.\n";
        let config = parse_bundle(md).unwrap();
        assert_eq!(config.name, "plain");
        assert!(config.when_to_use.is_empty());
        assert!(config.allowed_tools.is_empty());
        assert!(config.context_fragments.is_empty());
    }

    #[test]
    fn parse_bundle_rejects_invalid_skill_document() {
        // No name in the frontmatter and no `# heading`: the underlying
        // skill-document parser rejects it.
        let err = parse_bundle("---\ndescription: no name\n---\n\nBody.\n").unwrap_err();
        assert!(
            !err.to_string().is_empty(),
            "rejected document must carry an error"
        );
    }

    #[test]
    fn allowed_tools_whitelist_is_allow_all_when_empty() {
        let config = SkillBundleConfig {
            name: "x".to_string(),
            description: String::new(),
            when_to_use: Vec::new(),
            allowed_tools: Vec::new(),
            context_fragments: Vec::new(),
        };
        assert!(config.allows_tool("any_tool"));
    }

    #[test]
    fn allowed_tools_whitelist_constrains_when_non_empty() {
        let config = SkillBundleConfig {
            name: "x".to_string(),
            description: String::new(),
            when_to_use: Vec::new(),
            allowed_tools: vec!["read_file".to_string(), "search_files".to_string()],
            context_fragments: Vec::new(),
        };
        assert!(config.allows_tool("read_file"));
        assert!(!config.allows_tool("shell_exec"));
    }

    #[tokio::test]
    async fn install_bundle_registers_skill_and_listeners() {
        let registry = test_registry();
        let bus = EventBus::new();
        let seen = Arc::new(AtomicUsize::new(0));
        let listener = Arc::new(CountingListener {
            seen: Arc::clone(&seen),
        });
        let bundle = bundle_with(
            "bundle-a",
            vec![Arc::clone(&listener) as Arc<dyn EventListener>],
        );

        let registration = install_bundle(Arc::clone(&registry), bundle, &bus)
            .await
            .unwrap();

        // The skill is registered with the config's name and description.
        let skill = registry.read().unwrap().get("bundle-a");
        assert!(skill.is_some());
        assert_eq!(skill.unwrap().description(), "Test bundle");

        // The listener receives dispatched events.
        bus.dispatch(&AgentEvent::AgentRequest {
            request_id: "r1".to_string(),
        });
        assert_eq!(seen.load(Ordering::SeqCst), 1);

        drop(registration);
    }

    #[tokio::test]
    async fn dropping_registration_unregisters_skill_and_listeners() {
        let registry = test_registry();
        let bus = EventBus::new();
        let seen = Arc::new(AtomicUsize::new(0));
        let listener = Arc::new(CountingListener {
            seen: Arc::clone(&seen),
        });
        let bundle = bundle_with(
            "bundle-b",
            vec![Arc::clone(&listener) as Arc<dyn EventListener>],
        );

        let registration = install_bundle(Arc::clone(&registry), bundle, &bus)
            .await
            .unwrap();
        assert!(registry.read().unwrap().get("bundle-b").is_some());

        bus.dispatch(&AgentEvent::AgentRequest {
            request_id: "r1".to_string(),
        });
        assert_eq!(seen.load(Ordering::SeqCst), 1);

        drop(registration);

        // Skill unregistered: `get` returns None.
        assert!(registry.read().unwrap().get("bundle-b").is_none());
        // Listener removed: dispatch no longer reaches it.
        bus.dispatch(&AgentEvent::AgentRequest {
            request_id: "r2".to_string(),
        });
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn install_failure_rolls_back_and_leaves_nothing() {
        let registry = test_registry();
        let bus = EventBus::new();
        let seen = Arc::new(AtomicUsize::new(0));
        let listener = Arc::new(CountingListener {
            seen: Arc::clone(&seen),
        });
        let bundle = SkillBundle {
            skill_md: "---\nname: invalid\n---\n\n# Body\n".to_string(),
            config: SkillBundleConfig {
                name: "Invalid Name!".to_string(),
                description: String::new(),
                when_to_use: Vec::new(),
                allowed_tools: Vec::new(),
                context_fragments: Vec::new(),
            },
            listeners: vec![Arc::clone(&listener) as Arc<dyn EventListener>],
        };

        let err = install_bundle(Arc::clone(&registry), bundle, &bus)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("error.skill_name_invalid_chars"));

        // Nothing was registered and no listener was attached.
        assert!(registry.read().unwrap().list(true).is_empty());
        bus.dispatch(&AgentEvent::AgentRequest {
            request_id: "r1".to_string(),
        });
        assert_eq!(seen.load(Ordering::SeqCst), 0);
    }
}
