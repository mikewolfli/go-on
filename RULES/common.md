# Universal Team Coding Conventions

## Universal Enhanced Workflow Contract

- Follow phase order for development tasks: think -> act -> check -> done.
- Think phase must define objective, constraints, impacted modules, and acceptance checks.
- Act phase must deliver optimal, high-quality changes and keep tests/docs aligned with behavior changes.
- Check phase must include runnable proof for changed surfaces (compile/lint/test/contract as applicable).
- Done phase must report what changed, what was verified, and any known residual risk.
- If check fails, return to think with root-cause evidence; do not ship partial fixes.

- Functions and methods must be cohesive and single-responsibility.
- Maintain naming conventions and respect file/module boundaries.
- Add or update tests for all non-trivial logic changes.
- Keep user-visible behavior and documentation aligned in the same change.
- Error messages must be explicit and actionable.
- Never introduce hidden global state or non-deterministic side effects.
- Reuse existing helpers before adding new logic; avoid duplication.
- Make timeout, rate-limit, and breaker behavior explicit in code paths.
- Always check for existing functions, classes, or modules before adding new ones.
- Task lists must be complete, executed in order, and every step must be implemented.
- If a rule below is marked as language-specific (e.g., Rust), and the current project is not that language, skip or adapt the rule accordingly.

## Phase 4 Multi-Bus Integration Conventions

- All new capabilities must hook into at least one of sense decide evolve execute_tool gateways
- HarnessBus evaluate must be called before ToolBus execute_tool for governance check
- ObservabilityBus record must be called after every tool execution for latency tracking
- Every fault tolerance and resilience module must include an E2E lifecycle test with 10 nodes or more
- Fault tolerance E2E test must verify full state transition Online Offline Recovering Online
- Stress tests must include 500 nodes with batch fault injection and recovery verification
- Fault tolerance modules must include an integration test connected to MultiChannelTransport
- Checkpoint parent chain must auto-detect branch head when parent_checkpoint_id is None
- After checkpoint pruning the chain must remain intact parent_id must resolve to existing record
- Checkpoint rollback must restore exact checkpointed state not partial or default state
- MultiChannelTransport must support 6 channels Heartbeat Control Data Event Log Admin
- Messages must support 4 priority levels Low Normal High Critical
- ExactlyOnce QoS requires dedup via sent_ids HashSet
- Peek non-destructive read must be available for queue inspection
- Convenience methods send_control send_data send_event send_heartbeat are recommended
- Node lifecycle must follow Online Degraded Offline Isolation RecoveryPlan Recovering Online
- reintegrate_node must auto-resolve all active faults for the reintegrated node
- Cluster health scoring must use Healthy 10pct degraded Degraded 10-30pct Critical 30-50pct Down 50pct-plus
- Recovery escalation must follow Auto Coordinated Manual
- Never remove allow dead_code without verifying the annotated item is actually used in production
- Use cfg test instead of allow dead_code for test-only code
- For Bucket D F G H I J code retain precise per-item allow dead_code with F-GAP reference comment
- File-level allow dead_code is forbidden
- Module-level allow dead_code is forbidden
