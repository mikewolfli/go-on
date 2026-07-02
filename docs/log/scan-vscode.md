# VS Code Addon Source Code Scan Report

**Date:** 2026-07-02
**Scope:** `/Users/mikewolfli/Desktop/workspace/go-on/vscode-addon/src/`

---

## 1. `console.log` / `console.warn` That Should Use the Logger

The project has a proper `Logger` class (`logger.ts`) with scoped `log.info`, `log.warn`, `log.error` calls that route to VSCode's output channel. Many files bypass it.

### console.warn → should be log.warn

| File | Line | Code | Notes |
|------|------|------|-------|
| `approvalPanel.ts` | 68 | `console.warn("[approvalPanel] message error:", msg)` | eslint `no-console` suppressed; `log` not available (no import) |
| `approvalPanel.ts` | 132 | `console.warn("[approvalPanel] _fetchPendingRequests failed:", err)` | Same pattern |
| `commandRegistry.ts` | 209 | `console.warn("go-on: failed to fetch session list...", err)` | eslint `no-console` suppressed; no logger used |
| `coreCommandRegistry.ts` | 283 | `console.warn("[coreCommandRegistry] Invalid JSON params:", err)` | eslint `no-console` suppressed |
| `workflowView.ts` | 68 | `console.warn(\`[workflow] delete error: ${err}\`)` | eslint `no-console` suppressed; no logger used |
| `workflowView.ts` | 87 | `console.warn(\`[workflow] showConfirm error: ${err}\`)` | eslint `no-console` suppressed |
| `extension.ts` | 840 | `console.warn("go-on: backend readiness check failed:", err)` | eslint `no-console` suppressed |
| `extension.ts` | 862 | `console.warn("[extension] autoOpenChat failed:", err)` | eslint `no-console` suppressed |

### console.log → should be log.info (with framedProtocol logger)

| File | Line | Code | Notes |
|------|------|------|-------|
| `runtime/framedProtocol.ts` | 220 | `console.log(msg)` | Uses `console.log` despite having `const log = Logger.forModule("framedProtocol")` available at line 5. Should be `log.info(msg)`. |

### File-wide eslint-disable no-console

| File | Line | Issue |
|------|------|-------|
| `configManager.ts` | 1 | `/* eslint-disable no-console */` — disables the rule for the entire file. Contains 6 `console.warn` calls (lines 180, 198, 216, 221, 252, 267) that should use a logger |
| `chatView.ts` | 1 | `/* eslint-disable no-console */` — entire file disabled. Contains multiple `console.warn` and `console.error` calls |

### console.error → should be log.error

| File | Line | Code |
|------|------|------|
| `extension.ts` | 926 | `console.error(...)` with eslint `no-console` suppressed |

---

## 2. Dead Code — Unused Exports

### `stateSync.ts` — `stateSyncEventSummary` exported but only used internally

- **Export at line 45:** `export function stateSyncEventSummary(event: StateSyncEvent): string`
- **Usage:** Only referenced inside `stateSync.ts` itself at line 194.
- **No external consumers found.** This function is publicly exported but never imported or used by any other file. Consider making it `private` or removing the export.

### `utils.ts` — `isRecord` exported but only used internally

- **Export at line 14:** `export function isRecord(value: unknown): value is Record<string, unknown>`
- **Usage:** Only referenced inside `utils.ts` by `asRecord()` at line 23.
- **No external consumers found.** The type guard is useful but currently unused outside the module. Consider removing the export or adding consumers.

---

## 3. Duplicated / Redundant Code

### `workflowView.ts` duplicates `getErrorMessage` from utils

- **File:** `workflowView.ts`, lines 134-136
- **Code:**
  ```typescript
  private getErrorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }
  ```
- **Issue:** This is an exact copy of `getErrorMessage` from `utils.ts` (line 29-31). The method is only used once (line 207) in the same file. Should import from `./utils` instead.

### `extension.ts` — `getPrimaryWorkspaceRoot()` duplicates similar functions

- **`extension.ts` line 47:** Returns `vscode.Uri | undefined`
- **`coreCommandRegistry.ts` line 36:** `getPrimaryWorkspaceFolder()` returns `vscode.WorkspaceFolder | undefined`
- These serve slightly different purposes but the workspace-folder lookup logic is duplicated. Minor issue.

### Test calculation mismatch: `reconnect.test.ts` diverges from production

- **Test file:** `test/suite/reconnect.test.ts`
- **`BASE_DELAY = 2000`** (line 15) and **`MAX_DELAY = 300000`** (line 16)
- **Production:** `runtime/reconnect.ts` uses `BASE_DELAY = 1000` (implicit in `backoffMs()` formula `1000 * Math.pow(2, attempt)`) and `MAX_DELAY = 30000` (line 36 uses `Math.min(baseDelay, 30000)`).
- The test constants (`2000` base, `300000` max) do NOT match the production implementation (`1000` base, `30000` max). Tests pass because they test a standalone reimplementation, not the actual production code.

---

## 4. Eslint Suppressions That Could Be Fixed

### `managerTypes.ts` lines 8, 11 — unused callback params

```typescript
// eslint-disable-next-line @typescript-eslint/no-unused-vars
onToken: (_token: string) => void;
// eslint-disable-next-line @typescript-eslint/no-unused-vars
onError: (_error: Error) => void;
```

These parameters ARE intentionally unused in the interface definition. The leading `_` prefix convention signals intent. These suppressions are acceptable, but the `_` prefix already suppresses the TS error in modern TS strict mode — verify if the eslint overrides are still needed.

### `runtimeManager.ts` lines 783, 787, 793, 867, 906, 912, 914, 1189 — multiple `@typescript-eslint` suppressions

Several `// eslint-disable-next-line` comments for:
- `@typescript-eslint/no-explicit-any` (lines 783, 867, 1189)
- `@typescript-eslint/no-unsafe-member-access` (line 787)
- `@typescript-eslint/no-unsafe-call` (lines 793, 914)
- `@typescript-eslint/no-unsafe-assignment` (lines 793, 906)
- `no-constant-condition` (line 912)

These are needed because of `any` usage in the HTTP request/streaming code. Consider adding proper type annotations to reduce the suppression count, especially for the streaming processing logic.

### `runtimeBinaryService.ts` line 268 — `@typescript-eslint/no-var-requires`

```typescript
// eslint-disable-next-line @typescript-eslint/no-var-requires
const AdmZip = require("adm-zip");
```

Could be replaced with `import AdmZip from "adm-zip"` if the module supports ESM import or has a `@types/adm-zip` package.

---

## 5. Potential Issues / Meaningful Observations

### `Logger` class constructor is private but has eslint suppression

- **File:** `logger.ts`, line 26-27
- `// eslint-disable-next-line no-unused-vars` on the constructor parameter `private readonly moduleName: string` — this eslint-ignore is unnecessary because the `private readonly` keyword both declares and uses the parameter.

### `i18n.ts` — `Language` and `I18nMessages` types are not exported

- **Line 25:** `interface I18nMessages` (not exported)
- **Line 29:** `type Language = "en_US" | "zh_CN" | "zh_TW"` (not exported)
- These types are used by the public API methods of `I18nManager` (e.g., `loadLocale(language: Language)`, `setLanguage(language: Language)`) but consumers cannot reference them by name for type annotations. Should be exported if external use is intended.

### `i18n.ts` — `getFallbackMessages()` always returns empty object

- **Line 656-659:** Returns `{}` always. The parameter `_language` is unused (prefixed with `_`).
- The doc comment (lines 649-655) explains this is intentional ("_These are intentionally minimal to avoid duplicating locale file data_"), relying on en-US.json disk fallback.
- While not a bug, this is ~30 lines of dead logic for a method that returns `{}`. Could be simplified.

### `protocolContract.ts` — `workflowControlModes`, `defaultWorkflowControlMode`, `platformModes`, `defaultPlatformMode`

- Exported at lines 595-604. These are derived from `protocolContract.protocol` but are only consumed internally within the same file (by functions and aliases below them). No external references found.
- Similarly `protocolModeAliases` (line 606) and `CLIENT_SUPPORTED_VERSIONS` (line 453) are constant exports consumed within the module only.

### `PersistedCopilotState` — unused `oauthClientId` field

- **File:** `settings/copilotAuth.ts`, line 12: `oauthClientId?: string;`
- This field is defined in the interface but never set or read anywhere in the extension. The `_baseCopilotAuthState` and `_currentCopilotAuthState` methods in `settingsView.ts` use `oauthClientId` but only when it's stored in persisted state; it's never written by the extension itself. Potentially dead.

---

## Summary

| Category | Count | Most Affected Files |
|----------|-------|---------------------|
| `console.log/warn` bypassing Logger | 11 locations | `configManager.ts` (6), `workflowView.ts` (2), `extension.ts` (2) |
| `console.log` with logger available | 1 | `runtime/framedProtocol.ts:220` |
| File-wide eslint-disable no-console | 2 files | `configManager.ts`, `chatView.ts` |
| Unused export (internal use only) | 2 | `stateSync.ts:45`, `utils.ts:14` |
| Duplicated utility function | 1 | `workflowView.ts:134` |
| Test constants mismatch | 1 | `reconnect.test.ts` BASE_DELAY/MAX_DELAY differ from production |
| Unnecessary eslint suppression | 1 | `logger.ts:26` |
