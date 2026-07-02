# Go-On Skills Marketplace — Scan Report

**Date:** 2026-07-02  
**Scope:** All 18 skill directories under `skills/`  
**Scanner:** Zed Agent (automated audit)

---

## Summary

| Metric | Value |
|---|---|
| Total skills scanned | 18 |
| Valid frontmatter | 18/18 |
| Directory name matches `name` field | 18/18 |
| Instructions complete and useful | 18/18 |
| Missing documentation / broken references | 0/18 |
| Skills with `icon.png` or `tests/` | 0/18 |
| **Overall health** | **✅ All skills are well-formed** |

---

## Per-Skill Audit

### 1. `api-docs-generator/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | `name`, `description`, `version`, `author`, `tags`, `min_go_on_version` all present |
| Name matches directory | ✅ | `name: api-docs-generator` |
| Instructions complete | ✅ | How It Works (4 steps), Input Schema (5 params), realistic JSON example with output |
| Broken references | ✅ None | All self-contained; no external file links |
| Additional files | ❌ | No `icon.png`, no `tests/` directory |
| **Verdict** | **✅ Pass** | Production-quality skill |

### 2. `changelog-generator/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | All fields present |
| Name matches directory | ✅ | `name: changelog-generator` |
| Instructions complete | ✅ | 4-step workflow, 5 input params, Keep a Changelog formatted output example |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | |

### 3. `ci-pipeline-generator/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: ci-pipeline-generator` |
| Instructions complete | ✅ | 5-step workflow, 8 input params (most of any skill), full GitHub Actions YAML output |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Excellent coverage of platforms (GitHub Actions, GitLab CI, Jenkins, CircleCI) |

### 4. `code-reviewer/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | `version: 1.3.0` (only skill not at 1.0.0 — suggests active iteration) |
| Name matches directory | ✅ | `name: code-reviewer` |
| Instructions complete | ✅ | 4-step workflow, 3 input params, structured JSON output with scoring |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Scoring mechanism (0.0–1.0) is a nice touch |

### 5. `commit-message-generator/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: commit-message-generator` |
| Instructions complete | ✅ | 4-step workflow, 5 input params, realistic git diff example, Conventional Commits output |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Scope detection from file paths is well-documented |

### 6. `data-transformer/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: data-transformer` |
| Instructions complete | ✅ | 4-step workflow, 6 input params, basic and advanced (nested flatten) examples |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Advanced example with nested flatten is a strong addition |

### 7. `dependency-analyzer/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: dependency-analyzer` |
| Instructions complete | ✅ | 5-step workflow, 5 input params, rich structured report output with CVEs, license risk, upgrade paths |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Most comprehensive output of all skills — includes severity grouping, license summary, and prioritized actions |

### 8. `dockerfile-generator/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: dockerfile-generator` |
| Instructions complete | ✅ | 5-step workflow, 6 input params, multi-stage Dockerfile + docker-compose.yml output |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Covers multi-stage builds, layer caching, security (non-root), and compose services |

### 9. `knowledge-retriever/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: knowledge-retriever` |
| Instructions complete | ✅ | 4-step workflow, 2 input params, JSON output with scored results |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Simple but effective; lightweight input schema |

### 10. `log-analyzer/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: log-analyzer` |
| Instructions complete | ✅ | 4-step workflow, 6 input params, auto-format detection, anomaly detection, timeline output |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Strong cluster detection and root cause analysis logic |

### 11. `prompt-optimizer/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: prompt-optimizer` |
| Instructions complete | ✅ | 4-step workflow, 5 input params, 5-dimension scoring rubric, before/after comparison |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Multi-model support (Claude, GPT-4o, DeepSeek, Gemini) is well-considered |

### 12. `refactoring-advisor/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: refactoring-advisor` |
| Instructions complete | ✅ | 4-step workflow, 4 input params, 20+ code smells detected, prioritized output with effort estimates |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | God Function detection with full before/after example is excellent |

### 13. `regex-builder/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: regex-builder` |
| Instructions complete | ✅ | 5-step workflow, 6 input params, multi-engine support, token-by-token breakdown, test results |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Supports 5 regex engines; translate action between engines is unique |

### 14. `skill-creator/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: skill-creator` |
| Instructions complete | ✅ | 5-step lifecycle, 6 input params, full scaffolded SKILL.md output, validation checks, PR submission steps |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Meta-skill — well-designed, validates its own output |

### 15. `sql-query-helper/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: sql-query-helper` |
| Instructions complete | ✅ | 5-step workflow, 6 input params, schema context support, index recommendations, dialect translation |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Supports 5 SQL dialects; anti-pattern detection is practical |

### 16. `task-planner/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: task-planner` |
| Instructions complete | ✅ | 5-step workflow, 3 input params, JSON plan with dependency graph, effort estimation |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Dependency-graph output is excellent for agent execution ordering |

### 17. `test-generator/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: test-generator` |
| Instructions complete | ✅ | 4-step workflow, 5 input params, edge case + property-based test support |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | Covers 4 languages and 2 test modes |

### 18. `web-scraper/`

| Check | Status | Notes |
|---|---|---|
| Frontmatter valid | ✅ | |
| Name matches directory | ✅ | `name: web-scraper` |
| Instructions complete | ✅ | 4-step workflow, 5 input params, URL + raw HTML dual input, metadata extraction |
| Broken references | ✅ None | |
| Additional files | ❌ | No extras |
| **Verdict** | **✅ Pass** | |

---

## Patterns & Observations

### Strengths
- **Consistent structure** — Every skill follows the same template: YAML frontmatter → title → intro → How It Works → Input Schema table → JSON Example → Example Output.
- **Realistic examples** — Every skill has a fully-formed JSON input example with a matching output demonstration.
- **Good depth** — Most skills have 4–6 input parameters with sensible defaults, showing they were designed for real use.
- **Platform-agnostic** — Skills avoid coupling to any particular language or framework unless it's core to their purpose.
- **Tags are populated** — Every skill has a relevant `tags` list for marketplace discoverability.

### Minor Issues / Improvement Opportunities

| Issue | Affected Skills | Recommendation |
|---|---|---|
| No `icon.png` anywhere | All 18 | The README recommends optional icons (max 128×128); adding branded icons would improve marketplace visual browsing |
| No `tests/` directories | All 18 | Smoke tests per skill would help validate edge cases; the skill-creator mentions testing but none implement it |
| `author` always `go-on-team` | All 18 | All appear authored by the core team — expected for initial release, but community contributions should have distinct author fields |
| `version` is `1.0.0` everywhere | 17/18 (code-reviewer is 1.3.0) | Versions haven't been individually managed yet; code-reviewer's 1.3.0 suggests it was iterated on |
| `min_go_on_version` is `1.0.0` everywhere | All 18 | No skill has been updated to require a newer platform version |
| No `.gitkeep` in empty skill dirs | N/A | Only root `skills/.gitkeep` exists; individual skill dirs contain only `SKILL.md` |

---

## Suggested New Skills to Improve AI Agent Execution

The current 18 skills cover documentation, code analysis, generation, and data transformation. The following gaps exist for AI agent workflows:

### High Priority

| Proposed Skill | Rationale |
|---|---|
| **progress-tracker** | Agents executing multi-step plans need a way to record completed/remaining/blocked steps and persist state between invocations. Complements `task-planner`. Would output a structured JSON progress log. |
| **decision-logger** | Records architectural decisions, trade-off rationale, and context for future agent invocations (ADR-style). Prevents agents from repeating the same analysis. |
| **error-recovery-planner** | When a task fails, this skill analyzes the error, identifies root cause, and proposes recovery steps. Critical for autonomous agent execution. |
| **context-summarizer** | Summarizes conversation history, file changes, and decisions made — useful for managing token windows and passing context between agent sessions. |
| **self-reviewer** | An agent reviews its own proposed changes before presenting them to the user. Checks against project rules, style guides, and consistency. Complements `code-reviewer` but with an agent-in-the-loop focus. |

### Medium Priority

| Proposed Skill | Rationale |
|---|---|
| **pr-description-generator** | Generates structured PR descriptions from git diff/commit history, linking to related issues. Complements `commit-message-generator` at a higher granularity. |
| **schema-generator** | Generates JSON Schema, OpenAPI specs, or database schemas from code structures. Broader than `api-docs-generator`. |
| **env-validator** | Validates environment variable configurations against .env.example / docs, flags missing or deprecated vars. |
| **architecture-diagrammer** | Generates mermaid diagrams from code structure (module relationships, data flow, dependency graphs). Useful for agent documentation output. |
| **consistency-checker** | Checks naming conventions, import styles, and structural consistency across a codebase. Agents modifying code need a way to verify they haven't introduced inconsistencies. |

### Low Priority (Nice-to-Have)

| Proposed Skill | Rationale |
|---|---|
| **config-generator** | Generates configuration files (.editorconfig, .gitignore, .dockerignore, etc.) for new projects. |
| **boilerplate-generator** | Scaffolds module/directory structures with standard boilerplate. |
| **changelog-summarizer** | Summarizes a changelog for a non-technical audience (release notes). |
| **code-migrator** | Assists with framework/language migration by detecting patterns and suggesting equivalents. |
| **translation-helper** | Translates comments/docs while preserving code structure (the `skill-creator` example references this). |

---

## Conclusion

**All 18 skills pass validation.** The Go-On skills marketplace is well-structured, consistent, and contains production-quality documentation. No broken references, missing frontmatter, or incomplete instructions were found.

The most impactful next steps would be:
1. **Add 3–5 new skills** focused on agent execution state management (progress tracking, decision logging, error recovery).
2. **Add optional icons** to existing skills for marketplace visual polish.
3. **Encourage community contributions** so `author` fields diversify beyond `go-on-team`.
