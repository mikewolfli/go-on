# SafeGuard Mode (Phase 10+)

## Overview

SafeGuard Mode is a new runtime mode that combines automatic execution with targeted user confirmation at high-risk nodes. This mode enables efficient automation while maintaining safety gates for sensitive operations.

## Key Features

### Automatic Execution with Safety Checkpoints
- Operates **fully automatically** for routine/safe operations
- **Halts and requires explicit user approval** when detecting high-risk operations
- Provides optimal balance between automation and safety

### High-Risk Operation Detection
SafeGuard mode automatically flags the following operations as high-risk:

**Data Modifications:**
- `delete`, `remove`, `drop`, `truncate`
- `drop table`, `drop database`

**Reversal Operations:**
- `rollback`, `revert`, `reset`

**Forced Changes:**
- `force`, `downgrade`, `uninstall`

When the orchestrator detects any high-risk operation, it:
1. Pauses automatic execution
2. Notifies the user of the proposed operation
3. Requires explicit confirmation before proceeding
4. Logs the decision for audit purposes

## Usage

### Activation
Select SafeGuard mode when you want:
- Hands-off automation for most tasks
- Safety confirmation only for critical operations
- Minimal context switching (execute, then confirm)

### API
```rust
// Resolve a mode runtime by name ("ask" | "plan" | "edit" |
// "full_auto" | "safeguard"). Signature:
//   resolve_mode_runtime(mode, registry: Option<Arc<AgentRegistry>>,
//                       agent_name: Option<String>) -> Result<Box<dyn ModeRuntime>, String>
let runtime = resolve_mode_runtime(
    "safeguard",
    Some(registry.clone()),
    Some("agent-name".to_string()),
    // Note: `ModeKind::SafeGuard` is also resolvable via
    // `resolve_mode_runtime_with_posture(..., posture)`.
)?;

// Check if operation is high-risk:
if runtime.is_high_risk_operation("delete user table") {
    // Request user confirmation before proceeding
    // Implementation: wire to approval UI/API
}
```

## Mode Hierarchy

**SafeGuard is positioned as one level below FullAuto** in the automation hierarchy:

```
Automation Level Scale (by max tool calls)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Ask        (0)     ▁ Minimal - requires approval for all
Plan       (3)     ▁ Planning only - no execution
Edit       (20)    ▂ Low - constrained edits only
SafeGuard  (30)    ▄ High - mostly automatic, approval for dangerous ops ⭐
FullAuto   (50)    ▅ Maximum - fully automatic, no approvals needed
```

### SafeGuard vs FullAuto (Side by Side)

| Aspect | SafeGuard | FullAuto |
|--------|-----------|----------|
| **Tool Calls Limit** | 30 | 50 |
| **Approval at Deletion** | ✅ Yes | ❌ No |
| **Approval at Rollback** | ✅ Yes | ❌ No |
| **Auto Read Files** | ✅ Yes | ✅ Yes |
| **Auto Search Code** | ✅ Yes | ✅ Yes |
| **Auto Apply Patches** | ✅ Yes | ✅ Yes |
| **Auto Run Tests** | ✅ Yes | ✅ Yes |
| **Scope** | Restricted | Full |
| **Best For** | Safety-conscious automation | Complete trust scenarios |

## Mode Comparison

| Feature | Ask | Plan | Edit | FullAuto | SafeGuard |
|---------|-----|------|------|----------|-----------|
| Auto Execution | No | No | No | Yes | Yes |
| User Approval Required | Always | Always | For all | No | For high-risk ops only |
| Max Tool Calls | 0 | 3 | 20 | 50 | 30 |
| Risk Detection | N/A | N/A | Basic | None | Advanced |
| Use Case | Quick confirmation | Step-by-step planning | Constrained edits | Full automation | Balanced safety |

## Implementation Details

### Code Location
- Mode trait extension: [src/orchestration/mode.rs](src/orchestration/mode.rs)
  - Added `is_high_risk_operation()` method to ModeRuntime trait
  - All existing modes have default implementation
  - SafeGuard mode provides comprehensive risk detection
  
- Orchestrator integration: [src/orchestration/orchestrator.rs](src/orchestration/orchestrator.rs)
  - `resolve_mode_runtime()` (src/orchestration/mode.rs) recognizes "safeguard"
  - Imports SafeGuardModeRuntime

### Mode Configuration

Modes (`Ask` / `Plan` / `Edit` / `SafeGuard` / `FullAuto`) are defined in code
(`src/orchestration/mode.rs` `ModeKind`); there is no `[[modes]]` config table.
Select a mode at runtime via `mode = "..."` in the request, the GUI mode
selector, or Zed's `go-on` agent mode mapping.

## Future Enhancements

1. **Configurable Risk Levels**: Allow projects to define custom high-risk patterns
2. **Risk Scoring**: Assign confidence scores to operations (1-5 scale)
3. **Approval Delegation**: Route approvals to different users based on operation type
4. **Audit Logging**: Complete record of all high-risk operations and approvals
5. **Rollback Support**: Quick undo for recently approved operations

## Testing

SafeGuard mode includes test coverage:
- High-risk operation detection
- Approval requirement behavior
- Mode selection
- Tool availability verification

```bash
cargo test safeguard
```

## See Also

- [RULES.md](RULES.md) - Code review standards
- [src/orchestration/mode.rs](src/orchestration/mode.rs) - Mode runtime implementations
