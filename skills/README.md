# 🧩 Go-On Skill Marketplace

This directory contains community-contributed skills for the Go-On AGI platform.

## Directory structure

```
skills/
  <skill-name>/
    SKILL.md       # Required: skill manifest
    icon.png       # Optional: skill icon (max 128x128)
    tests/         # Optional: smoke tests
```

## SKILL.md format

```markdown
---
name: my-skill
description: A brief description of what this skill does
version: 1.0.0
author: github-username
tags: [code, review, rust]
min_go_on_version: 1.0.0
---

# My Skill

Detailed documentation for this skill.
```

## CI pipeline

Every PR that modifies `skills/` triggers the `skill-market.yml` workflow:

1. **Validate** — Checks that each skill has a valid `SKILL.md` with required fields
2. **Index** — Builds `goon-skill-index.yaml` from all skills
3. **Smoke test** — Verifies go-on can parse the manifests
4. **PR comment** — Posts validation results
5. **Publish** — (manual) Deploys index to GitHub Pages

## Publishing a skill

1. Fork the repository
2. Create a directory under `skills/` with your skill name
3. Add a `SKILL.md` (see format above)
4. Submit a Pull Request
4. After review and merge, the index is auto-updated

## Available Skills (34 skills)

### Built-in Skills (34)

| Skill | Tags | Description |
|-------|------|-------------|
| [**analyze-text**](classify-text/SKILL.md) | classify, embed, text, nlp | Classify text or generate semantic embeddings — merged from classify-text + embed-text |
| [api-docs-generator](api-docs-generator/SKILL.md) | api, docs, generation | Generates API documentation from code |
| [api-tester](api-tester/SKILL.md) | api, testing, validation | Tests API endpoints and validates responses |
| [architecture-diagrammer](architecture-diagrammer/SKILL.md) | architecture, diagram, visualization | Generates architecture diagrams from code |
| [**conventional-commits-toolkit**](changelog-generator/SKILL.md) | changelog, commit, git, release | Generates changelogs and conventional commit messages — merged from changelog-generator + commit-message-generator |
| [ci-pipeline-generator](ci-pipeline-generator/SKILL.md) | ci, pipeline, devops | Generates CI/CD pipeline configurations |
| [code-execution-sandbox](code-execution-sandbox/SKILL.md) | execution, sandbox, safety | Executes untrusted code in a sandboxed environment |
| [code-review](code-review/SKILL.md) | code, review, quality, pr | Two-mode code review: git diff review (Standards + Spec) + static snippet analysis |
| [context-summarizer](context-summarizer/SKILL.md) | context, summary, compression | Summarizes long conversations and context windows |
| [data-pipeline-optimizer](data-pipeline-optimizer/SKILL.md) | data, pipeline, etl | Optimizes data processing pipelines |
| [data-transformer](data-transformer/SKILL.md) | data, transform, conversion | Transforms data between formats and schemas |
| [dockerfile-generator](dockerfile-generator/SKILL.md) | docker, container, devops | Generates optimized Dockerfiles |
| [env-config-validator](env-config-validator/SKILL.md) | env, config, validation | Validates environment configuration |
| [error-recovery-planner](error-recovery-planner/SKILL.md) | error, recovery, resilience | Plans error recovery strategies |
| [knowledge-retriever](knowledge-retriever/SKILL.md) | knowledge, retrieval, search | Retrieves knowledge from documentation and codebase |
| [log-analyzer](log-analyzer/SKILL.md) | log, analysis, debugging | Analyzes application logs for errors and patterns |
| [**note-taking**](note-taking/SKILL.md) | note, decision, writing, organization | Maintain notes + architectural decision records — now includes decision-logger functionality |
| [performance-analyzer](performance-analyzer/SKILL.md) | performance, profiling, optimization | Analyzes code performance and bottlenecks |
| [progress-tracker](progress-tracker/SKILL.md) | progress, tracking, status | Tracks progress across tasks and milestones |
| [**project-analyzer**](project-analyzer/SKILL.md) | project, dependency, analysis, security | Analyzes project structure AND deep-audits dependencies — merged from project-analyzer + dependency-analyzer |
| [**prompt-optimizer**](prompt-optimizer/SKILL.md) | prompt, optimization, LLM | Analyzes and improves LLM prompts for clarity and efficiency |
| [refactoring-advisor](refactoring-advisor/SKILL.md) | refactoring, code-quality, improvement | Advises on code refactoring opportunities |
| [regex-builder](regex-builder/SKILL.md) | regex, pattern, text | Builds and tests regular expressions |
| [security-auditor](security-auditor/SKILL.md) | security, audit, vulnerability | Audits code for security vulnerabilities |
| [**self-reviewer**](self-reviewer/SKILL.md) | review, code-quality, self-improvement | Performs structured self-review of agent work |
| [semantic-diff](semantic-diff/SKILL.md) | diff, semantic, comparison | Analyze code changes semantically |
| [skill-creator](skill-creator/SKILL.md) | skill, meta, creation | Guides creation of new Go-On skills |
| [sql-query-helper](sql-query-helper/SKILL.md) | sql, database, query | Helps write and optimize SQL queries |
| [summarize-text](summarize-text/SKILL.md) | summarize, text, compression | Summarize long text into concise, structured summaries |
| [task-planner](task-planner/SKILL.md) | task, planning, execution | Plans and decomposes complex tasks |
| [test-generator](test-generator/SKILL.md) | test, generation, quality | Generates unit and integration tests |
| [translate-text](translate-text/SKILL.md) | translate, i18n, language | Translate text between languages |
| [web-scraper](web-scraper/SKILL.md) | web, scraping, data-extraction | Scrapes and extracts data from websites |
| [**workflow-optimizer**](workflow-optimizer/SKILL.md) | workflow, optimization, pipeline | Analyzes and optimizes multi-step workflows |

### External Agent Skills (installed at `~/.agents/skills`, not in this repo)

The following global agent skills live in the user's `~/.agents/skills/` directory
(auto-discovered by go-on on startup); they are **not** part of this repository's
`skills/` marketplace. The list reflects what is actually installed on the
developer machine at the time of writing:

| Skill | Source | Description |
|-------|--------|-------------|
| grilling | productivity | Relentless one-at-a-time interview to stress-test plans/decisions |
| grill-with-docs | engineering | Grilling + domain-modeling that produces ADRs and glossary |
| code-review | engineering | Two-axis (Standards + Spec) code review with parallel sub-agents |
| diagnosing-bugs | engineering | 6-phase structured debugging protocol |
| domain-modeling | engineering | Build and sharpen domain model with glossary + ADRs |
| wayfinder | engineering | Multi-session work planning via decision-ticket maps |

> Note: previously-listed `handoff`, `research`, `triage`, and `implement` are
> not installed in this workspace's `~/.agents/skills/` and have been removed
> from this list. These skills (if desired) live outside this repository.

> The backend's `builtin_skills()` fallback (`src/orchestration/skill_market.rs`)
> mirrors this list under the same 34 names, so the marketplace display names
> and the local `skills/` inventory stay in sync.

### Removed (merged into others)

| Removed Skill | Merged Into | Reason |
|---------------|-------------|--------|
| commit-message-generator | conventional-commits-toolkit | Same domain, same Conventional Commits taxonomy |
| decision-logger | note-taking | Decision-logger is schema'd note-taking with ADR format |
| dependency-analyzer | project-analyzer | Overlapping dependency analysis at different depths |
| embed-text | analyze-text | Same input pipeline, different output schemas |
| code-reviewer | code-review | Merged into code-review as `snippet` mode |
| review-pr | code-review | code-review is superset (2-axis + sub-agents) |
| grill-me | grilling | 100% command alias wrapper |
| claude-handoff | handoff | 90% duplicate, only delivery method differs |
