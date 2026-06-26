# Deep Scan Report: core, governance, security modules

**Date:** 2026-06-26  
**Scope:** `src/core/`, `src/governance/`, `src/security/`  
**Scanner:** Manual code review of all `.rs` source files

---

## Summary

| Module       | Files | Dead-Code Annotations | Feature-Gated Dead Code | Block_on Patterns | Empty/Stub Tests | Unused Imports | Other Issues |
|-------------|-------|----------------------|------------------------|-------------------|------------------|----------------|-------------|
| core        | 18    | 0                    | 0                      | 0 (test only)     | 0                | 0             | 2           |
| governance  | 20    | 0                    | 3                      | 0                 | 0                | 0             | 2           |
| security    | 10    | 6                    | 3                      | 0                 | 0                | 0             | 1           |
| **Total**   | 48    | 6                    | 6                      | 0                 | 0                | 0             | 5           |

---

## 1. `#[allow(dead_code)]` Annotations

### `src/security/mod.rs`

| Line | Item | Annotation | Notes |
|------|------|-----------|-------|
| L30  | `get_content_safety_checker()` | `#[allow(dead_code)]` | Convenience accessor for `CONTENT_SAFETY_CHECKER` global singleton. Exists for one-shot safety checks but has **no callers** in the scanned modules. If no external caller exists, this should be removed or a tracking issue filed. **Severity: Low** |
| L70  | `get_prompt_injection_detector()` | `#[allow(dead_code)]` | Same pattern as above for `PROMPT_INJECTION_DETECTOR`. **No callers** found in scanned modules. **Severity: Low** |

### `src/security/rate_limiter.rs`

| Line | Item | Annotation | Notes |
|------|------|-----------|-------|
| L35-37 | `MaxConcurrentGuard` struct | `#[allow(dead_code, reason = "New API surface — wired from ACP HTTP request handler in subsequent PR")]` | Struct is defined but never constructed in production code. The `reason` says it's reserved for future wiring. **Severity: Medium** — code that exists only for "future PRs" should be stubbed or behind a feature flag. |
| L48-50 | `GlobalRateLimiter.semaphore` field | `#[allow(dead_code, ...)]` | Same reason. The `Semaphore` field is only used by `try_acquire_global()` which is also dead-coded. **Severity: Medium** |
| L77-79 | `try_acquire_global()` | `#[allow(dead_code, ...)]` | Method not called from any code in the scanned modules. **Severity: Medium** |
| L90-92 | `release_global()` | `#[allow(dead_code, ...)]` | Not called anywhere. **Severity: Medium** |

### `src/security/mtls.rs`

| Line | Item | Annotation | Notes |
|------|------|-----------|-------|
| L253 | `with_client_cert()` | `#[allow(dead_code)]` | Comment says "Called from `run_acp_http_server` under `#[cfg(feature = "multi-users-server")]`" but no such cfg path is present in the scanned files. If the feature-gated caller exists elsewhere, the annotation is justified. **Severity: Low** |
| L262 | `with_allowed_cns()` | `#[allow(dead_code)]` | Same pattern as above. **Severity: Low** |

### `src/security/secret_rotation.rs`

| Line | Item | Annotation | Notes |
|------|------|-----------|-------|
| L248-250 | `VaultRotator` struct | `#[allow(dead_code, reason = "F-GAP reserved — wired via server startup when vault is configured")]` | The struct exists and is constructed in `src/security/mod.rs::start_secret_rotation_if_configured()`. The dead_code annotation may be needed when `vault` feature is disabled. **Severity: Info** — justified. |
| L268-270 | `VaultRotator::new()` | `#[allow(dead_code, ...)]` | Same as above. **Severity: Info** |
| L293-295 | `VaultRotator::headers()` | `#[allow(dead_code, ...)]` | Marked dead code because it's behind `#[cfg(feature = "vault")]` and only called within vault-gated code that may also be dead-coded. **Severity: Low** |

---

## 2. Feature-Gated Code (never/rarely used features)

### `src/governance/approval_engine.rs`

| Line | Feature Gate | Code | Issue |
|------|-------------|------|-------|
| L319-330 | `#[cfg(feature = "backend-sqlite")]` | `with_db_path()` — calls `init_sqlite()`, `load_pending_from_sqlite()` | The methods `init_sqlite`, `load_pending_from_sqlite`, `upsert_sqlite`, `update_status_sqlite`, `delete_from_sqlite` (L337-428) are **entirely gated** behind `backend-sqlite`. If this feature is never enabled, ~90 lines of dead SQLite code exist. **Severity: Medium** |
| L497-500 | `#[cfg(feature = "backend-sqlite")]` | `submit_for_approval()` — persists to SQLite | Same gate. **Severity: Low** |
| L557-560 | `#[cfg(feature = "backend-sqlite")]` | `approve()` — SQLite update | Same gate. **Severity: Low** |
| L723-730 | `#[cfg(feature = "backend-sqlite")]` | `process_timeouts()` — SQLite update | Same gate. **Severity: Low** |

### `src/security/mod.rs`

| Line | Feature Gate | Code | Issue |
|------|-------------|------|-------|
| L116 | `#[cfg(feature = "vault")]` | `let vault_token = std::env::var("VAULT_TOKEN").ok()?;` | The `vault` feature controls whether a Vault token is loaded. Without it, `VaultRotator::new` still constructs but with no token, effectively causing all operations to fail at runtime. **Severity: Medium** — the feature acts as a compile-time choice but the non-feature path is a runtime error. |
| L125-126 | `#[cfg(feature = "vault")]` | Passing `vault_token` to `VaultRotator::new` | Same gate. |

### `src/security/secret_rotation.rs`

| Line | Feature Gate | Code | Issue |
|------|-------------|------|-------|
| L255-259 | `#[cfg(feature = "vault")]` | `token: String` and `client: &'static reqwest::Client` on `VaultRotator` | Without the feature, `VaultRotator` has no `token` or `client` fields, and all `KeyRotator` trait methods for `VaultRotator` return `BackendError("Vault not configured")`. The `MemoryRotator` implementation is used only in tests. **Severity: Medium** — vault is essentially a compile-time feature toggle with runtime fallback. |

---

## 3. `block_in_place` + `block_on` / `Handle::current().block_on()` Patterns

**No occurrences found in the three scanned modules.**  
The search found patterns in **other modules** (tests only, using `tokio::runtime::Runtime::new().block_on(...)` in test helpers), but these are not within the scan scope.

### Key observation: server_builder.rs (outside scan scope)
`go-on/src/acp/impl/runtime/server_builder.rs` at approximately line 666-670 contains an inline comment documenting that `block_in_place + block_on` was **previously** used but has already been refactored to async `.await`.

---

## 4. Structs/Enums with Potential Dead Code

### `src/core/config_validation.rs`

| Line | Item | Type | Issue |
|------|------|------|-------|
| L15-20 | `report_language()` | Function | Private function used only by `generate_report()` (L788). Is `generate_report()` called anywhere? The function is `pub` but manual grep did not find callers outside this file. **Severity: Low** — possible dead code if report generation is never invoked. |
| L29-37 | `tr()` | Function | Private i18n helper used within this file. **No issue** — used by localize functions. |
| L39-52 | `trf()` | Function | Same as above. **No issue**. |
| L54-227 | `localize_validation_message()` | Function | Long chain of 12+ sequential `if` statements for message localization. Maintainability concern but not dead code. **Severity: Info** |
| L284-295 | `ValidationResult` | Struct | Used by `ConfigValidator` and `validate_config_file()`. The `has_errors()`, `critical_errors()`, `regular_errors()` methods are all used. **No issue**. |
| L329-336 | `ErrorSeverity` | Enum | Used. `Warning` variant is defined but when is a `Warning` severity error actually pushed? The `validate()` method only pushes errors with `Critical` severity. The `Warning` variant appears to be dead. **Severity: Low** |
| L362-371 | `Recommendation` | Struct | Used. **No issue**. |
| L375-386 | `RecommendationCategory` | Enum | All variants used in `generate_report()`. **No issue**. |
| L390-397 | `ImpactLevel` | Enum | All variants used. **No issue**. |
| L401-408 | `PriorityLevel` | Enum | All variants used. **No issue**. |
| L412-423 | `DependencyAnalysis` | Struct | `internal_dependencies` and `config_dependencies` fields are declared but never populated by any validation function. They are only read in `generate_report`. **Severity: Medium** — declared but never written. |

### `src/governance/runtime_controls.rs`

| Line | Item | Type | Issue |
|------|------|------|-------|
| L12 | `infer_task_type_from_phase()` | Function | Private function defined but never called within this file. **Severity: Medium** — dead code. |
| L464-468 | `TIMEOUT_START_CYCLE`, `TIMEOUT_WARNED` | Statics | Used by `run_timeout_check()` and `spawn_timeout_loop()`. **No issue**. |

### `src/governance/harness_bus/mod.rs`

| Line | Item | Type | Issue |
|------|------|------|-------|
| L175-310 | `evaluate()` | Method | Long method (~135 lines). The `_start` variable on L192 is used for timing but the result is passed through many branches. The `timeout_policy` and `timeout_duration` variables on L296-297 are computed but only used for debug tracing; they have no behavioral effect. **Severity: Info** — diagnostic-only computation. |

### `src/governance/pua.rs`

| Line | Item | Type | Issue |
|------|------|------|-------|
| L52-56 | `PuaViolationKind` | Enum | All variants used. **No issue**. |
| L89-95 | `TaskType` | Enum | All variants used. **No issue**. |
| L98-103 | `QualityCategory` | Enum | All variants used. **No issue**. |
| L106-110 | `VerificationMethod` | Enum | All variants used. **No issue**. |

### `src/governance/hardening.rs`

| Line | Item | Type | Issue |
|------|------|------|-------|
| L251 | `impl std::error::Error for BudgetExceededError` | Empty impl | Block with no body: `impl std::error::Error for BudgetExceededError {}` — this is normal for error types that don't need `source()`. **No issue**. |
| L425 | `Idempotency` | Struct | Struct with no fields. The `key()` method returns a fixed value. Appears to be a placeholder. **Severity: Low** — stub implementation. |

---

## 5. Empty Function Bodies / Stub / No-Op Implementations

### `src/governance/approval_engine.rs`

| Line | Item | Issue |
|------|------|-------|
| L806-819 | `fn feedback_to_pua(&self, ...)` | The method body only calls `self.pua_engine.lock()` and does nothing with the result (appears to be a no-op or placeholder). The match arms on `request.status` do nothing. **Severity: High** — PUA feedback silently dropped. |
| L822-866 | `fn feedback_to_learner(&self, ...)` | Similarly, the learner feedback records decisions but the actual learning call (`learner.record_decision()`) is conditionally called only for finalized statuses. The code path is complex and may silently skip recording. **Severity: Medium** |

### Let's verify by reading those sections:

<details>
<summary>Click to expand verification</summary>

```rust
fn feedback_to_pua(&self, request: &ApprovalRequest) {
    let mut pua = self.pua_engine.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("[approval_engine] lock poisoned, recovering");
        poisoned.into_inner()
    });
    let now = current_timestamp_ms();
    match &request.status {
        ApprovalStatus::Approved { .. } => {}
        ApprovalStatus::Rejected { .. } => {}
        ApprovalStatus::EscalatedToManager { .. } => {}
        ApprovalStatus::AutoDenied { .. } => {}
        _ => {}
    }
    tracing::debug!(
        id = %request.id,
        action = %request.action,
        "PUA feedback recorded"
    );
}
```

This is **indeed a no-op** — the match arms are all empty `{}` blocks. The method acquires the lock, matches the status, and does nothing in every branch. **Severity: High**
</details>

### `src/governance/approval_engine.rs` — `feedback_to_learner()`

The method at L822-866 has complex conditional logic but does record decisions. However, the `PuaLearningRecord` type in `pua.rs` and the `append_learning_record` function are separate from this path. **Severity: Medium** for the complex conditionals.

---

## 6. Unnecessary Locks / Synchronization

| File | Line | Issue |
|------|------|-------|
| `src/core/provider.rs` | L31-32, L44-46 | `DefaultOrchestrationProvider` uses `Mutex<HashMap<...>>` but only exposes `skill_count()` which reads the map. A `RwLock` or `AtomicUsize` counter would be more appropriate. **Severity: Low** |
| `src/governance/approval_engine.rs` | L752-768 | Multiple queue-access methods (`get_request`, `pending_requests`, `requests_for_user`, `get_escalation_chain`) each iterate the entire `queue` Vec. These are called from a single-threaded context (the `ApprovalEngine` is wrapped in `Arc<RwLock<>>`) so locking overhead is acceptable. **No issue**. |
| `src/governance/harness_bus/audit.rs` | L17 | `HarnessAuditTrail` has `pub entries: Vec<AuditEntry>` without any synchronization wrapper — it relies on the caller to serialize access. This is fine since `HarnessBus` controls access, but it's a potential footgun. **Severity: Info** |
| `src/security/rate_limiter.rs` | L46 | Uses `Mutex<HashMap<String, TokenBucket>>` for per-tenant buckets. This is fine for the current usage pattern. **No issue**. |

---

## 7. Tests — Ignored, Empty, or `assert!(true)`

**No findings** in the three scanned modules. All test modules have meaningful assertions. The `#[cfg(test)]` modules checked:

- `src/core/config/autotune.rs` (L286-562): 6 meaningful tests
- `src/core/config/hot_reload.rs` (L357-367): 1 test for serialization
- `src/core/config/schema_version.rs` (L271-387): 7 tests
- `src/core/setup/mod.rs` (L89-201): 3 tests
- `src/core/error.rs` (L218-303): 8 tests
- `src/governance/approval_engine.rs` (L893-999): 4 async tests
- `src/governance/pua.rs` (L836-1225): Many detailed tests
- `src/security/vulnerability_scan.rs` (L1089-1309): Many tests

All tests contain concrete assertions — no `assert!(true)` or `#[ignore]` found.

---

## 8. Unused Imports

| File | Line | Import | Status |
|------|------|--------|--------|
| `src/security/secret_rotation.rs` | L9 | `use std::time::{SystemTime, UNIX_EPOCH};` | `current_timestamp_ms()` at L917 uses these. **No issue**. |
| `src/security/request_signing.rs` | L185 | `use hmac::{KeyInit, Mac};` | Only used in test module. **No issue** — gated behind `#[cfg(test)]` correctly. |
| `src/security/rate_limiter.rs` | L8 | `use std::sync::Mutex` | Used. **No issue**. |
| `src/governance/hardening.rs` | (outline) | Various | All imports appear used. **No issue**. |

All imports in the scanned modules appear to be genuinely used in their respective files.

---

## 9. Additional Concerns

### 9.1 Potential No-Op: `feedback_to_pua()` in approval_engine.rs

**File:** `src/governance/approval_engine.rs`  
**Lines:** L806-L819  
**Severity: HIGH**

The method acquires a lock on `self.pua_engine`, matches on `request.status` with **all empty match arms**, and then logs a debug message. No actual feedback is sent to the PUA engine. This means:

- Approval/rejection/escalation events are never fed back into the PUA rule engine
- The `PuaRuleEngine` cannot learn from approval decisions
- The `evaluate_approval_feedback()` method on `PuaRuleEngine` is never called from this path

### 9.2 Unused Struct Fields in DependencyAnalysis

**File:** `src/core/config_validation.rs`  
**Lines:** L420-L422  
**Severity: MEDIUM**

`DependencyAnalysis.internal_dependencies` (HashSet) and `DependencyAnalysis.config_dependencies` (HashMap) are declared but never populated by any validation path. They are only read in `generate_report()`. This means the dependency analysis report will always show empty sections for these two categories.

### 9.3 `ErrorSeverity::Warning` Variant Never Used

**File:** `src/core/config_validation.rs`  
**Lines:** L335  
**Severity: LOW**

The `ErrorSeverity::Warning` variant exists but the validation logic only pushes errors with `Critical` severity. Warnings go into the separate `warnings: Vec<ValidationWarning>` field, not into errors. The `Warning` variant is dead code in the enum.

### 9.4 `infer_task_type_from_phase()` Dead Function

**File:** `src/governance/runtime_controls.rs`  
**Lines:** L14-L22 (from outline)  
**Severity: MEDIUM**

This function is defined but never called within the module. It appears to be a utility that was written but never integrated into the runtime control flow.

### 9.5 `MtlsAcceptor` Uses `tokio::sync::RwLock` for Cached Config

**File:** `src/security/mtls.rs`  
**Lines:** L64, L203-L213  
**Severity: INFO**

The `accept()` method acquires a read lock, and if no cached config exists, drops the read lock and acquires a write lock. This is a valid pattern but the `RwLock` here is used in a hot path — every TLS connection triggers this check. Consider eagerly building the config in `new()` to avoid the lock contention on every accept.

---

## 10. Clean Modules (No Issues Found)

The following files were reviewed and found to have **no significant issues**:

- `src/core/mod.rs` — module declarations only
- `src/core/bootstrap.rs` — clean async bootstrap
- `src/core/onboarding.rs` — clean
- `src/core/providers.rs` — static provider specs, clean
- `src/core/config/mod.rs` — re-exports only
- `src/core/config/types.rs` — type definitions, clean
- `src/core/config/defaults.rs` — default implementations, clean
- `src/core/config/load/parser.rs` — config loading, clean
- `src/core/config/load/validator.rs` — config validation, clean
- `src/core/config/load/env_override.rs` — env override logic, clean
- `src/core/config/load/migrator.rs` — schema migration, clean
- `src/core/setup/config_gen.rs` — config generation, clean
- `src/core/setup/prompts.rs` — interactive prompts, clean
- `src/core/setup/secrets.rs` — secret management, clean
- `src/governance/mod.rs` — module declarations only
- `src/governance/rationalization.rs` — clean implementation
- `src/governance/review_controls.rs` — clean
- `src/governance/status.rs` — clean with good test coverage
- `src/governance/drift/mod.rs` — re-exports only
- `src/governance/drift/drift_protection.rs` — clean
- `src/governance/harness_bus/evaluator.rs` — complex but correctly implemented
- `src/governance/harness_bus/types.rs` — type definitions, clean
- `src/governance/harness_bus/audit.rs` — clean
- `src/security/content_safety.rs` — clean
- `src/security/prompt_injection.rs` — clean
- `src/security/security_advisor.rs` — clean
- `src/security/vulnerability_scan.rs` — clean
- `src/security/audit_integrity.rs` — clean

---

## Priority Recommendations

| Priority | Issue | File | Line |
|----------|-------|------|------|
| **HIGH** | `feedback_to_pua()` is a no-op — empty match arms | `src/governance/approval_engine.rs` | L806-L819 |
| **MEDIUM** | `DependencyAnalysis.internal_dependencies` and `config_dependencies` never populated | `src/core/config_validation.rs` | L420-L422 |
| **MEDIUM** | `infer_task_type_from_phase()` defined but never called | `src/governance/runtime_controls.rs` | L14-L22 |
| **MEDIUM** | `MaxConcurrentGuard` and associated dead-code-annotated API surface (4 items) | `src/security/rate_limiter.rs` | L35-L96 |
| **MEDIUM** | `backend-sqlite` feature-gated SQLite code (~90 lines) — verify feature is used | `src/governance/approval_engine.rs` | L319-428, L497, L557, L723 |
| **MEDIUM** | `vault` feature gate — without it, `VaultRotator` is a runtime error maker | `src/security/mod.rs` | L116-L127 |
| **LOW** | `ErrorSeverity::Warning` variant unused | `src/core/config_validation.rs` | L335 |
| **LOW** | `allow(dead_code)` on `get_content_safety_checker()` — no callers found | `src/security/mod.rs` | L30 |
| **LOW** | `allow(dead_code)` on `get_prompt_injection_detector()` — no callers found | `src/security/mod.rs` | L70 |
| **LOW** | `Idempotency` struct is a stub (no fields, fixed return) | `src/governance/hardening.rs` | L425 |
