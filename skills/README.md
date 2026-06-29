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
5. After review and merge, the index is auto-updated
