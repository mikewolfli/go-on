# ACP Module Migration Plan (Phase 2: include! to mod)

## Overview
This document outlines the step-by-step plan to migrate the ACP module from using `include!` macros to proper Rust module structure. The migration will be executed in phases to maintain system stability and ensure backward compatibility.

## Migration Principles

### Safety First
1. **One file at a time**: Migrate only one file per commit
2. **Test after each step**: Run `cargo check` and relevant tests
3. **Small commits**: Keep changes focused and reviewable
4. **Rollback points**: Create git tags at major milestones

### Compatibility Guarantee
1. **Preserve public API**: No changes to function signatures
2. **Gradual replacement**: Use parallel structure during transition
3. **Feature flags**: Optional for complex transitions
4. **Documentation updates**: Keep docs in sync with changes

### Quality Assurance
1. **Module-level testing**: Each module must have tests
2. **Performance benchmarking**: Monitor for regressions
3. **Code review required**: All changes must be reviewed
4. **Documentation completeness**: Update all affected docs

## Phase 1: Preparation (1-2 hours)
**Goal**: Establish foundation without breaking existing functionality

### Tasks
1. **Create new module structure**
   - Create `acp/modules/` directory
   - Create `acp/modules/mod.rs` as entry point
   - Keep existing `include!` structure intact

2. **Set up validation**
   - Create migration test script
   - Ensure all existing tests pass
   - Establish performance baseline

### Success Criteria
- ✅ New directory structure exists
- ✅ All tests pass with original structure
- ✅ Performance baseline recorded

## Phase 2: Migrate Helpers Modules (2-3 hours)
**Goal**: Migrate relatively independent helper modules

### Tasks
1. **Migrate `helpers/context.rs`**
   - Create `acp/modules/helpers/context.rs`
   - Copy original content, remove `include!` dependencies
   - Update import paths
   - Verify compilation

2. **Migrate remaining helpers**
   - `helpers/policy.rs`
   - `helpers/misc.rs`
   - `helpers/requirement.rs`
   - `helpers/conversation.rs`
   - `helpers/metrics.rs`

3. **Update `acp/mod.rs`**
   - Add `pub mod modules;`
   - Maintain backward-compatible re-exports

### Success Criteria
- ✅ All helper modules migrated
- ✅ Compilation passes
- ✅ Tests for helper modules pass

## Phase 3: Migrate Implementation Modules (4-6 hours)
**Goal**: Migrate core implementation modules (most complex part)

### Subphase 3.1: `impl/runtime.rs`
1. **Convert to standalone functions**
   - Transform `impl AcpServer` block to standalone functions
   - Add `AcpServer` as first parameter where needed
   - Maintain identical function signatures

2. **Create module file**
   - Create `acp/modules/impl/runtime.rs`
   - Export all public functions
   - Add necessary imports

3. **Update `server.rs`**
   - Add `use crate::acp::modules::impl::runtime;`
   - Create wrapper methods that call new functions

### Subphase 3.2: Other `impl` modules
1. **`impl/request.rs`**
   - Migrate `handle_request` function
   - Handle complex dependencies

2. **`impl/chat.rs`**
   - Migrate `handle_chat` and `should_escalate_approval_strategy`

3. **Remaining `impl` modules**
   - `impl/conversation.rs`
   - `impl/storage.rs`
   - `impl/agent.rs`
   - `impl/io.rs`

### Success Criteria
- ✅ All `impl` modules migrated
- ✅ `AcpServer` methods work via new structure
- ✅ All ACP tests pass

## Phase 4: Migrate Top-Level Modules (2-3 hours)
**Goal**: Migrate `prelude`, `server`, `background`, etc.

### Tasks
1. **Migrate `prelude.rs`**
   - Create `acp/modules/prelude.rs`
   - Handle complex imports and re-exports

2. **Migrate `server.rs`**
   - Create `acp/modules/server.rs`
   - Move `AcpServer` struct to new location
   - Update all method implementations

3. **Migrate `background.rs`**
   - Create `acp/modules/background.rs`
   - Migrate maintenance loop code

4. **Migrate `tests.rs`**
   - Create `acp/modules/tests.rs`
   - Ensure tests continue to work

### Success Criteria
- ✅ All top-level modules migrated
- ✅ `AcpServer` fully functional via new structure
- ✅ All integration tests pass

## Phase 5: Integration and Cleanup (2-3 hours)
**Goal**: Complete migration and clean up old code

### Tasks
1. **Update `acp/mod.rs`**
   - Remove all `include!` statements
   - Use new module structure
   - Maintain identical public API

2. **Update dependency imports**
   - Search and update all `crate::acp::` imports
   - Ensure correct paths

3. **Delete old files**
   - Backup old `include!` files
   - Delete unnecessary files
   - Update `.gitignore`

4. **Final validation**
   - Run complete test suite
   - Check for performance regressions
   - Verify API compatibility

### Success Criteria
- ✅ No `include!` macros remain in ACP module
- ✅ All tests pass
- ✅ Public API unchanged
- ✅ Performance within 5% of baseline

## Phase 6: Optimization and Refactoring (Optional, 2-4 hours)
**Goal**: Leverage new structure for improvements

### Tasks
1. **Module boundary optimization**
   - Analyze inter-module dependencies
   - Extract common dependencies to `common` module
   - Reduce circular dependencies

2. **Interface design improvements**
   - Define clear module interfaces
   - Add documentation comments
   - Create module-level tests

3. **Performance optimization**
   - Analyze compilation times
   - Optimize import tree
   - Reduce unnecessary public items

### Success Criteria
- ✅ Cleaner module boundaries
- ✅ Better documentation
- ✅ Improved compilation times

## Execution Rules

### Branch and Merge Rules
1. **Branch**: Create `migrate-acp-modules` branch from `main`
2. **Commits**: Each phase should have its own commit
3. **Validation**: Must pass `cargo check` and relevant tests before commit
4. **Merge**: Only after all phases complete and tests pass

### Risk Mitigation
1. **Rollback plan**: Keep original files until Phase 5 completion
2. **Feature flags**: Use `#[cfg(feature = "new_modules")]` if needed
3. **Parallel testing**: Run tests on both old and new structures during transition
4. **Performance monitoring**: Track compilation and runtime performance

### Validation Gates
Each phase must pass:
- `cargo check` without errors
- `cargo test --lib` for library tests
- `cargo test --bin go-on` for binary tests
- `cargo test acp` for ACP-specific tests

## Timeline Estimate

| Phase | Estimated Time | Priority | Risk Level |
|-------|----------------|----------|------------|
| Phase 1 | 1-2 hours | High | Low |
| Phase 2 | 2-3 hours | Medium | Low |
| Phase 3 | 4-6 hours | High | Medium |
| Phase 4 | 2-3 hours | Medium | Low |
| Phase 5 | 2-3 hours | High | Medium |
| Phase 6 | 2-4 hours | Low | Low |
| **Total** | **13-21 hours** | | |

## Done Criteria
The migration is complete when all conditions are met:

1. ✅ No `include!` macros in `acp/` module
2. ✅ All tests pass with new structure
3. ✅ Public API identical to original
4. ✅ Compilation time within acceptable bounds
5. ✅ Documentation updated
6. ✅ Code review completed

## Notes
- This migration follows the same principles as REORG.MD
- Priority is on stability over speed
- Each step should be reversible
- Communication is key - update team on progress

## Version History
- **2026-04-09**: Initial migration plan created
- **Status**: Ready for execution