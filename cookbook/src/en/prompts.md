# Prompts System

## Overview

The Prompts System is go-on's built-in prompt template management feature, providing **84+ ready-to-use templates** across 12 industry categories. Users can browse, search, and insert templates via the GUI, expand templates quickly with `/` commands in Chat, or have AI agents automatically invoke templates through MCP (Model Context Protocol).

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

## 12 Industry Categories

Each category contains 7 templates, totaling **84+ templates** covering typical use cases across industries.

| # | Category | Description | Representative Templates |
|---|----------|-------------|------------------------|
| 1 | Software Development | Code generation, debugging, refactoring, code review | `explain_code` Explain code / `review_code` Code review / `generate_test` Generate unit tests |
| 2 | Writing & Creative | Article writing, creative writing, copywriting | `write_article` Write article / `creative_story` Creative story / `copywriting` Copywriting |
| 3 | Academic Research | Paper writing, literature review, data analysis | `write_paper` Paper writing / `literature_review` Literature review / `data_analysis` Data analysis |
| 4 | Business Analysis | Market analysis, business model, competitive analysis | `market_analysis` Market analysis / `business_model` Business model / `competitive_analysis` Competitive analysis |
| 5 | Marketing | Marketing plans, ad copy, social media | `marketing_plan` Marketing plan / `ad_copy` Ad copy / `social_media` Social media content |
| 6 | Legal & Compliance | Contract review, legal advice, compliance checks | `contract_review` Contract review / `legal_advice` Legal advice / `compliance_check` Compliance check |
| 7 | Medical & Health | Medical consultation, health management, drug info | `medical_consult` Medical consultation / `health_plan` Health plan / `drug_info` Drug information |
| 8 | Education & Training | Course design, lesson plans, tutoring | `course_design` Course design / `lesson_plan` Lesson plan / `tutoring` Tutoring |
| 9 | Finance & Investment | Investment analysis, risk assessment, financial reports | `investment_analysis` Investment analysis / `risk_assessment` Risk assessment / `financial_report` Financial report |
| 10 | Data Science | Data analysis, machine learning, data visualization | `data_cleaning` Data cleaning / `ml_model` ML model / `data_viz` Data visualization |
| 11 | Design & Creative | UI/UX design, graphic design, creative brainstorming | `ui_design` UI design / `brand_design` Brand design / `creative_brainstorm` Creative brainstorming |
| 12 | System Operations | Server management, network config, monitoring alerts | `server_setup` Server setup / `network_config` Network config / `monitor_setup` Monitoring setup |

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
- `/review_code` — Review code quality
- `/generate_test Rust` — Generate unit tests for Rust code
- `/write_article topic: AI trends` — Write an article on the specified topic

### Common Commands

| Command | Function |
|---------|----------|
| `/explain_code` | Explain selected code |
| `/review_code` | Review code quality |
| `/optimize_code` | Optimize code performance |
| `/generate_test` | Generate unit tests |
| `/refactor_code` | Refactor code |
| `/write_doc` | Write documentation comments |
| `/write_article` | Write an article |
| `/market_analysis` | Market analysis |
| `/contract_review` | Contract review |
| `/data_analysis` | Data analysis |

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

AI agents discover these tools via the MCP protocol and can automatically select and apply appropriate templates during conversations. For example, when a user says "help me review this code", the AI agent can automatically call `prompts_get` to retrieve the `review_code` template and apply it to the response.

---

## Settings Toggle

In **Settings → Feature Toggles**, you can enable or disable the **Prompts** module. When disabled:

- The Prompts Tab is hidden from the tab bar
- Related RPC interfaces stop responding
- Related MCP tools stop responding

> Default state: **Enabled**
