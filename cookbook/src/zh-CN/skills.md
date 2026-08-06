# Skills 扩展

Skills 系统为 go-on 运行时提供了一种轻量、类型安全的能力单元注册与分发机制，
类似于 MCP Tools，但直接内嵌在运行时中，无需额外网络调用。

---

## 启用 Skills

通过 `config.toml`（或 `config/` 下的服务端预设）中的单一开关控制：

```toml
[runtime]
skills_enabled = true   # true = 启动时加载内置 skills（默认值）
                        # false = 不加载（生产环境推荐）
```

> **生产环境提示**：`config/config.simple-server.toml` 与 `config/config.multi-users-server.toml` 默认 `skills_enabled = false`，
> 以减少暴露面。如有需要可显式开启。

---

## 内置 Skills

| 名称           | 说明                                       |
|----------------|--------------------------------------------|
| `builtin.echo` | 原样返回输入值（冒烟测试用 skill）          |

---

## Skill 命名规则

通过内部 `SkillRegistry::register()` API 注册自定义 skill 时，名称必须满足：

| 约束     | 规则                                                     |
|----------|----------------------------------------------------------|
| 长度     | 1–64 字符                                                |
| 允许字符 | `a-z`、`0-9`、`.`、`_`、`-`                             |
| 唯一性   | 名称不可重复，重复注册将返回错误                          |
| Schema   | `input_schema()` 必须返回 JSON **对象**（`{…}`）         |

---

## 实现自定义 Skill

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

    fn description(&self) -> &str { "根据给定名字返回问候语" }

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
        Ok(json!({ "greeting": format!("你好，{name}！") }))
    }
}
```

服务器启动后注册：

```rust
server.register_skill(Arc::new(GreetSkill));
```

> 注册失败（名称重复、命名不合法、Schema 非对象）仅记录 `WARN` 日志，
> 不会导致服务器崩溃。

---

## 相关文档

- [架构总览](overview.md)
- [后端 CLI](backend-cli.md)
