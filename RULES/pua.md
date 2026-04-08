# PUA Runtime Enforcement Rules

Scope: all agent interactions proxied through go-on.
Activation: automatic on each request.
Authority: merged and refined from .github/copilot-instructions.md.

## Three Red Lines

Red line 1: Close the loop.
- Reject claims like "I think it works" without build or test proof.
- Require concrete output such as cargo check, cargo test, or npm build success.

Red line 2: Fact-driven verification.
- Reject unverified attribution such as "probably environment issue".
- Require file checks, grep evidence, logs, and exact root-cause references.

Red line 3: Exhaust approaches.
- Reject early-exit responses after repeated failure.
- Require methodology switching and full checklist completion.

## Pressure Escalation

- L0: normal execution.
- L1: after first failure, force a different approach.
- L2: after repeated failure, require deep search plus multiple hypotheses.
- L3: execute all checklist items.
- L4: invert assumptions and run opposite strategy.

L3 checklist (all required):
1. Read and quote exact error text.
2. Grep the codebase for related symbols/patterns.
3. Trace error/stack to concrete file and line.
4. Verify dependency/version compatibility.
5. Isolate minimal reproduction.
6. Use verbose or debug output.
7. Check version-specific documentation.

## Quality Compass (pre-delivery)

All items must pass before completion:
1. Build proof shown.
2. Error paths tested.
3. Pattern category scanned (iceberg rule).
4. Root cause and prevention explained.
5. Quality improved with explicit rationale.

## Iceberg Rule

Fix one bug category, then scan and address all similar instances in scope.

Examples:
- Empty catch block found -> scan for all empty catches.
- Unsafe evaluation pattern found -> scan for all unsafe patterns.
- Type mismatch found -> scan related module/type boundaries.

## Methodology Router

When stuck, explicitly switch methodology:
- Huawei: RCA and self-attack debugging.
- Amazon: Working backwards architecture.
- ByteDance: A/B metrics-driven iteration.
- Baidu: Search-first investigation.
- Musk: delete and simplify path.
- Jobs: subtraction and quality focus.
- Tencent: parallel multi-approach race.
- Meituan: standardize and scale.
- Pinduoduo: shorten dependency chain.
- Netflix: high bar execution.
- Xiaomi: single-focus breakthrough.
- JD: execution red line.
- Alibaba: goal-process-result closed loop.

## Auto-trigger Phrases

Escalate when output contains unverified language or surrender patterns:
- "I think", "maybe", "probably", "should work"
- "We cannot solve this", "need more context", "beyond scope"

## Enforcement in go-on

On each request:
1. Extract task and track failure count.
2. Validate red lines.
3. Apply escalation level.
4. Enforce quality compass.
5. Enforce iceberg scan evidence.
6. Reject/return for correction if any requirement fails.

Recommended observability:
- Rule violation count by type.
- Escalation level transitions.
- Quality compass score trend.
- Pattern-scan completion ratio.

