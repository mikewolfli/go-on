# Release Readiness (Stage C)

This document defines production SLO and pre-release drill gates for go-on.

## SLO Baseline

1. Availability: >= 99.9%
2. API error ratio (5xx-equivalent): <= 0.5%
3. P95 latency for key RPC calls: <= 2s
4. P99 latency for key RPC calls: <= 5s
5. Critical alerts in release drill: 0 allowed

## Pre-release Drill Checklist

1. Use `config/config.multi-users-server.toml` with `production_strict=true` and `entry_auth_enabled=true` (single-server variant: `config/config.simple-server.toml`).
2. Verify ingress TLS configuration via `scripts/deploy/nginx/go-on.conf`.
3. Run release readiness gate (it validates all four profiles against the configs in `config/`):
   - Linux/macOS: `scripts/run-release-readiness-gate.sh`
   - Windows: `scripts/run-release-readiness-gate.ps1`
4. Run the manual runtime drill (not automated by the gate script, which only
   covers compile-time checks) against a running server and confirm:
   - `runtime.stability`
   - `security.baseline`
   - `observability.alerts`
   - `optimization.peak`
   - in-flight graceful shutdown drain validation

## Audit Artifacts

Keep the following artifacts per release candidate:

1. Gate output logs
2. Config snapshot (`config/config.multi-users-server.toml` + env map without secrets)
3. Commit SHA and build metadata

## Automation Gates

| Gate | Script | Manual Check |
|:-----|:-------|:------------|
| Build | `cargo build --release` | ✅ Binary exists |
| Lint | `cargo clippy -- -D warnings` | ✅ Zero warnings |
| Unit Tests | `cargo test --lib` | ✅ All passing |
| Integration Tests | `cargo test --test '*'` | ✅ All passing |
| Profile Check | `cargo check --no-default-features -F multi-users-server` | ✅ Zero errors |
| Contract Smoke | `cargo test --test pua_contract_smoke` | ✅ All passing |
| Performance Baseline | `cargo bench` (see `benches/`) | 📊 Report generated |
| Release Gate | `scripts/run-release-readiness-gate.sh` | ✅ All gates pass |
