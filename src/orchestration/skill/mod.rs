//! Skill system — registry, execution, and built-in skills.
//!
//! ## Sub-modules
//!
//! * [`registry`] — Skill registry, stats, persistence
//! * [`execution`] — Skill trait, prompt-based skills, composed skills, built-ins

pub mod auto_extract;
pub mod bundle; // M4.2: skills as plugins — installable capability bundles
pub mod discovery_cache;
pub mod execution;
pub mod registry;
pub mod usage;

// Re-exports for backward compatibility
// NOTE: registry::spawn_skill_refresh_task is available via the wildcard re-export below.
pub use execution::*;
pub use registry::*;

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use std::time::Duration;

    struct EchoSkill;

    #[async_trait]
    impl Skill for EchoSkill {
        fn name(&self) -> &str {
            "echo_skill"
        }

        fn description(&self) -> &str {
            "Echoes input"
        }

        async fn execute(&self, input: &Value) -> Result<Value> {
            Ok(input.clone())
        }
    }

    #[tokio::test]
    async fn registry_lists_and_executes_skills() {
        let mut registry = SkillRegistry::default();
        registry.register(Arc::new(EchoSkill)).unwrap();

        let listed = registry.list(false);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "echo_skill");

        let skill = registry.get("echo_skill").unwrap();
        let result = skill.execute(&json!({"value": 1})).await.unwrap();
        assert_eq!(result["value"], 1);
    }

    #[test]
    fn register_rejects_empty_name() {
        struct BadSkill;
        #[async_trait]
        impl Skill for BadSkill {
            fn name(&self) -> &str {
                ""
            }
            async fn execute(&self, input: &Value) -> Result<Value> {
                Ok(input.clone())
            }
        }
        let mut registry = SkillRegistry::default();
        assert!(registry.register(Arc::new(BadSkill)).is_err());
    }

    #[test]
    fn register_rejects_name_too_long() {
        let long_name = "a".repeat(65);
        struct LongSkill(String);
        #[async_trait]
        impl Skill for LongSkill {
            fn name(&self) -> &str {
                &self.0
            }
            async fn execute(&self, input: &Value) -> Result<Value> {
                Ok(input.clone())
            }
        }
        let mut registry = SkillRegistry::default();
        assert!(registry.register(Arc::new(LongSkill(long_name))).is_err());
    }

    #[test]
    fn register_rejects_invalid_chars() {
        struct BadCharsSkill;
        #[async_trait]
        impl Skill for BadCharsSkill {
            fn name(&self) -> &str {
                "Bad Skill!"
            }
            async fn execute(&self, input: &Value) -> Result<Value> {
                Ok(input.clone())
            }
        }
        let mut registry = SkillRegistry::default();
        assert!(registry.register(Arc::new(BadCharsSkill)).is_err());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut registry = SkillRegistry::default();
        registry.register(Arc::new(EchoSkill)).unwrap();
        let err = registry.register(Arc::new(EchoSkill)).unwrap_err();
        assert!(err.to_string().contains("error.skill_already_registered"));
    }

    #[test]
    fn register_rejects_non_object_schema() {
        struct BadSchemaSkill;
        #[async_trait]
        impl Skill for BadSchemaSkill {
            fn name(&self) -> &str {
                "bad-schema"
            }
            fn input_schema(&self) -> Value {
                json!("not-an-object")
            }
            async fn execute(&self, input: &Value) -> Result<Value> {
                Ok(input.clone())
            }
        }
        let mut registry = SkillRegistry::default();
        assert!(registry.register(Arc::new(BadSchemaSkill)).is_err());
    }

    #[tokio::test]
    async fn builtin_echo_skill_roundtrips() {
        let skill = super::EchoSkill;
        assert_eq!(skill.name(), "builtin.echo");
        let input = json!({"key": "value", "num": 42});
        let output: Value = skill.execute(&input).await.unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn unregister_removes_skill_and_stats() {
        let mut registry = SkillRegistry::default();
        registry.register(Arc::new(EchoSkill)).unwrap();
        registry.record_outcome("echo_skill", true, Duration::from_millis(12));

        assert!(registry.unregister("echo_skill"));
        assert!(registry.get("echo_skill").is_none());
        assert!(registry.score_of("echo_skill").is_none());
        assert!(!registry.unregister("echo_skill"));
    }

    #[test]
    fn discover_local_skills_skips_nonexistent_dir() {
        let mut registry = SkillRegistry::default();
        let tmp = std::env::temp_dir().join("go-on-test-nonexistent");
        let summary = registry
            .discover_and_register_local_skills(Some(&tmp))
            .unwrap();
        assert_eq!(summary.registered, 0);
        assert_eq!(summary.skipped, 0);
        assert!(summary.errors.is_empty());
    }

    #[test]
    fn discover_local_skills_parses_skill_md() {
        use std::fs;

        let tmp = std::env::temp_dir().join("go-on-test-skills");
        let _ = fs::remove_dir_all(&tmp);

        // Create a valid SKILL.md
        let skill_dir = tmp.join("test-agent");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-agent\ndescription: A test agent skill\n---\n\n# Test Agent\n\nThis is a test skill.",
        )
        .unwrap();

        let mut registry = SkillRegistry::default();
        let summary = registry
            .discover_and_register_local_skills(Some(&tmp))
            .unwrap();

        assert_eq!(summary.registered, 1);
        assert_eq!(summary.skipped, 0);
        assert!(summary.errors.is_empty());

        let skill = registry.get("test-agent");
        assert!(skill.is_some());
        assert_eq!(skill.unwrap().description(), "A test agent skill");

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_local_skills_skips_already_registered() {
        use std::fs;

        let tmp = std::env::temp_dir().join("go-on-test-skills-dup");
        let _ = fs::remove_dir_all(&tmp);

        // Create a valid SKILL.md
        let skill_dir = tmp.join("my-dup-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-dup-skill\ndescription: Duplicate test\n---\n\n# Duplicate",
        )
        .unwrap();

        let mut registry = SkillRegistry::default();

        // First discovery — should register
        let s1 = registry
            .discover_and_register_local_skills(Some(&tmp))
            .unwrap();
        assert_eq!(s1.registered, 1);
        assert_eq!(registry.list(true).len(), 1);

        // Second discovery — should skip (already registered)
        let s2 = registry
            .discover_and_register_local_skills(Some(&tmp))
            .unwrap();
        assert_eq!(s2.registered, 0);
        assert_eq!(s2.skipped, 1);

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_local_skills_skips_missing_skill_md() {
        use std::fs;

        let tmp = std::env::temp_dir().join("go-on-test-nomd");
        let _ = fs::remove_dir_all(&tmp);

        // Create a directory with no SKILL.md
        let empty_dir = tmp.join("empty-agent");
        fs::create_dir_all(&empty_dir).unwrap();

        let mut registry = SkillRegistry::default();
        let summary = registry
            .discover_and_register_local_skills(Some(&tmp))
            .unwrap();

        assert_eq!(summary.registered, 0);
        assert_eq!(summary.skipped, 1);

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }
}
