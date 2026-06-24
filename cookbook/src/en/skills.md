# Skills Extension

The skills system provides a lightweight, type-safe mechanism for registering and
dispatching reusable capability units — similar to MCP tools but embedded directly
in the go-on runtime.

---

## Enabling Skills

Skills are controlled by a single configuration flag in `config.toml` (or
`config.production.toml`):

```toml
[runtime]
skills_enabled = true   # true = load builtin skills on startup (default)
                        # false = no skills loaded (recommended for production)
```

> **Production note**: `config.production.toml` ships with `skills_enabled = false`
> to minimise the exposed tool surface.  Enable it explicitly if you need builtin
> skills in production.

---

## Builtin Skills

| Name                  | Description                                                    |
|-----------------------|----------------------------------------------------------------|
| `builtin.echo`        | Returns the input value unchanged (smoke-test skill)           |
| `skill-creator`       | Creates new prompt-based skills from structured definitions    |

---

## Skill Name Rules

Custom skills registered via the internal `SkillRegistry::register()` API must
follow these rules:

| Constraint     | Rule                                                          |
|----------------|---------------------------------------------------------------|
| Length         | 1–64 characters                                               |
| Allowed chars  | `a-z`, `0-9`, `.`, `_`, `-`                                  |
| Uniqueness     | Name must be unique; duplicates are rejected with an error    |
| Schema         | `input_schema()` must return a JSON **object** (`{…}`)       |

---

## Skill Tools

Skill management is exposed to AI models through two built-in tools:

| Tool | Description |
|------|-------------|
| `skill_list` | Lists all registered skills with name, description, and score |
| `skill_execute` | Executes a registered skill by name with the given input |
| `skill-finder` | Searches for skills by natural language query using token-based similarity |
| `skill-creator` | Creates new prompt-based skills from a template |
| `import_skill` | Imports a skill from a remote URL or GitHub repository |
| `github_search_skills` | Searches GitHub for skill repositories |

## Local Skill Discovery

Place a `SKILL.md` file in `~/.agents/skills/<skill-name>/` with YAML frontmatter:

```markdown
---
name: my-skill
description: A description of what this skill does
---

Instructions for the skill go here.
```

Skills are automatically discovered when go-on starts. The `spawn_skill_refresh_task` background task periodically rescans the directory for new skills.

## Implementing a Custom Skill

```rust
use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use crate::orchestration::skill::Skill;

pub struct GreetSkill;

#[async_trait]
impl Skill for GreetSkill {
    fn name(&self) -> &str { "my.greet" }

    fn description(&self) -> &str { "Returns a greeting for the given name" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, input: &Value) -> Result<Value> {
        let name = input["name"].as_str().unwrap_or("stranger");
        Ok(json!({ "greeting": format!("Hello, {name}!") }))
    }
}
```

Register it on the server after startup:

```rust
server.register_skill(Arc::new(GreetSkill));
```

> Failed registrations (duplicate name, invalid name, bad schema) are logged as
> `WARN` level and do not crash the server.

---

## Related

- [Architecture Overview](overview.md)
- [Backend CLI](backend-cli.md)
