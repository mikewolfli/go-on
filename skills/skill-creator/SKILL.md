---
name: skill-creator
description: Guides users through creating, testing, and publishing new skills with proper SKILL.md format and best practices
version: 1.0.0
author: go-on-team
tags: [skill, meta, creation, scaffolding, template, publishing]
min_go_on_version: 1.0.0
---

# Skill Creator Skill

A meta-skill that helps users design, scaffold, test, and publish new Go-On skills. Walks through the complete skill lifecycle: ideation → YAML frontmatter → input/output schema design → example generation → local testing → GitHub PR submission.

## How It Works

1. **Ideate** — Asks guiding questions to define the skill's purpose, target audience, and unique value proposition
2. **Scaffold** — Generates a complete `SKILL.md` with proper YAML frontmatter, description, input schema table, and realistic JSON examples
3. **Validate** — Checks the generated skill against Go-On's skill requirements (non-empty name, valid characters, proper JSON schema types, example matches schema)
4. **Test** — Produces a sample invocation that can be used with `SkillExecuteTool` to verify the skill works end-to-end
5. **Publish** — Generates the files and directory structure needed for a GitHub PR to the `skills/` directory in the Go-On repository

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `name` | string | Skill name (kebab-case, 3-50 chars) |
| `description` | string | One-line description of what the skill does |
| `inputs` | array | Array of input parameter definitions: `[{"name": "...", "type": "...", "description": "..."}]` |
| `tags` | string[] | Optional: categorization tags for the skill market |
| `language` | string | Optional: primary language the skill targets |
| `include_example` | boolean | Optional: include a realistic usage example (default: true) |

## Example

```json
{
  "name": "translation-helper",
  "description": "Translates code comments, documentation, and user-facing strings between languages while preserving code structure",
  "inputs": [
    {"name": "text", "type": "string", "description": "Text to translate"},
    {"name": "source_lang", "type": "string", "description": "Source language code (e.g. zh-CN)"},
    {"name": "target_lang", "type": "string", "description": "Target language code (e.g. en)"},
    {"name": "preserve_code", "type": "boolean", "description": "Preserve code blocks and inline code"}
  ],
  "tags": ["translation", "i18n", "documentation", "localization"],
  "language": "general"
}
```

**Example output (abbreviated):**

```markdown
---
name: translation-helper
description: Translates code comments, documentation, and user-facing strings between languages while preserving code structure
version: 1.0.0
author: [your-github-username]
tags: [translation, i18n, documentation, localization]
min_go_on_version: 1.0.0
---

# Translation Helper Skill

Translates code comments, documentation, and user-facing strings between languages while preserving code structure, markdown formatting, and template syntax.

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `text` | string | Text to translate |
| `source_lang` | string | Source language code (e.g. zh-CN) |
| `target_lang` | string | Target language code (e.g. en) |
| `preserve_code` | boolean | Preserve code blocks and inline code |

## Example

```json
{
  "text": "// 计算用户的年龄（从出生日期）\nfn calculate_age(birth_date: NaiveDate) -> u32 { ... }",
  "source_lang": "zh-CN",
  "target_lang": "en",
  "preserve_code": true
}
```

## Validation Results
- ✅ Name: translation-helper (kebab-case, valid)
- ✅ Input schema: 4 parameters, all have name/type/description
- ✅ Description: ≤200 chars
- ✅ Example matches input schema

## Next Steps
1. Save this to `skills/translation-helper/SKILL.md`
2. Add an `icon.png` (optional, max 128×128)
3. Test locally: `SkillExecuteTool("translation-helper", {...})`
4. Submit a PR to the go-on repository
```
