# Skills 擴展

Skills 系統為 go-on 運行時提供了一種輕量、類型安全的能力單元註冊與分發機制，
類似於 MCP Tools，但直接內嵌在運行時中，無需額外網絡調用。

---

## 啟用 Skills

透過 `config.toml`（或 `config/` 下的服務端預設）中的單一開關控制：

```toml
[runtime]
skills_enabled = true   # true = 啟動時加載內置 skills（默認值）
                        # false = 不加載（生產環境推薦）
```

> **生產環境提示**：`config/config.simple-server.toml` 與 `config/config.multi-users-server.toml` 默認 `skills_enabled = false`，
> 以減少暴露面。如有需要可顯式開啟。

---

## 內置 Skills

| 名稱           | 說明                                       |
|----------------|--------------------------------------------|
| `builtin.echo` | 原樣返回輸入值（冒煙測試用 skill）          |

---

## Skill 命名規則

通過內部 `SkillRegistry::register()` API 註冊自定義 skill 時，名稱必須滿足：

| 約束     | 規則                                                     |
|----------|----------------------------------------------------------|
| 長度     | 1–64 字符                                                |
| 允許字符 | `a-z`、`0-9`、`.`、`_`、`-`                             |
| 唯一性   | 名稱不可重複，重複註冊將返回錯誤                          |
| Schema   | `input_schema()` 必須返回 JSON **對象**（`{…}`）         |

---

## 實現自定義 Skill

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

    fn description(&self) -> &str { "根據給定名字返回問候語" }

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

服務器啟動後註冊：

```rust
server.register_skill(Arc::new(GreetSkill));
```

> 註冊失敗（名稱重複、命名不合法、Schema 非對象）僅記錄 `WARN` 日誌，
> 不會導致服務器崩潰。

---

## 相關文檔

- [架構總覽](overview.md)
- [後端 CLI](backend-cli.md)
