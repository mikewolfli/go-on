---
name: ci-pipeline-generator
description: Generates CI/CD pipeline configurations for GitHub Actions, GitLab CI, Jenkins, and CircleCI
version: 1.0.0
author: go-on-team
tags: [ci, cd, pipeline, github-actions, gitlab-ci, jenkins, circleci, devops]
min_go_on_version: 1.0.0
---

# CI/CD Pipeline Generator Skill

Generates complete, production-ready CI/CD pipeline configurations from a high-level description of the project, language, test framework, deployment target, and quality gates. Supports GitHub Actions, GitLab CI, Jenkins Pipeline, and CircleCI.

## How It Works

1. **Profile** — Analyzes the project language, build system, test framework, and deployment targets
2. **Stage** — Generates pipeline stages (lint → build → test → security-scan → package → deploy) with appropriate parallelism and caching
3. **Quality Gates** — Inserts configurable quality gates (test coverage threshold, clippy warnings, security vulnerability limits, build time budget)
4. **Matrix** — Generates build matrix for multi-version/multi-platform testing when applicable
5. **Secrets** — Documents required CI secrets and environment variables with clear setup instructions

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `language` | string | Primary language: `rust`, `python`, `typescript`, `go`, `java`, `ruby` |
| `platform` | string | CI platform: `github-actions`, `gitlab-ci`, `jenkins`, `circleci` (default: `github-actions`) |
| `test_command` | string | Test command (e.g., `cargo test`, `npm test`, `pytest`) |
| `build_command` | string | Optional: build command (if different from test) |
| `deploy_target` | string | Optional: deployment target (`docker`, `npm`, `crates.io`, `pypi`, `aws`, `gcp`, `azure`) |
| `coverage_threshold` | integer | Optional: minimum test coverage percentage for quality gate (default: 0 = disabled) |
| `include_lint` | boolean | Optional: include linting stage (default: true) |
| `include_security_scan` | boolean | Optional: include dependency security scanning (default: false) |

## Example

```json
{
  "language": "rust",
  "platform": "github-actions",
  "test_command": "cargo test --all-features",
  "build_command": "cargo build --release",
  "deploy_target": "crates.io",
  "coverage_threshold": 80,
  "include_lint": true,
  "include_security_scan": true
}
```

**Example output (abbreviated):**

```yaml
# .github/workflows/ci.yml — Generated for Rust + crates.io
name: CI

on:
  push:
    branches: [main, develop]
  pull_request:
    branches: [main]

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: "-D warnings"

jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions-rust-lang/setup-rust-toolchain@v1
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo fmt --check

  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions-rust-lang/setup-rust-toolchain@v1
      - run: cargo test --all-features
      - name: Upload coverage
        if: github.ref == 'refs/heads/main'
        run: |
          cargo install cargo-tarpaulin
          cargo tarpaulin --out Xml
        # Requires: CODECOV_TOKEN secret

  security:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions-rust-lang/setup-rust-toolchain@v1
      - run: cargo audit
        # Requires: cargo install cargo-audit

  publish:
    if: startsWith(github.ref, 'refs/tags/v')
    needs: [lint, test, security]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions-rust-lang/setup-rust-toolchain@v1
      - run: cargo publish
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```
