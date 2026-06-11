//! Skill system — registry, execution, and built-in skills.
//!
//! ## Sub-modules
//!
//! * [`registry`] — Skill registry, stats, persistence
//! * [`execution`] — Skill trait, prompt-based skills, composed skills, built-ins

pub mod execution;
pub mod registry;

// Re-exports for backward compatibility
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

        let listed = registry.list();
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
}
