# log1

- Date: 2026-05-01 (multi-round deep scan and fix)
- Rule baseline: followed BLUE38 (minimal patch, three-end consistency, i18n user-facing strings, zero warnings).

## Round 1 scan -> fix records (2026-05-01 - initial)

### 1. Backend compile error (scope conflict)
- Problem: `src/acp/impl/chat.rs` referenced `capability_optimization_hint` outside its local feature-gated scope, causing compile failure.
- Fix: hoisted `capability_optimization_hint` to outer scope and made mutability conditional by feature gate.
- Result: `cargo check` passed; `cargo clippy -D warnings` passed on all 3 profiles.

### 2. GUI i18n hardcoded placeholder
- Problem: `GUI/src/views/ConfigView.vue` had hardcoded working-dir placeholder text.
- Fix: switched to locale key `config.workingDirPlaceholder`.
- Added locale keys in:
  - `GUI/src/locales/en-US.json`
  - `GUI/src/locales/zh-CN.json`
  - `GUI/src/locales/zh-TW.json`
- Result: `GUI npm run build` passed.

### 3. VSCode addon i18n hardcoded strings in advanced edit flow
- Problem: `vscode-addon/src/advancedEdit.ts` contained multiple user-facing hardcoded strings (quick pick labels/placeholders/errors).
- Fix: replaced with i18n keys and message lookups; added key mappings in `vscode-addon/src/i18n.ts`; added locale entries in:
  - `vscode-addon/src/locales/en-US.json`
  - `vscode-addon/src/locales/zh-CN.json`
  - `vscode-addon/src/locales/zh-TW.json`
- Result: `npm run compile` passed; `node scripts/contract-smoke.js` passed.

## Round 2 scan -> fix records (2026-05-01)

### 1. Backend pathbuf useless conversion (tests/e2e_integration.rs:109)
- Problem: `PathBuf::from(binary_path())` where `binary_path()` already returns `PathBuf`.
- Fix: replaced with direct `binary_path()` assignment.
- Verification: `cargo clippy -D warnings` all 3 profiles ✅

### 2. GUI global error handler i18n
- Problem: `GUI/src/main.ts:13-18` hardcoded English error message as fallback in global error handler.
- Fix: added `i18n.global.t("error.unexpected")` with English fallback.
- Added locale entry `error.unexpected`:
  - `GUI/src/locales/en-US.json`: "An unexpected error occurred. Please check the console for details."
  - `GUI/src/locales/zh-CN.json`: "发生意外错误。请查看控制台获取详细信息。"
  - `GUI/src/locales/zh-TW.json`: "發生意外錯誤。請查看控制台獲取詳細資訊。"
- Verification: `npm run build` ✅

### 3. VSCode addon processFlowView.ts i18n (6 hardcoded strings)
- Problem: `vscode-addon/src/processFlowView.ts` contained 6 user-facing hardcoded strings:
  - "Invalid import data"
  - "Processes imported successfully"
  - "Process not found" (2x)
  - "Invalid process: ID is required"
  - "Invalid stages format: must be array"
  - 2 parameterized process name messages
- Fix: created 7 new MessageKeys in `vscode-addon/src/i18n.ts`:
  - `processFlowInvalidImportData`
  - `processFlowImported`
  - `processFlowProcessNotFound`
  - `processFlowInvalidProcessId`
  - `processFlowInvalidStagesFormat`
  - `processFlowCreatedSuccess` (with name param)
  - `processFlowCompletedSuccess` (with name param)
- Added locale entries in all 3 locales (en-US / zh-CN / zh-TW)
- Updated processFlowView.ts to use `i18n.getMessage(MessageKeys.*)` with params
- Verification: `npm run compile` ✅, `contract-smoke` ✅

## Verification summary (round 1 & 2)

- Backend:
  - `cargo check` ✅
  - `cargo clippy --no-default-features --features profile-local -- -D warnings` ✅
  - `cargo clippy --no-default-features --features profile-simple-server -- -D warnings` ✅
  - `cargo clippy --no-default-features --features profile-multi-users-server -- -D warnings` ✅
- GUI:
  - `npm run build` ✅
- VSCode addon:
  - `npm run compile` ✅
  - `node scripts/contract-smoke.js` ✅

## Status after Round 1 & 2: All issues fixed, no new problems detected

- ✅ Zero clippy warnings across all 3 backend profiles
- ✅ User-facing strings (GUI, addon) fully i18n'd
- ✅ Three-end (backend/GUI/addon) consistency verified
- ✅ No outstanding issues

## Round 3: Cargo Feature Design Improvement (2026-05-01)

### Problem
Profile features (`profile-local`, `profile-simple-server`, `profile-multi-users-server`) were mutually exclusive by design but required explicit `--no-default-features` flag for non-default profiles, creating friction in the build workflow.

### Solution (Plan B - Cargo Layer)
1. **Enhanced Cargo.toml documentation** - Clarified that profiles are mutually exclusive and added compile-time validation notes
2. **Created `.cargo/config.toml` with build aliases** - Convenient shortcuts for all profile combinations:
   - `cargo build-local` / `cargo build-server` / `cargo build-multi` (debug builds)
   - `cargo release-local` / `cargo release-server` / `cargo release-multi` (release builds)
   - `cargo check-local` / `cargo check-server` / `cargo check-multi` (quick validation)
   - `cargo clippy-local -- -D warnings` (lint all profiles)
   - `cargo test-local` / `cargo test-server` / `cargo test-multi` (profile-specific tests)

### Result
- ✅ All three profiles compile successfully
- ✅ Cargo aliases simplify command invocation (no more manual `--no-default-features`)
- ✅ Better documentation in Cargo.toml about feature design
- ✅ Future-proof for Rust's native mutually-exclusive-features support (when stabilized)

### Verification (round 3)
- `cargo build` (default profile-local) ✅
- `cargo check-server` (simple-server profile via alias) ✅
- `cargo build --no-default-features --features profile-multi-users-server` ✅
- `cargo clippy-local -- -D warnings` ✅
- `cargo clippy-server -- -D warnings` ✅
- `cargo clippy-multi -- -D warnings` ✅

## Summary

**Three rounds of systematic improvements:**
1. Round 1: Fixed 3 immediate critical issues (backend compile, GUI/addon i18n hardcoding)
2. Round 2: Fixed 3 additional issues discovered in deeper scan (PathBuf conversion, global error handler, processFlowView i18n)
3. Round 3: Improved developer experience by adding Cargo build aliases for profile selection

**Final state:**
- ✅ All three ends (backend/GUI/addon) verified without errors
- ✅ Zero clippy warnings on all backend profiles
- ✅ All user-facing strings fully i18n'd
- ✅ Build process simplified with cargo aliases
- ✅ Cargo.toml feature design documented for clarity
