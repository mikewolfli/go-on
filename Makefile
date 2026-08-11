.PHONY: all build check clippy test test-all fmt clean lint doc audit dev-container

# Default target shows help
all: help

help: ## Show this help
	@echo "go-on development make targets:"
	@echo ""
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "Default build: cargo build (local profile)"
	@echo "Common profiles:"
	@echo "  make build             (default: local)"
	@echo "  make build-simple      (simple-server)"
	@echo "  make build-multi       (multi-users-server)"
	@echo "  make build-full        (full)"

build: ## Build with default (local) profile
	cargo build

build-simple: ## Build with simple-server profile
	cargo build --no-default-features --features simple-server

build-multi: ## Build with multi-users-server profile
	cargo build --no-default-features --features multi-users-server

build-full: ## Build with full profile (all features)
	cargo build --no-default-features --features full

build-release: ## Build release with default profile
	cargo build --release

check: ## Check all targets compile
	cargo check --all-targets --locked

clippy: ## Run clippy on all targets
	cargo clippy --all-targets --locked -- -D warnings

clippy-fix: ## Auto-fix clippy suggestions
	cargo clippy --all-targets --locked --fix --allow-dirty

test: ## Run all tests
	cargo test --all-targets --locked

test-lib: ## Run library tests only (fast)
	cargo test --lib --locked

test-all-profiles: ## Run tests for all 4 profiles
	@echo "=== local profile ===" && cargo test --all-targets --locked 2>&1 | tail -5
	@echo "=== simple-server profile ===" && cargo test --no-default-features --features simple-server --all-targets --locked 2>&1 | tail -5
	@echo "=== multi-users-server profile ===" && cargo test --no-default-features --features multi-users-server --all-targets --locked 2>&1 | tail -5
	@echo "=== full profile ===" && cargo test --no-default-features --features full --all-targets --locked 2>&1 | tail -5

fmt: ## Format all Rust code
	cargo fmt --all

clean: ## Clean build artifacts
	cargo clean

lint: check clippy ## Run all lints (check + clippy)

doc: ## Build documentation for the go-on crate
	cargo doc --no-deps --package go-on
	@echo "Documentation generated at target/doc/go_on/index.html"

audit: ## Check dependencies for vulnerabilities (requires cargo-audit)
	@if command -v cargo-audit >/dev/null 2>&1; then \
		cargo audit; \
	else \
		echo "cargo-audit not installed. Run: cargo install cargo-audit"; \
	fi

deny: ## Check dependency licenses (requires cargo-deny)
	@if command -v cargo-deny >/dev/null 2>&1; then \
		cargo deny check; \
	else \
		echo "cargo-deny not installed. Run: cargo install cargo-deny"; \
	fi

bench: ## Run benchmarks (criterion runs on stable Rust)
	cargo bench

bench-acp: ## Run ACP protocol benchmarks only
	cargo bench --bench acp_bench

ci: check clippy test ## Run CI gate (check + clippy + test)

dev-container: ## Set up dev container
	@if [ -f .devcontainer/devcontainer.json ]; then \
		echo "Dev container config exists. Use VS Code's 'Reopen in Container' feature."; \
	else \
		echo "Dev container not configured. Run: make dev-container-setup"; \
	fi

gui-check: ## Check the GUI crate
	cargo check --all-targets --locked --manifest-path gui/Cargo.toml

gui-test: ## Test the GUI crate
	cargo test --all-targets --locked --manifest-path gui/Cargo.toml 2>/dev/null || echo "GUI tests not available on this platform"

vscode-install: ## Install VS Code extension dependencies
	cd vscode-addon && npm ci

count: ## Count lines of Rust code
	@find src -name '*.rs' -type f | xargs wc -l | tail -1

tag-version: ## Verify git tag matches Cargo.toml version
	@CARGO_VER=$$(grep '^version ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/'); \
	GIT_TAG=$$(git describe --tags --abbrev=0 2>/dev/null || echo "none"); \
	if [ "$$GIT_TAG" = "none" ]; then \
		echo "⚠️  No git tag found. Create one: git tag v$$CARGO_VER"; \
	elif [ "v$$CARGO_VER" != "$$GIT_TAG" ]; then \
		echo "⚠️  Mismatch: Cargo.toml version=$$CARGO_VER, latest tag=$$GIT_TAG"; \
		echo "   Run: git tag v$$CARGO_VER"; \
	else \
		echo "✅ Tag v$$CARGO_VER matches Cargo.toml"; \
	fi

.PHONY: help build build-simple build-multi build-full build-release
.PHONY: check clippy clippy-fix test test-lib test-all-profiles
.PHONY: fmt clean lint doc audit deny bench ci dev-container
.PHONY: gui-check gui-test vscode-install count tag-version
