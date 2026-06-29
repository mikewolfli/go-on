---
name: dependency-analyzer
description: Analyzes project dependencies for security vulnerabilities, version drift, licensing, and upgrade paths
version: 1.0.0
author: go-on-team
tags: [dependencies, security, licensing, audit, cargo, npm, pip, go]
min_go_on_version: 1.0.0
---

# Dependency Analyzer Skill

Scans project dependency manifests (Cargo.toml, package.json, requirements.txt, go.mod) and produces a structured report covering security advisories, outdated packages, license compatibility, and suggested upgrade paths. Annotates findings by severity and provides actionable remediation advice.

## How It Works

1. **Parse** — Reads and normalizes dependency declarations from the supported manifest formats
2. **Audit** — Cross-references against a built-in knowledge base of common vulnerabilities, deprecated packages, and known compatibility issues
3. **License Check** — Identifies license types and flags high-risk licenses (AGPL, no-license, custom-restrictive) for review
4. **Upgrade Path** — Suggests target versions based on semver compatibility, major version migration notes, and peer dependency requirements
5. **Report** — Produces a structured report grouped by severity (critical, high, medium, low, info)

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `manifest` | string | Content of the dependency manifest file |
| `manifest_type` | string | Manifest format: `cargo`, `npm`, `pip`, `go-mod`, `gem`, `maven` |
| `include_license_check` | boolean | Optional: include license analysis (default: true) |
| `include_upgrade_suggestions` | boolean | Optional: suggest specific version upgrades (default: true) |
| `depth` | integer | Optional: dependency tree depth to analyze (default: 1, max: 3) |

## Example

```json
{
  "manifest": "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies]\ntokio = { version = \"1.28\", features = [\"full\"] }\nserde = \"1.0\"\nserde_json = \"1.0\"\nreqwest = { version = \"0.11.14\", features = [\"json\"] }\nopenssl = \"0.10.55\"\nlegacy-lib = \"0.5\"\nimage = \"0.24\"\n",
  "manifest_type": "cargo",
  "include_license_check": true,
  "include_upgrade_suggestions": true,
  "depth": 1
}
```

**Example output (abbreviated):**

```markdown
# Dependency Analysis Report

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
