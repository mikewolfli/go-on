# Prompts System

## Overview

The Prompts System is go-on's built-in prompt template management feature, providing **149 ready-to-use templates** across 16 categories (per `prompts/en.json`). Users can browse, search, and insert templates via the GUI, expand templates quickly with `/` commands in Chat, or have AI agents automatically invoke templates through MCP (Model Context Protocol).

> Related docs: [GUI Console](gui.md) | [Workflow Config](workflow-config.md)

---

## Directory Structure

Prompt templates are organized in a layered directory structure:

```
prompts/
├── en.json           # English built-in templates (default)
├── zh-CN.json        # Simplified Chinese built-in templates
├── zh-TW.json        # Traditional Chinese built-in templates
└── custom/
    ├── en.json       # English custom templates
    ├── zh-CN.json    # Simplified Chinese custom templates
    └── zh-TW.json    # Traditional Chinese custom templates
```

- `prompts/{lang}.json` — Built-in templates, shipped with the system, read-only
- `prompts/custom/{lang}.json` — Custom templates, users can freely create, edit, and delete
- Language auto-switch: when the GUI language changes, the system automatically loads the corresponding language template file

### Supported Languages

| Language Code | Name       |
|---------------|------------|
| `en`          | English    |
| `zh-CN`       | Simplified Chinese |
| `zh-TW`       | Traditional Chinese |

When switching the GUI language, the prompt template system automatically loads the corresponding language file — no manual switching needed.

---

## 16 Categories

Templates are grouped into 16 categories with varying counts, totaling **149 templates**:

| # | Category | Templates | Representative Templates |
|---|----------|-----------|--------------------------|
| 1 | Software Development | 10 | `explain_code` / `code_review` / `generate_unit_test` |
| 2 | Writing & Creative | 12 | `blog_post_outline` / `proofread_text` / `creative_story` |
| 3 | Academic Research | 8 | `literature_review` / `abstract_generation` / `peer_review` |
| 4 | Business Analysis | 9 | `swot_analysis` / `business_plan` / `competitive_analysis` |
| 5 | Marketing | 13 | `marketing_strategy` / `ad_copy` / `social_media_content` |
| 6 | Legal & Compliance | 11 | `contract_review` / `contract_clause` / `compliance_checklist` |
| 7 | Medical & Health | 8 | `symptom_analysis` / `medication_guide` / `treatment_plan` |
| 8 | Education & Training | 10 | `lesson_plan` / `quiz_generation` / `explain_concept` |
| 9 | Finance & Investment | 11 | `investment_analysis` / `budget_planning` / `financial_report` |
| 10 | Data Science | 11 | `eda_plan` / `model_selection` / `sql_query` |
| 11 | Design & Creative | 11 | `ux_review` / `design_brief` / `accessibility_audit` |
| 12 | System Operations | 10 | `incident_response` / `monitoring_setup` / `security_hardening` |
| 13 | Productivity | 8 | `requirements_breakdown` / `prd_draft` / `meeting_minutes` |
| 14 | Engineering Delivery | 6 | `release_notes` / `rca_report` / `rollback_plan` |
| 15 | Operations Support | 6 | `customer_reply` / `kb_article` / `faq_builder` |
| 16 | Go-On Agent Skills | 5 | `skill_discovery` / `tool_selection` / `best_practices` |

---

## GUI Operations

In the GUI's **Prompts Tab**, you can:

- **Browse** — View all templates grouped by industry category (built-in + custom)
- **Search** — Search template titles and content by keyword
- **Create** — Create new custom templates, select the category and language
- **Edit** — Modify existing custom templates
- **Delete** — Remove unwanted custom templates

### Workflow

1. Switch to the Prompts Tab
2. Select an industry category on the left to filter
3. Use the search box to search by keyword
4. Click a template card to view details
5. Click the **Insert to Chat** button to insert template content into the Chat input box
6. Fine-tune the template in Chat and send

---

## Chat `/` Commands

In the Chat input box, type `/` to trigger command completion. Type a template ID and press Enter to expand the template content directly.

### Command Format

```
/<template_id> <parameters>
```

Examples:
- `/explain_code` — Explain selected code
- `/code_review` — Review code quality
- `/generate_unit_test Rust` — Generate unit tests for Rust code
- `/blog_post_outline topic: AI trends` — Write an article on the specified topic

### Common Commands

| Command | Function |
|---------|----------|
| `/explain_code` | Explain selected code |
| `/code_review` | Review code quality |
| `/refactor_suggestion` | Suggest code refactorings |
| `/generate_unit_test` | Generate unit tests |
| `/debug_error` | Debug an error message |
| `/generate_documentation` | Write documentation comments |
| `/blog_post_outline` | Outline an article |
| `/marketing_strategy` | Marketing strategy |
| `/contract_review` | Contract review |
| `/literature_review` | Literature review |

When you type `/`, the system shows an autocomplete list with fuzzy search support, category filtering, and full template ID matching.

---

## Custom Templates

### Creating a Template

1. In the Prompts Tab, click **Create New Template**
2. Fill in the template details:
   - **ID** — Unique identifier for `/` command invocation (e.g., `my_custom_template`)
   - **Title** — Display name of the template
   - **Category** — Industry category
   - **Language** — Template language
   - **Content** — Prompt text, supports `{{variable}}` placeholders
   - **Description** — Brief description of the template's purpose
3. Save — the template will be written to `prompts/custom/{lang}.json`

### Editing a Template

Find the custom template in the Prompts Tab, click the edit button to modify. After saving, the system updates `prompts/custom/{lang}.json`.

### Deleting a Template

Find the custom template in the Prompts Tab, click the delete button and confirm.

> ⚠️ Note: Only custom templates can be deleted; built-in templates are read-only.

---

## Backend RPC Interface

The prompts system provides the following RPC interfaces:

| RPC | Method | Description |
|-----|--------|-------------|
| `prompts.list` | List | Get all templates, filterable by category and language |
| `prompts.search` | Search | Search templates by keyword |
| `prompts.get` | Get | Get a single template's details |
| `prompts.create` | Create | Create a new custom template |
| `prompts.update` | Update | Update a custom template |
| `prompts.delete` | Delete | Delete a custom template |

---

## MCP Tools

Through MCP (Model Context Protocol), AI agents can automatically discover and invoke prompt templates:

| MCP Tool | Function |
|----------|----------|
| `prompts_list` | List all available prompt templates |
| `prompts_get` | Get the detailed content of a specific template |

AI agents discover these tools via the MCP protocol and can automatically select and apply appropriate templates during conversations. For example, when a user says "help me review this code", the AI agent can automatically call `prompts_get` to retrieve the `code_review` template and apply it to the response.

---

## Settings Toggle

In **Settings → Feature Toggles**, you can enable or disable the **Prompts** module. When disabled:

- The Prompts Tab is hidden from the tab bar
- Related RPC interfaces stop responding
- Related MCP tools stop responding

> Default state: **Enabled**
