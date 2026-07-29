---
name: project-analyzer
description: Analyzes full project structure, dependencies (with security, licensing, and upgrade auditing), architecture patterns, and generates comprehensive reports
version: 1.0.0
author: go-on-team
tags: [analysis, architecture, dependencies, security, licensing, audit, project, documentation, refactoring]
min_go_on_version: 1.0.0
---

# Project Analyzer Skill

Performs deep analysis of a project's structure, dependency graph (including security auditing, license checking, and upgrade paths), architecture patterns, code quality metrics, and generates comprehensive reports. Helps developers understand codebase health, identify technical debt, and plan refactoring efforts.

## How It Works

1. **Structure scan** — Walks the directory tree and maps the project's file organization
2. **Dependency analysis** — Parses dependency files (Cargo.toml, package.json, requirements.txt, go.mod, Gemfile) and builds a dependency graph
3. **Dependency auditing** — Cross-references dependencies against known vulnerabilities, deprecated packages, and compatibility issues; validates license types and flags high-risk licenses (AGPL, no-license, custom-restrictive); suggests target versions based on semver compatibility and migration notes
4. **Architecture detection** — Identifies architectural patterns (MVC, layered, hexagonal, microservices) from directory structure and imports
5. **Code quality metrics** — Estimates code complexity, file sizes, language distribution, and test coverage gaps
6. **Report generation** — Produces structured JSON and optional Markdown summary grouped by severity (critical, high, medium, low, info)

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Root directory of the project to analyze |
| `depth` | integer | Directory traversal depth (default: 5, max: 20) |
| `dependency_depth` | integer | Optional: dependency tree depth to analyze (default: 1, max: 3) |
| `include_patterns` | array | Optional glob patterns to include (e.g. `["**/*.rs", "**/*.py"]`) |
| `exclude_patterns` | array | Optional glob patterns to exclude (e.g. `["**/node_modules/**"]`) |
| `analyze_deps` | boolean | Whether to parse and analyze dependency files (default: true) |
| `include_license_check` | boolean | Optional: include license analysis (default: true) |
| `include_upgrade_suggestions` | boolean | Optional: suggest specific version upgrades (default: true) |
| `output_format` | string | Report format: `json` or `markdown` (default: `json`) |

## Example

```json
{
  "directory": "/home/user/projects/my-app",
  "depth": 5,
  "dependency_depth": 1,
  "exclude_patterns": ["**/node_modules/**", "**/target/**", "**/.git/**"],
  "analyze_deps": true,
  "include_license_check": true,
  "include_upgrade_suggestions": true,
  "output_format": "json"
}
```

**Example output (abbreviated):**

```json
{
  "project_name": "my-app",
  "language": "Rust",
  "total_files": 156,
  "total_lines": 28450,
  "languages": {
    "Rust": {"files": 98, "lines": 22100},
    "TypeScript": {"files": 34, "lines": 5200},
    "Markdown": {"files": 24, "lines": 1150}
  },
  "dependencies": {
    "direct": 42,
    "transitive": 156,
    "outdated": 3,
    "vulnerable": 1
  },
  "architecture": {
    "pattern": "layered",
    "layers": ["presentation", "domain", "infrastructure"],
    "circular_deps": 0
  },
  "audit_summary": {
    "critical": 1,
    "high": 0,
    "medium": 2,
    "low": 1,
    "info": 3
  }
}
```

### Detailed Dependency Audit Report

When dependency auditing is enabled, the output also includes a Markdown section with full details:

```markdown
## Critical
| Package | Version | Issue | Recommendation |
|---------|---------|-------|---------------|
| openssl | 0.10.55 | CVE-2024-XXXX: buffer overflow in TLS handshake | Upgrade to 0.10.60+ |

## Medium
| Package | Version | Issue | Recommendation |
|---------|---------|-------|---------------|
| reqwest | 0.11.14 | 6 minor versions behind (latest: 0.12.7) | Upgrade to 0.12.x (breaking: native-tls → rustls default) |
| image | 0.24 | 1 major version behind (latest: 0.25) | Upgrade to 0.25 (breaking: decoder API changes) |

## License Summary
| Package | License | Risk |
|---------|---------|------|
| tokio | MIT | ✅ Low |
| openssl | Apache-2.0 | ✅ Low |
| image | MIT/Apache-2.0 | ✅ Low |

## Suggested Actions
1. **Immediate**: Update openssl to 0.10.60 (security fix)
2. **This quarter**: Migrate reqwest to 0.12.x (documented migration guide)
3. **Backlog**: Plan image 0.24 → 0.25 migration (breaking changes in decoder API)
```
