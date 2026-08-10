# Contributing to go-on

Thank you for your interest in contributing to go-on! This document provides guidelines and workflows for contributing effectively.

## Quick Links

- [README](README.md) — Project overview and getting started
- [Development Rules](docs/DEVELOPMENT_RULES.md) — Core engineering rules
- [RULES/](RULES/) — Runtime rule system (auto-loaded by config)
- [Cookbook](cookbook/) — mdBook-format documentation (trilingual)
- [Architecture Blueprints](docs/blueprints/) — Design documents

## Code of Conduct

This project is committed to providing a welcoming and inclusive experience. Be respectful, constructive, and considerate in all interactions.

## Getting Started

### Prerequisites

- Rust 2021 edition (stable toolchain)
- For GUI development: EGUI system dependencies (see [.devcontainer/Dockerfile](.devcontainer/Dockerfile))
- For VS Code addon: Node.js 22+

### Local Development

```bash
# Build with default (local) profile
cargo build

# Run tests
cargo test --all-targets

# Run lints
cargo clippy --all-targets -- -D warnings
cargo fmt --all

# Build GUI
cargo build --manifest-path gui/Cargo.toml
```

## Commit Guidelines

We follow **Conventional Commits** for all commit messages:

```
<type>(<scope>): <description>

[optional body]
```

### Types

| Type     | Usage                                  |
|----------|----------------------------------------|
| `feat`   | A new feature                          |
| `fix`    | A bug fix                              |
| `refactor` | Code change that neither fixes nor adds |
| `docs`   | Documentation only changes             |
| `test`   | Adding or updating tests               |
| `chore`  | Build, CI, or tooling changes          |
| `perf`   | Performance improvement                |
| `i18n`   | Internationalization changes           |
| `ci`     | CI pipeline changes                    |

### Scope Examples

- `feat(cli):` — CLI changes
- `fix(gui):` — GUI changes
- `refactor(transport):` — Transport/bus changes
- `docs(readme):` — README updates
- `i18n(zh-CN):` — Chinese localization
- `ci(build):` — CI build changes

### Examples

```
feat(tools): add PDF text extraction tool
fix(cache): resolve TTL race condition on concurrent access
docs(api): update ACP protocol method list
i18n(zh-TW): add traditional Chinese translations for new error keys
```

**Tip:** Use the built-in `/commit` command in go-on CLI to generate conventional commit messages from your working tree diff.

### Version Tags

Release tags follow `v<major>.<minor>.<patch>` (e.g., `v1.4.1`) and must match the version in `Cargo.toml`.

```bash
# After merging a release PR:
git tag v1.4.1
git push origin v1.4.1
```

## Pull Request Process

1. **Scope your changes** — Keep PRs focused on a single concern.
2. **Run the full CI gate** before opening:
   ```bash
   cargo check --all-targets
   cargo clippy --all-targets -- -D warnings
   cargo test --all-targets
   ```
3. **Update documentation** — README, CHANGELOG, and config templates as needed.
4. **i18n completeness** — If you add new user-facing strings, update all three language files (`en`, `zh-CN`, `zh-TW`).
5. **Multi-profile check** — For core changes, verify compilation across all 4 profiles:
   ```bash
   make test-all-profiles
   ```
6. **Changelog entry** — Add your change to `CHANGELOG.md` and `CHANGELOG.zh-CN.md` under the appropriate section.

### PR Template Checklist

- [ ] Changes are scoped and documented
- [ ] `cargo check --all-targets` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test --all-targets` passes
- [ ] Multi-profile compilation verified (if applicable)
- [ ] i18n keys added to all three language files (if applicable)
- [ ] CHANGELOG updated (both languages)
- [ ] README/docs updated (if user-facing change)

## Testing Standards

- **Unit tests** — Place `#[cfg(test)] mod tests` at the bottom of each module.
- **Integration tests** — Process-level blackbox tests go in `tests/`.
- **E2E tests** — For fault tolerance and transport changes, include lifecycle E2E tests.
- **No flaky tests** — All tests must be deterministic. Flaky tests should be investigated and fixed immediately.
- **All 4 profiles** — Core changes must pass under `local`, `simple-server`, `multi-users-server`, and `full`.

## Architecture Overview

go-on uses a **sub-bus capability architecture** with 7 feature-gated sub-buses
(tool, orchestration, observability, optimization, memory, protocol, and
distributed-memory), governed by the HarnessBus (governance) and connected
through a cognitive loop with a unified **DispatchOutput** handler pattern:

```
HarnessBus (Governance) → CapabilityBus (Intelligence) → ToolBus / ObservBus / etc.
```

See [Architecture Blueprints](docs/blueprints/) and the [README](README.md#architecture) for details.

## Questions?

If you have questions about contributing, open a GitHub Discussion or check the [cookbook](cookbook/) for detailed guides.
