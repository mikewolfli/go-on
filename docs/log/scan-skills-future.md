# Scan Report: Skills + Future Docs Completeness Check

## 1. SkillRegistry (`src/orchestration/skill/registry.rs`)

**Status: Functionally complete.** All major CRUD operations are implemented.

| Operation | Status | Notes |
|-----------|--------|-------|
| `register()` | ✅ | Name validation (1-64 chars, lowercase/digits/.-_), uniqueness check, schema type check |
| `get()` | ✅ | Direct HashMap lookup |
| `unregister()` | ✅ | Cleans up stats, evolution_history, and prompt_skill_data |
| `list()` | ✅ | Returns descriptors sorted by score descending, then name ascending |
| `record_outcome()` | ✅ | Tracks success/failure/latency |
| `score_of()` / `descriptor()` | ✅ | Computed via success-rate minus latency-penalty |
| `best_match()` / `best_match_with_input()` | ✅ | Composite scoring: 35% name + 25% runtime + 40% semantic similarity |
| `create_skill_from_prompt()` | ✅ | Full implementation with evolution history (capped at 50) + disk persistence |
| `remove_prompt_skill()` | ✅ | Removes from all maps + persists to disk |
| `discover_and_register_local_skills()` | ✅ | Scans `~/.agents/skills/` for SKILL.md/agent.md, mtime-based hot-reload supported |
| `spawn_skill_refresh_task()` | ✅ | Background 60s interval watcher, handles hot-reload, safe for non-async test contexts |
| `load_prompt_skills_from_disk()` | ✅ | JSON deserialization + registration at startup |
| `save_prompt_skills_to_disk()` | ✅ | JSON serialization of prompt_skill_data |
| General skill **export** | ❌ **Missing** | `save_prompt_skills_to_disk()` only persists `prompt_skill_data` (prompt-based skills). There is no method to export an arbitrary registered skill to a portable format. |
| General skill **import** (portable) | ❌ **Missing** | Only SKILL.md and prompt-based persistence formats are supported. No generic skill import from e.g. JSON serialization. |

**Minor observations:**
- Lines 158-172: `register()` uses the wrong i18n key `"error.skill_name_invalid_chars"` for the non-object-schema error message — it should have its own key (cosmetic, not blocking).
- `remove_prompt_skill()` on line 438 accesses field `self.prompt_skill_data` directly. This is inside `impl SkillRegistry` so it works, but it's the only method besides `unregister()` that bypasses `unregister()` for the core maps. Consistent, but `unregister()` is called from `discover_and_register_local_skills` and `spawn_skill_refresh_task` instead, so there's a slight inconsistency in cleanup paths.

## 2. Skill Execution (`src/orchestration/skill/execution.rs`)

**Status: PromptBasedSkill is fully functional. ComposedSkill does not exist.**

- **PromptBasedSkill** (`Skill` impl): ✅ Fully implemented
  - Prompt template substitution with `{key}` and `{{key}}` formats
  - Timeout handling via `tokio::time::timeout`
  - Retry logic with rate-limit-aware exponential backoff (1s, 2s, 4s... capped at 30s)
  - Clear error messages when no LLM agent is configured
- **ChatBasedSkillAgent** (bridges to `Agent` trait): ✅ Fully implemented
- **EchoSkill**: ✅ Simple built-in test skill
- **SkillCreatorSkill**: ✅ Creates skills via `SkillRegistry::create_skill_from_prompt()`
- **ComposedSkill**: ❌ **Does not exist.** There is no `ComposedSkill` struct, trait impl, or any reference in `execution.rs` or anywhere else in the project. This is a missing concept that would be needed for combining/sequencing multiple skills.

**Other notes:**
- The `Skill` trait on lines 34-46 handles description, input_schema, and async execute. No `Clone` or serialization requirements, which is appropriate for trait objects behind `Arc<dyn Skill>`.
- `hashed_embedding()` (line 350) uses `DefaultHasher` which is not cryptographically stable across runs. Acceptable for a heuristic similarity score, but noted.

## 3. Future Roadmap Documents (`docs/design/`)

### FUTURE5.MD — **BLOCKING: Git merge conflict markers present**

File contains unresolved `<<<<<<< Updated upstream` / `>>>>>>> Stashed changes` markers at:
- Lines 4-8 (header section)
- Lines 65-142 (entire P0–P7 checklist block — duplicated with `[ ]` vs `[x]` versions)
- Lines 158-161 (table footer)
- Lines 172-182 (risk table)

**These must be resolved before the file can be used as a reliable reference.** The stashed changes version appears to have all checkboxes checked (`[x]`), while the upstream version has them unchecked (`[ ]`).

### FUTURE4.MD — **BLOCKING: Corrupted Chinese text**

Chinese text throughout the Chinese-language sections is corrupted with `0x3f` (`?`) bytes. The final lines (151-159) contain an embedded user note asking for a fix but the fix was never applied. The English-language sections and table structure are intact. The file title should be "FUTURE4 — 自进化全能助手升级路线图（自动匹配方案 + 子AI自训练接入）" — this and all other Chinese text needs restoration.

### FUTURE.MD, FUTURE2.MD, FUTURE3.MD, FUTURE6.MD

All clean. No issues found. These are comprehensive roadmap documents with phased execution plans, risk matrices, and DoD checklists.

## 4. `docs/design/future-last.md` and `docs/design/design.md`

**No blocking issues.**

- **future-last.md**: A highly aspirational vision document ("Kubernetes for AI Agents"). Contains pseudo-code examples, architecture diagrams, and a 2-year roadmap. Not actionable for current development; conceptual only.
- **design.md**: A mix of Phase 0/1 architecture notes (capability matrix, key interfaces) and what appears to be the original project specification/generation prompt. The bottom half is a task specification, not executable code. No functional issues.

## 5. `Cargo.toml`

**No blocking issues.**

| Check | Result |
|-------|--------|
| `[patch]` section | ❌ **Not present** — no dependency overrides |
| Dependency issues | ✅ All versions are consistent between `[workspace.dependencies]` and crate `[dependencies]` |
| `deadpool-postgres` / `tokio-postgres` | ✅ Commented out with clear rationale (lines 29-33) |
| Feature flags | ✅ Well-organized with mutually-exclusive profiles, compile-time assertions in lib.rs |
| Optional deps | ✅ Cleanly gated by features, no dead references |

## 6. `.zed/` Directory

**No issues.** Contains a single file `ai_instructions.md` with AI coding constraints (forbidden patterns, mandatory behaviors) and a description of the skill system usage. Properly structured, self-consistent.

---

## Summary of Blocking Issues

| # | File | Issue | Severity |
|---|------|-------|----------|
| 1 | `docs/design/FUTURE5.MD` | Unresolved git merge conflict markers (`<<<<<<<` / `>>>>>>>`) across 3 sections | **High** |
| 2 | `docs/design/FUTURE4.MD` | Corrupted Chinese text (UTF-8 bytes replaced with `?`) | **High** |
| 3 | `src/orchestration/skill/execution.rs` | `ComposedSkill` does not exist — referenced in task description but never implemented | **Medium** |
| 4 | `src/orchestration/skill/registry.rs` | No general skill export/import mechanism (only prompt-based persistence exists) | **Low-Medium** |
