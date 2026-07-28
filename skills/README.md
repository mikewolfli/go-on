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

## Available Skills (39 skills)

| Skill | Tags | Description |
|-------|------|-------------|
| [api-docs-generator](api-docs-generator/SKILL.md) | api, docs, generation | Generates API documentation from code |
| [api-tester](api-tester/SKILL.md) | api, testing, validation | Tests API endpoints and validates responses |
| [architecture-diagrammer](architecture-diagrammer/SKILL.md) | architecture, diagram, visualization | Generates architecture diagrams from code |
| [changelog-generator](changelog-generator/SKILL.md) | changelog, release, docs | Generates changelogs from git history |
| [classify-text](classify-text/SKILL.md) | classify, text, nlp | Classify text into predefined categories with confidence scores |
| [ci-pipeline-generator](ci-pipeline-generator/SKILL.md) | ci, pipeline, devops | Generates CI/CD pipeline configurations |
| [code-execution-sandbox](code-execution-sandbox/SKILL.md) | execution, sandbox, safety | Executes untrusted code in a sandboxed environment |
| [code-reviewer](code-reviewer/SKILL.md) | code, review, quality | Reviews code for bugs, style, and best practices |
| [commit-message-generator](commit-message-generator/SKILL.md) | git, commit, message | Generates conventional commit messages from diffs |
| [context-summarizer](context-summarizer/SKILL.md) | context, summary, compression | Summarizes long conversations and context windows |
| [data-pipeline-optimizer](data-pipeline-optimizer/SKILL.md) | data, pipeline, etl | Optimizes data processing pipelines |
| [data-transformer](data-transformer/SKILL.md) | data, transform, conversion | Transforms data between formats and schemas |
| [decision-logger](decision-logger/SKILL.md) | decision, logging, audit | Logs architectural and design decisions |
| [dependency-analyzer](dependency-analyzer/SKILL.md) | dependency, analysis, graph | Analyzes project dependency graphs |
| [dockerfile-generator](dockerfile-generator/SKILL.md) | docker, container, devops | Generates optimized Dockerfiles |
| [embed-text](embed-text/SKILL.md) | embed, text, vector | Generate a semantic embedding/vector representation of text for similarity search |
| [env-config-validator](env-config-validator/SKILL.md) | env, config, validation | Validates environment configuration |
| [error-recovery-planner](error-recovery-planner/SKILL.md) | error, recovery, resilience | Plans error recovery strategies |
| [knowledge-retriever](knowledge-retriever/SKILL.md) | knowledge, retrieval, search | Retrieves knowledge from documentation and codebase |
| [log-analyzer](log-analyzer/SKILL.md) | log, analysis, debugging | Analyzes application logs for errors and patterns |
| [note-taking](note-taking/SKILL.md) | note, writing, organization | Maintain structured working notes across sessions for project context and decisions |
| [performance-analyzer](performance-analyzer/SKILL.md) | performance, profiling, optimization | Analyzes code performance and bottlenecks |
| [progress-tracker](progress-tracker/SKILL.md) | progress, tracking, status | Tracks progress across tasks and milestones |
| [project-analyzer](project-analyzer/SKILL.md) | project, analysis, structure | Analyzes project structure and conventions |
| [**prompt-optimizer**](prompt-optimizer/SKILL.md) | prompt, optimization, LLM | Analyzes and improves LLM prompts for clarity and efficiency |
| [refactoring-advisor](refactoring-advisor/SKILL.md) | refactoring, code-quality, improvement | Advises on code refactoring opportunities |
| [regex-builder](regex-builder/SKILL.md) | regex, pattern, text | Builds and tests regular expressions |
| [review-pr](review-pr/SKILL.md) | review, pr, quality | Review a pull request diff and provide comprehensive feedback |
| [security-auditor](security-auditor/SKILL.md) | security, audit, vulnerability | Audits code for security vulnerabilities |
| [**self-reviewer**](self-reviewer/SKILL.md) | review, code-quality, self-improvement | Performs structured self-review of agent work |
| [semantic-diff](semantic-diff/SKILL.md) | diff, semantic, comparison | Analyze code changes semantically — understand what changed, why, and potential impacts |
| [skill-creator](skill-creator/SKILL.md) | skill, meta, creation | Guides creation of new Go-On skills |
| [sql-query-helper](sql-query-helper/SKILL.md) | sql, database, query | Helps write and optimize SQL queries |
| [summarize-text](summarize-text/SKILL.md) | summarize, text, compression | Summarize long text into concise, structured summaries |
| [task-planner](task-planner/SKILL.md) | task, planning, execution | Plans and decomposes complex tasks |
| [test-generator](test-generator/SKILL.md) | test, generation, quality | Generates unit and integration tests |
| [translate-text](translate-text/SKILL.md) | translate, i18n, language | Translate text between languages with natural-sounding results |
| [web-scraper](web-scraper/SKILL.md) | web, scraping, data-extraction | Scrapes and extracts data from websites |
| [**workflow-optimizer**](workflow-optimizer/SKILL.md) | workflow, optimization, pipeline | Analyzes and optimizes multi-step workflows |
