#!/bin/bash

# CI测试脚本
# 这个脚本模拟GitHub Actions CI流程中的所有步骤

set -e  # 任何命令失败则退出
echo "=== 开始CI测试 ==="

run_strict_clippy() {
    local uname_out
    uname_out=$(uname -s 2>/dev/null || echo "unknown")
    case "${uname_out}" in
        MINGW*|MSYS*|CYGWIN*)
            echo "检测到 Windows 环境，使用严格 bins 门禁（Unix-only 依赖 target 跳过）"
            cargo clippy --bins -- -D warnings
            ;;
        *)
            cargo clippy --all-targets -- -D warnings
            ;;
    esac
}

# 1. 构建项目
echo "=== 步骤1: 构建项目 ==="
cargo build --verbose
echo "✅ 构建成功"

# 2. 运行测试
echo "=== 步骤2: 运行测试 ==="
cargo test --all --verbose
echo "✅ 测试通过"

# 3. 代码检查 (严格模式)
echo "=== 步骤3: 代码检查 ==="
run_strict_clippy
echo "✅ 代码检查通过"

# 4. 格式化检查
echo "=== 步骤4: 格式化检查 ==="
cargo fmt --all -- --check
echo "✅ 格式化检查通过"

# 5. 运行i18n模块全量测试
echo "=== 步骤5: 运行i18n模块全量测试 ==="
cargo test i18n:: -- --nocapture
echo "✅ i18n模块全量测试通过"

# 5a Core Config Unit Suite主链测试
echo "=== 步骤5a: 运行Core Config Unit Suite主链测试 ==="
cargo test core::config::tests:: -- --nocapture
echo "✅ Core Config Unit Suite主链测试通过"

# 5b Governance PUA Unit Suite主链测试
echo "=== 步骤5b: 运行Governance PUA Unit Suite主链测试 ==="
cargo test governance::pua::tests:: -- --nocapture
echo "✅ Governance PUA Unit Suite主链测试通过"

# 5c MCP Module Unit Suite主链测试
echo "=== 步骤5c: 运行MCP Module Unit Suite主链测试 ==="
cargo test mcp::tests:: -- --nocapture
echo "✅ MCP Module Unit Suite主链测试通过"

# 5d Protocol MCP Server Unit Suite主链测试
echo "=== 步骤5d: 运行Protocol MCP Server Unit Suite主链测试 ==="
cargo test protocol::mcp_server::tests:: -- --nocapture
echo "✅ Protocol MCP Server Unit Suite主链测试通过"

# 5e OpenAI Compatible Agent Unit Suite主链测试
echo "=== 步骤5e: 运行OpenAI Compatible Agent Unit Suite主链测试 ==="
cargo test agents::openai_compatible::tests:: -- --nocapture
echo "✅ OpenAI Compatible Agent Unit Suite主链测试通过"

# 5f Memory Cache Unit Suite主链测试
echo "=== 步骤5f: 运行Memory Cache Unit Suite主链测试 ==="
cargo test memory::cache::tests:: -- --nocapture
echo "✅ Memory Cache Unit Suite主链测试通过"

# 5g Memory Vector Unit Suite主链测试
echo "=== 步骤5g: 运行Memory Vector Unit Suite主链测试 ==="
cargo test memory::vector::tests:: -- --nocapture
echo "✅ Memory Vector Unit Suite主链测试通过"

# 5h Orchestration Task Router Unit Suite主链测试
echo "=== 步骤5h: 运行Orchestration Task Router Unit Suite主链测试 ==="
cargo test orchestration::task_router::tests:: -- --nocapture
echo "✅ Orchestration Task Router Unit Suite主链测试通过"

# 5i Orchestration Flow Unit Suite主链测试
echo "=== 步骤5i: 运行Orchestration Flow Unit Suite主链测试 ==="
cargo test orchestration::flow::tests:: -- --nocapture
echo "✅ Orchestration Flow Unit Suite主链测试通过"

# 5j Orchestration Flow With Models Unit Suite主链测试
echo "=== 步骤5j: 运行Orchestration Flow With Models Unit Suite主链测试 ==="
cargo test orchestration::flow_with_models::tests:: -- --nocapture
echo "✅ Orchestration Flow With Models Unit Suite主链测试通过"

# 5k Orchestration Orchestrator Unit Suite主链测试
echo "=== 步骤5k: 运行Orchestration Orchestrator Unit Suite主链测试 ==="
cargo test orchestration::orchestrator::tests:: -- --nocapture
echo "✅ Orchestration Orchestrator Unit Suite主链测试通过"

# 5l Orchestration Tool Unit Suite主链测试
echo "=== 步骤5l: 运行Orchestration Tool Unit Suite主链测试 ==="
cargo test orchestration::tool::tests:: -- --nocapture
echo "✅ Orchestration Tool Unit Suite主链测试通过"

# 5m Core Error Unit Suite主链测试
echo "=== 步骤5m: 运行Core Error Unit Suite主链测试 ==="
cargo test core::error::tests:: -- --nocapture
echo "✅ Core Error Unit Suite主链测试通过"

# 5n Copilot Agent Unit Suite主链测试
echo "=== 步骤5n: 运行Copilot Agent Unit Suite主链测试 ==="
cargo test agents::copilot::tests:: -- --nocapture
echo "✅ Copilot Agent Unit Suite主链测试通过"

# 5o Anthropic Agent Unit Suite主链测试
echo "=== 步骤5o: 运行Anthropic Agent Unit Suite主链测试 ==="
cargo test agents::anthropic::tests:: -- --nocapture
echo "✅ Anthropic Agent Unit Suite主链测试通过"

# 5p Qwen Agent Unit Suite主链测试
echo "=== 步骤5p: 运行Qwen Agent Unit Suite主链测试 ==="
cargo test agents::qwen::tests:: -- --nocapture
echo "✅ Qwen Agent Unit Suite主链测试通过"

# 5q Wenxin Agent Unit Suite主链测试
echo "=== 步骤5q: 运行Wenxin Agent Unit Suite主链测试 ==="
cargo test agents::wenxin::tests:: -- --nocapture
echo "✅ Wenxin Agent Unit Suite主链测试通过"

# 5r DeepSeek Agent Unit Suite主链测试
echo "=== 步骤5r: 运行DeepSeek Agent Unit Suite主链测试 ==="
cargo test agents::deepseek::tests:: -- --nocapture
echo "✅ DeepSeek Agent Unit Suite主链测试通过"

# 5s Optimization Cost Optimizer Unit Suite主链测试
echo "=== 步骤5s: 运行Optimization Cost Optimizer Unit Suite主链测试 ==="
cargo test optimization::cost_optimizer::tests:: -- --nocapture
echo "✅ Optimization Cost Optimizer Unit Suite主链测试通过"

# 5t Optimization Speed Optimizer Unit Suite主链测试
echo "=== 步骤5t: 运行Optimization Speed Optimizer Unit Suite主链测试 ==="
cargo test optimization::speed_optimizer::tests:: -- --nocapture
echo "✅ Optimization Speed Optimizer Unit Suite主链测试通过"

# 5u Optimization Reliability Optimizer Unit Suite主链测试
echo "=== 步骤5u: 运行Optimization Reliability Optimizer Unit Suite主链测试 ==="
cargo test optimization::reliability_optimizer::tests:: -- --nocapture
echo "✅ Optimization Reliability Optimizer Unit Suite主链测试通过"

# 5v Optimization Failure Prevention Unit Suite主链测试
echo "=== 步骤5v: 运行Optimization Failure Prevention Unit Suite主链测试 ==="
cargo test optimization::failure_prevention::tests:: -- --nocapture
echo "✅ Optimization Failure Prevention Unit Suite主链测试通过"

# 5w Intelligence Adaptive Selector Unit Suite主链测试
echo "=== 步骤5w: 运行Intelligence Adaptive Selector Unit Suite主链测试 ==="
cargo test intelligence::adaptive_selector::tests:: -- --nocapture
echo "✅ Intelligence Adaptive Selector Unit Suite主链测试通过"

# 5x Intelligence Model Selector Unit Suite主链测试
echo "=== 步骤5x: 运行Intelligence Model Selector Unit Suite主链测试 ==="
cargo test intelligence::model_selector::tests:: -- --nocapture
echo "✅ Intelligence Model Selector Unit Suite主链测试通过"

# 5y Intelligence Advanced Modules Unit Suite主链测试
echo "=== 步骤5y: 运行Intelligence Advanced Modules Unit Suite主链测试 ==="
cargo test intelligence::advanced_modules::tests:: -- --nocapture
echo "✅ Intelligence Advanced Modules Unit Suite主链测试通过"

# 5z Intelligence Reinforcement Unit Suite主链测试
echo "=== 步骤5z: 运行Intelligence Reinforcement Unit Suite主链测试 ==="
cargo test intelligence::reinforcement::tests:: -- --nocapture
echo "✅ Intelligence Reinforcement Unit Suite主链测试通过"

# 5aa Core Setup Unit Suite主链测试
echo "=== 步骤5aa: 运行Core Setup Unit Suite主链测试 ==="
cargo test core::setup::tests:: -- --nocapture
echo "✅ Core Setup Unit Suite主链测试通过"

# 5ab Generic Agent Unit Suite主链测试
echo "=== 步骤5ab: 运行Generic Agent Unit Suite主链测试 ==="
cargo test agents::agent::tests:: -- --nocapture
echo "✅ Generic Agent Unit Suite主链测试通过"

# 5ac ACP Prelude Metrics Unit Suite主链测试
echo "=== 步骤5ac: 运行ACP Prelude Metrics Unit Suite主链测试 ==="
cargo test acp::prelude::tests:: -- --nocapture
echo "✅ ACP Prelude Metrics Unit Suite主链测试通过"

# 5ac-1 ACP Timeout Model Unit Suite主链测试
echo "=== 步骤5ac-1: 运行ACP Timeout Model Unit Suite主链测试 ==="
cargo test run_with_optional_timeout_returns_timeout_error -- --nocapture
cargo test probe_agent_runtime_readiness_accepts_async_local_listener -- --nocapture
cargo test runtime_metrics_tracks_agent_and_probe_timeouts -- --nocapture
echo "✅ ACP Timeout Model Unit Suite主链测试通过"

# 5ad Orchestration Skill Registry Unit Suite主链测试
echo "=== 步骤5ad: 运行Orchestration Skill Registry Unit Suite主链测试 ==="
cargo test orchestration::skill::tests:: -- --nocapture
echo "✅ Orchestration Skill Registry Unit Suite主链测试通过"

# 5ae Rust All Targets Full Gate主链测试（全量兜底）
echo "=== 步骤5ae: 运行Rust All Targets Full Gate主链测试（全量兜底） ==="
cargo test --workspace --all-targets -- --nocapture
echo "✅ Rust All Targets Full Gate主链测试通过"

# 5.1 OpenAI兼容回归测试
echo "=== 步骤5.1: 运行OpenAI兼容回归测试 ==="
cargo test openai_http_request_matrix_regression -- --nocapture
echo "✅ OpenAI兼容回归测试通过"

# 5.1b Responses API R2 回归测试
echo "=== 步骤5.1b: 运行Responses API回归测试 ==="
cargo test responses_api_r1_minimal_request -- --nocapture
echo "✅ Responses API回归测试通过"

# 5.1c Responses 失败分类单测
echo "=== 步骤5.1c: 运行Responses失败分类单测 ==="
cargo test responses_api_upstream_error_classification_is_stable -- --nocapture
echo "✅ Responses失败分类单测通过"

# 5.1d Responses 输入映射单测
echo "=== 步骤5.1d: 运行Responses输入映射单测 ==="
cargo test responses_api_maps_input_to_messages -- --nocapture
echo "✅ Responses输入映射单测通过"

# 5.1e Responses ID 唯一性单测
echo "=== 步骤5.1e: 运行Responses ID唯一性单测 ==="
cargo test responses_api_id_generation_is_unique -- --nocapture
echo "✅ Responses ID唯一性单测通过"

# 5.1f Responses 流式事件类型单测
echo "=== 步骤5.1f: 运行Responses流式事件类型单测 ==="
cargo test responses_api_stream_event_types_are_correct -- --nocapture
echo "✅ Responses流式事件类型单测通过"

# 5.1g Responses R4 Golden Snapshot 单测
echo "=== 步骤5.1g: 运行Responses R4 Golden Snapshot单测 ==="
cargo test responses_api_r4_golden_snapshot -- --nocapture
echo "✅ Responses R4 Golden Snapshot单测通过"

# 5.1h Responses R4 完整字段矩阵集成测试
echo "=== 步骤5.1h: 运行Responses R4完整字段矩阵集成测试 ==="
cargo test responses_api_r4_complete_field_matrix -- --nocapture
echo "✅ Responses R4完整字段矩阵集成测试通过"

# 5.1i Responses 路由契约集成测试
echo "=== 步骤5.1i: 运行Responses路由契约集成测试 ==="
cargo test responses_api_r4_route_contracts -- --nocapture
echo "✅ Responses路由契约集成测试通过"

# 5.1j Responses 流式降级集成测试
echo "=== 步骤5.1j: 运行Responses流式降级集成测试 ==="
cargo test responses_api_stream_degrades_setup_unavailable -- --nocapture
echo "✅ Responses流式降级集成测试通过"

# 5.1k Responses 非流式降级集成测试
echo "=== 步骤5.1k: 运行Responses非流式降级集成测试 ==="
cargo test responses_api_non_stream_degrades_setup_unavailable -- --nocapture
echo "✅ Responses非流式降级集成测试通过"

# 5.1l ACP/MCP 共存适配集成测试
echo "=== 步骤5.1l: 运行ACP/MCP共存适配集成测试 ==="
cargo test rpc_mcp_adapter_initialize_list_and_call -- --nocapture
echo "✅ ACP/MCP共存适配集成测试通过"

# 5.1m Auto模式三链路共存集成测试
echo "=== 步骤5.1m: 运行Auto模式三链路共存集成测试 ==="
cargo test rpc_auto_mode_http_root_acp_and_mcp_coexist -- --nocapture
echo "✅ Auto模式三链路共存集成测试通过"

# 5.1n GUI协议模式解析单测
echo "=== 步骤5.1n: 运行GUI协议模式解析单测 ==="
cargo test --manifest-path GUI/src-tauri/Cargo.toml protocol_mode_parser -- --nocapture
echo "✅ GUI协议模式解析单测通过"

# 5.1o GUI Tauri Rust编译检查
echo "=== 步骤5.1o: 运行GUI Tauri Rust编译检查 ==="
cargo test --manifest-path GUI/src-tauri/Cargo.toml --no-run
echo "✅ GUI Tauri Rust编译检查通过"

# 5.1p RPC Chat Provider Fallback降级集成测试
echo "=== 步骤5.1p: 运行RPC Chat Provider Fallback降级集成测试 ==="
cargo test rpc_chat_provider_failure_degrades_to_fallback_agent -- --nocapture
echo "✅ RPC Chat Provider Fallback降级集成测试通过"

# 5.1q RPC Config Reload and Runtime Warnings集成测试
echo "=== 步骤5.1q: 运行RPC Config Reload and Runtime Warnings集成测试 ==="
cargo test rpc_config_reload_reports_runtime_warnings -- --nocapture
echo "✅ RPC Config Reload and Runtime Warnings集成测试通过"

# 5.1r RPC Chat Review Timeout Collision集成测试
echo "=== 步骤5.1r: 运行RPC Chat Review Timeout Collision集成测试 ==="
cargo test rpc_chat_review_timeout_collision_reports_timeout_and_gate_outcome -- --nocapture
echo "✅ RPC Chat Review Timeout Collision集成测试通过"

# 5.1s RPC Initialize Health Phase and Shutdown集成测试
echo "=== 步骤5.1s: 运行RPC Initialize Health Phase and Shutdown集成测试 ==="
cargo test rpc_initialize_health_phase_and_shutdown -- --nocapture
echo "✅ RPC Initialize Health Phase and Shutdown集成测试通过"

# 5.1t HTTP Chat Stream SSE and Knowledge Persistence集成测试
echo "=== 步骤5.1t: 运行HTTP Chat Stream SSE and Knowledge Persistence集成测试 ==="
cargo test http_chat_stream_emits_sse_and_persists_knowledge -- --nocapture
echo "✅ HTTP Chat Stream SSE and Knowledge Persistence集成测试通过"

# 5.1u RPC Debug Panel Snapshot and Runtime Data集成测试
echo "=== 步骤5.1u: 运行RPC Debug Panel Snapshot and Runtime Data集成测试 ==="
cargo test rpc_debug_panel_snapshot_contains_runtime_and_conversation_data -- --nocapture
echo "✅ RPC Debug Panel Snapshot and Runtime Data集成测试通过"

# 5.1v RPC Conversation Checkpoint and Rollback集成测试
echo "=== 步骤5.1v: 运行RPC Conversation Checkpoint and Rollback集成测试 ==="
cargo test rpc_conversation_checkpoint_and_rollback -- --nocapture
echo "✅ RPC Conversation Checkpoint and Rollback集成测试通过"

# 5.1w RPC Cache Clear and Checkpoint Missing Messages集成测试
echo "=== 步骤5.1w: 运行RPC Cache Clear and Checkpoint Missing Messages集成测试 ==="
cargo test rpc_cache_clear_and_checkpoint_missing_messages -- --nocapture
echo "✅ RPC Cache Clear and Checkpoint Missing Messages集成测试通过"

# 5.1x RPC Chat Rate Limit Saturation集成测试
echo "=== 步骤5.1x: 运行RPC Chat Rate Limit Saturation集成测试 ==="
cargo test rpc_chat_rate_limit_saturation_returns_rate_limited_error -- --nocapture
echo "✅ RPC Chat Rate Limit Saturation集成测试通过"

# 5.1y RPC Rejects Non-2.0 JSON-RPC Version集成测试
echo "=== 步骤5.1y: 运行RPC Rejects Non-2.0 JSON-RPC Version集成测试 ==="
cargo test rpc_rejects_non_2_0_jsonrpc_version -- --nocapture
echo "✅ RPC Rejects Non-2.0 JSON-RPC Version集成测试通过"

# 5.1z RPC Chat Rejects Invalid Parameters集成测试
echo "=== 步骤5.1z: 运行RPC Chat Rejects Invalid Parameters集成测试 ==="
cargo test rpc_chat_rejects_invalid_params -- --nocapture
echo "✅ RPC Chat Rejects Invalid Parameters集成测试通过"

# 5.1aa RPC Breaker Status and Reset集成测试
echo "=== 步骤5.1aa: 运行RPC Breaker Status and Reset集成测试 ==="
cargo test rpc_breaker_status_and_reset -- --nocapture
echo "✅ RPC Breaker Status and Reset集成测试通过"

# 5.1ab Startup Fails When Cache Vector Paths Unavailable集成测试
echo "=== 步骤5.1ab: 运行Startup Fails When Cache Vector Paths Unavailable集成测试 ==="
cargo test startup_fails_when_cache_vector_paths_are_unavailable -- --nocapture
echo "✅ Startup Fails When Cache Vector Paths Unavailable集成测试通过"

# 5.1ac RPC Legacy Method Aliases Remain Compatible集成测试
echo "=== 步骤5.1ac: 运行RPC Legacy Method Aliases Remain Compatible集成测试 ==="
cargo test rpc_legacy_method_aliases_remain_compatible -- --nocapture
echo "✅ RPC Legacy Method Aliases Remain Compatible集成测试通过"

# 5.1ad RPC Action Vector Maintenance and Trace Metrics集成测试
echo "=== 步骤5.1ad: 运行RPC Action Vector Maintenance and Trace Metrics集成测试 ==="
cargo test rpc_action_vector_maintenance_and_trace_metrics -- --nocapture
echo "✅ RPC Action Vector Maintenance and Trace Metrics集成测试通过"

# 5.1ae RPC Unknown Method and Config Reload集成测试
echo "=== 步骤5.1ae: 运行RPC Unknown Method and Config Reload集成测试 ==="
cargo test rpc_unknown_method_and_config_reload -- --nocapture
echo "✅ RPC Unknown Method and Config Reload集成测试通过"

# 5.1af RPC Task Execute Blocks When Requirement Not Confirmed集成测试
echo "=== 步骤5.1af: 运行RPC Task Execute Blocks When Requirement Not Confirmed集成测试 ==="
cargo test rpc_task_execute_blocks_when_requirement_not_confirmed -- --nocapture
echo "✅ RPC Task Execute Blocks When Requirement Not Confirmed集成测试通过"

# 5.1ag RPC Workflow Execute Review Policy and Learning Feedback集成测试
echo "=== 步骤5.1ag: 运行RPC Workflow Execute Review Policy and Learning Feedback集成测试 ==="
cargo test rpc_workflow_execute_returns_review_policy_and_learning_feedback_fields -- --nocapture
echo "✅ RPC Workflow Execute Review Policy and Learning Feedback集成测试通过"

# 5.1ah RPC Workflow Execute Enforces Dual Review集成测试
echo "=== 步骤5.1ah: 运行RPC Workflow Execute Enforces Dual Review集成测试 ==="
cargo test rpc_workflow_execute_enforces_dual_review_and_returns_decisions -- --nocapture
echo "✅ RPC Workflow Execute Enforces Dual Review集成测试通过"

# 5.1ai RPC Learning Summary Aggregates Clarification Feedback集成测试
echo "=== 步骤5.1ai: 运行RPC Learning Summary Aggregates Clarification Feedback集成测试 ==="
cargo test rpc_learning_summary_aggregates_clarification_feedback_metrics -- --nocapture
echo "✅ RPC Learning Summary Aggregates Clarification Feedback集成测试通过"

# 5.1aj RPC Primary Secondary Policy Artifact Persisted集成测试
echo "=== 步骤5.1aj: 运行RPC Primary Secondary Policy Artifact Persisted集成测试 ==="
cargo test rpc_primary_secondary_policy_artifact_is_persisted_and_response_contains_policy -- --nocapture
echo "✅ RPC Primary Secondary Policy Artifact Persisted集成测试通过"

# 5.1ak RPC Primary Secondary Summary Stability and Failover集成测试
echo "=== 步骤5.1ak: 运行RPC Primary Secondary Summary Stability and Failover集成测试 ==="
cargo test rpc_primary_secondary_summary_reports_stability_and_failover_metrics -- --nocapture
echo "✅ RPC Primary Secondary Summary Stability and Failover集成测试通过"

# 5.1al RPC Workflow Consult Returns Artifact and Consensus集成测试
echo "=== 步骤5.1al: 运行RPC Workflow Consult Returns Artifact and Consensus集成测试 ==="
cargo test rpc_workflow_consult_returns_artifact_and_consensus_signal -- --nocapture
echo "✅ RPC Workflow Consult Returns Artifact and Consensus集成测试通过"

# 5.1am RPC Workflow Research Persists Artifact and Plan集成测试
echo "=== 步骤5.1am: 运行RPC Workflow Research Persists Artifact and Plan集成测试 ==="
cargo test rpc_workflow_research_persists_artifact_and_plan -- --nocapture
echo "✅ RPC Workflow Research Persists Artifact and Plan集成测试通过"

# 5.1an RPC Confirm Requires Ready to Confirm and Clarification Rounds集成测试
echo "=== 步骤5.1an: 运行RPC Confirm Requires Ready to Confirm and Clarification Rounds集成测试 ==="
cargo test rpc_confirm_requires_ready_to_confirm_and_respects_clarification_rounds -- --nocapture
echo "✅ RPC Confirm Requires Ready to Confirm and Clarification Rounds集成测试通过"

# 5.1ao RPC Autotune Reset Restores Default State集成测试
echo "=== 步骤5.1ao: 运行RPC Autotune Reset Restores Default State集成测试 ==="
cargo test rpc_autotune_reset_restores_default_state_and_persists -- --nocapture
echo "✅ RPC Autotune Reset Restores Default State集成测试通过"

# 5.1ap RPC Workflow Execute Auto Consultation Blocks Without Consensus集成测试
echo "=== 步骤5.1ap: 运行RPC Workflow Execute Auto Consultation Blocks Without Consensus集成测试 ==="
cargo test rpc_workflow_execute_auto_consultation_blocks_without_consensus -- --nocapture
echo "✅ RPC Workflow Execute Auto Consultation Blocks Without Consensus集成测试通过"

# 5.1aq ACP Core Unit Suite集成测试
echo "=== 步骤5.1aq: 运行ACP Core Unit Suite集成测试 ==="
cargo test acp::tests::test_suite:: -- --nocapture
echo "✅ ACP Core Unit Suite集成测试通过"

# 6a BLUE14 P0-1 协议模式CLI主链测试
echo "=== 步骤6a: 运行BLUE14协议模式CLI主链测试 ==="
cargo test cli_protocol_mode -- --nocapture
cargo run -- --help | grep -q "protocol-mode"
echo "✅ BLUE14协议模式CLI主链测试通过"

# 6b BLUE14 P0-2 性能监控主链测试
echo "=== 步骤6b: 运行BLUE14性能监控主链测试 ==="
cargo test performance_measure_time_returns_duration -- --nocapture
cargo test http_chat_completions_updates_health_metrics_and_emits_latency_log -- --nocapture
echo "✅ BLUE14性能监控主链测试通过"

# 6c BLUE14 P1-1 错误分类边界主链测试
echo "=== 步骤6c: 运行BLUE14错误分类边界主链测试 ==="
cargo test agent_error_can_be_classified -- --nocapture
echo "✅ BLUE14错误分类边界主链测试通过"

# 6d BLUE14 P1-2 Clippy 静态分析门禁
echo "=== 步骤6d: 运行BLUE14 Clippy静态分析门禁 ==="
run_strict_clippy
echo "✅ BLUE14 Clippy静态分析门禁通过"

# 6e BLUE14 P1-2 Cargo Audit 依赖安全门禁
echo "=== 步骤6e: 运行BLUE14 Cargo Audit依赖安全门禁 ==="
if ! cargo --list | grep -q "^    audit$"; then
    echo "cargo-audit 未安装，正在安装..."
    cargo install cargo-audit --locked
fi
audit_ok=0
for attempt in 1 2 3; do
    if cargo audit; then
        audit_ok=1
        break
    fi
    echo "cargo audit 第 ${attempt} 次失败，准备重试..."
done

if [ "$audit_ok" -eq 0 ]; then
    if [ -d "$HOME/.cargo/advisory-db" ]; then
        echo "cargo audit 在线更新失败，尝试使用本地 advisory cache (--no-fetch --stale)..."
        cargo audit --no-fetch --stale
    else
        echo "❌ cargo audit 在线更新失败且本地 advisory DB 不存在"
        exit 1
    fi
fi
echo "✅ BLUE14 Cargo Audit依赖安全门禁通过"

# 6f BLUE14 P2-2 文档完整性主链测试
echo "=== 步骤6f: 运行BLUE14文档完整性主链测试 ==="
grep -q "BLUE14-P2-2-README-QUICKSTART" README.md
grep -q "BLUE14-P2-2-README-MODES" README.md
grep -q "BLUE14-P2-2-README-CHEATSHEET" README.md
grep -q "BLUE14-P2-2-GUI-QUICKSTART" GUI/README.md
grep -q "BLUE14-P2-2-VSCODE-QUICKSTART" vscode-addon/README.md
echo "✅ BLUE14文档完整性主链测试通过"

# 6g BLUE14 AI1 Token优化与缓存主链测试
echo "=== 步骤6g: 运行BLUE14 AI1 Token优化与缓存主链测试 ==="
cargo test smart_compress_reduces_length_without_losing_system_prompt -- --nocapture
cargo test context_cache_hit_avoids_model_call -- --nocapture
echo "✅ BLUE14 AI1 Token优化与缓存主链测试通过"

# 6h BLUE14 AI2/AI3 学习与强化学习主链测试
echo "=== 步骤6h: 运行BLUE14 AI2/AI3 学习与强化学习主链测试 ==="
cargo test feedback_system_collects_and_persists_event -- --nocapture
cargo test experience_base_finds_similar_success_case -- --nocapture
cargo test q_learning_updates_q_table_on_reward -- --nocapture
cargo test reward_function_positive_for_successful_low_token_task -- --nocapture
cargo test exploration_decays_toward_minimum -- --nocapture
echo "✅ BLUE14 AI2/AI3 学习与强化学习主链测试通过"

# 6i BLUE14 AI4 知识淬炼主链测试
echo "=== 步骤6i: 运行BLUE14 AI4 知识淬炼主链测试 ==="
cargo test distiller_filters_low_confidence_insights -- --nocapture
cargo test deduplicate_removes_high_similarity_entries -- --nocapture
cargo test aggregate_verdict_approve_when_all_signals_pass -- --nocapture
echo "✅ BLUE14 AI4 知识淬炼主链测试通过"

# 6j BLUE14 HD1 策略硬化主链测试
echo "=== 步骤6j: 运行BLUE14 HD1 策略硬化主链测试 ==="
cargo test strict_policy_denies_write_and_shell -- --nocapture
echo "✅ BLUE14 HD1 策略硬化主链测试通过"

# 6k BLUE14 P2-1 用户可见输出i18n主链测试
echo "=== 步骤6k: 运行BLUE14 P2-1 用户可见输出i18n主链测试 ==="
cargo test onboarding_and_status_keys_exist_in_all_languages -- --nocapture
echo "✅ BLUE14 P2-1 用户可见输出i18n主链测试通过"

# 6l BLUE14 HD2 资源配额实时跟踪主链测试
echo "=== 步骤6l: 运行BLUE14 HD2 资源配额实时跟踪主链测试 ==="
cargo test budget_tracker_rejects_on_token_overflow -- --nocapture
cargo test budget_tracker_allows_within_limit_and_reports_remaining -- --nocapture
echo "✅ BLUE14 HD2 资源配额实时跟踪主链测试通过"

# 6m BLUE14 HD3 幂等性请求缓存主链测试
echo "=== 步骤6m: 运行BLUE14 HD3 幂等性请求缓存主链测试 ==="
cargo test idempotency_cache_returns_cached_result_within_ttl -- --nocapture
cargo test idempotency_cache_evicts_expired_entries -- --nocapture
cargo test idempotency_key_is_deterministic -- --nocapture
echo "✅ BLUE14 HD3 幂等性请求缓存主链测试通过"

# 6n BLUE14 HD4 审计日志收集与查询主链测试
echo "=== 步骤6n: 运行BLUE14 HD4 审计日志收集与查询主链测试 ==="
cargo test audit_logger_writes_and_reads_back_entry -- --nocapture
cargo test audit_logger_query_by_path_filters_correctly -- --nocapture
echo "✅ BLUE14 HD4 审计日志收集与查询主链测试通过"

# 6o BLUE14 PUA1 规则引擎接入主链测试
echo "=== 步骤6o: 运行BLUE14 PUA1 规则引擎接入主链测试 ==="
cargo test pua_rule_engine_blocks_red_line_action -- --nocapture
cargo test pua_rule_engine_fails_stage_with_missing_required_action -- --nocapture
cargo test pua_rule_engine_passes_when_all_conditions_met -- --nocapture
echo "✅ BLUE14 PUA1 规则引擎接入主链测试通过"

# 6p BLUE14 PUA2 动态质量指南针主链测试
echo "=== 步骤6p: 运行BLUE14 PUA2 动态质量指南针主链测试 ==="
cargo test compass_adds_security_check_for_security_patch_task -- --nocapture
cargo test compass_base_checks_always_present -- --nocapture
cargo test quality_compass_compat_returns_five_items -- --nocapture
echo "✅ BLUE14 PUA2 动态质量指南针主链测试通过"

# 6q BLUE14 PUA3 学习与反馈系统主链测试
echo "=== 步骤6q: 运行BLUE14 PUA3 学习与反馈系统主链测试 ==="
cargo test pua_collector_writes_report_to_ndjson -- --nocapture
cargo test pua_learning_data_extraction_returns_correct_records -- --nocapture
echo "✅ BLUE14 PUA3 学习与反馈系统主链测试通过"

# 6r BLUE14 PUA4 执行报告生成主链测试
echo "=== 步骤6r: 运行BLUE14 PUA4 执行报告生成主链测试 ==="
cargo test pua_report_status_fail_when_missing_checks_present -- --nocapture
cargo test pua_report_status_pass_when_all_checks_complete -- --nocapture
echo "✅ BLUE14 PUA4 执行报告生成主链测试通过"

# 6s BLUE14 HSS2 RpcHarness 高级能力扩展主链测试
echo "=== 步骤6s: 运行BLUE14 HSS2 RpcHarness 高级能力扩展主链测试 ==="
cargo test concurrent_requests_return_consistent_responses -- --nocapture
cargo test run_scenario_file_executes_runtime_health_requests -- --nocapture
echo "✅ BLUE14 HSS2 RpcHarness 高级能力扩展主链测试通过"

# 6t BLUE14 HSS3 数据驱动集成测试基础设施主链测试
echo "=== 步骤6t: 运行BLUE14 HSS3 数据驱动集成测试基础设施主链测试 ==="
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE14 HSS3 数据驱动集成测试基础设施主链测试通过"

# 6u BLUE14 AGENT1 PUA 字段合约烟雾覆盖主链测试
echo "=== 步骤6u: 运行BLUE14 AGENT1 PUA 字段合约烟雾覆盖主链测试 ==="
cargo test --test pua_contract_smoke -- --nocapture
echo "✅ BLUE14 AGENT1 PUA 字段合约烟雾覆盖主链测试通过"

# 6v BLUE14 AGENT2 学习系统与PUA反馈数据通道对齐主链测试
echo "=== 步骤6v: 运行BLUE14 AGENT2 学习系统与PUA反馈数据通道对齐主链测试 ==="
cargo test learning_record_roundtrip_supports_workflow_and_pua -- --nocapture
cargo test analyze_patterns_reads_mixed_workflow_and_pua_records -- --nocapture
cargo test pua_learning_data_extraction_returns_correct_records -- --nocapture
echo "✅ BLUE14 AGENT2 学习系统与PUA反馈数据通道对齐主链测试通过"

# 6w BLUE14 AGENT3 BudgetTracker 与 PUA escalation 联动主链测试
echo "=== 步骤6w: 运行BLUE14 AGENT3 BudgetTracker 与 PUA escalation 联动主链测试 ==="
cargo test budget_tracker_token_overflow_escalates_pua_level -- --nocapture
cargo test budget_tracker_token_overflow_escalation_capped_at_l5 -- --nocapture
echo "✅ BLUE14 AGENT3 BudgetTracker 与 PUA escalation 联动主链测试通过"

# 6x BLUE14 AGENT4 tf!/anyhow 错误统一溯源主链测试
echo "=== 步骤6x: 运行BLUE14 AGENT4 tf!/anyhow 错误统一溯源主链测试 ==="
cargo test classify_request_error_kind_detects_pua_violation -- --nocapture
cargo test classify_request_error_kind_detects_budget_exceeded -- --nocapture
cargo test classify_request_error_kind_detects_sandbox_blocked -- --nocapture
echo "✅ BLUE14 AGENT4 tf!/anyhow 错误统一溯源主链测试通过"

# 6y BLUE15 P1-4 并发锁模型与毒化恢复主链测试
echo "=== 步骤6y: 运行BLUE15 P1-4 并发锁模型与毒化恢复主链测试 ==="
cargo test acp_lock_monitor_recovers_poisoned_mutex_and_records_stats -- --nocapture
cargo test summarize_lock_health_marks_poisoned_components_warn -- --nocapture
echo "✅ BLUE15 P1-4 并发锁模型与毒化恢复主链测试通过"

# 6z BLUE15 P2-3 超时模型统一与线程开销收敛主链测试
echo "=== 步骤6z: 运行BLUE15 P2-3 超时模型统一与线程开销收敛主链测试 ==="
cargo test run_with_optional_timeout_returns_timeout_error -- --nocapture
cargo test probe_agent_runtime_readiness_accepts_async_local_listener -- --nocapture
cargo test runtime_metrics_tracks_agent_and_probe_timeouts -- --nocapture
echo "✅ BLUE15 P2-3 超时模型统一与线程开销收敛主链测试通过"

# 6aa BLUE15 P3-1 覆盖率与基准测试体系优化主链测试
echo "=== 步骤6aa: 运行BLUE15 P3-1 覆盖率与基准测试体系优化主链测试 ==="
cargo test run_scenario_file_executes_quality_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
if cargo --list | grep -q "^    tarpaulin$"; then
    cargo tarpaulin --out Stdout --fail-under 70
else
    echo "cargo-tarpaulin 未安装，跳过可选覆盖率门禁"
fi
echo "✅ BLUE15 P3-1 覆盖率与基准测试体系优化主链测试通过"

# 6ab BLUE15 P1-1 模型选择增强（探索-利用平衡）主链测试
echo "=== 步骤6ab: 运行BLUE15 P1-1 模型选择增强（探索-利用平衡）主链测试 ==="
cargo test test_rank_candidates_uses_model_level_ucb_scores -- --nocapture
cargo test test_snapshot_contains_sorted_ucb_scores -- --nocapture
cargo test run_scenario_file_executes_model_selector_benchmark_requests -- --nocapture
echo "✅ BLUE15 P1-1 模型选择增强（探索-利用平衡）主链测试通过"

# 6ac BLUE15 P1-2 学习数据持久化与回放主链测试
echo "=== 步骤6ac: 运行BLUE15 P1-2 学习数据持久化与回放主链测试 ==="
cargo test run_scenario_file_executes_learning_replay_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 P1-2 学习数据持久化与回放主链测试通过"

# 6ad BLUE15 P1-3 PUA 动态规则与审计可视化主链测试
echo "=== 步骤6ad: 运行BLUE15 P1-3 PUA 动态规则与审计可视化主链测试 ==="
cargo test run_scenario_file_executes_governance_dynamic_rules_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 P1-3 PUA 动态规则与审计可视化主链测试通过"

# 6ae BLUE15 P2-1 故障恢复与降级（断路器）主链测试
echo "=== 步骤6ae: 运行BLUE15 P2-1 故障恢复与降级（断路器）主链测试 ==="
cargo test optimization::failure_prevention::tests::test_recover_resets_unhealthy_service_to_healthy -- --nocapture
cargo test run_scenario_file_executes_breaker_recovery_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 P2-1 故障恢复与降级（断路器）主链测试通过"

# 6af BLUE15 P2-2 可观测性完善（追踪覆盖率与告警）主链测试
echo "=== 步骤6af: 运行BLUE15 P2-2 可观测性完善（追踪覆盖率与告警）主链测试 ==="
cargo test run_scenario_file_executes_observability_alerts_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 P2-2 可观测性完善（追踪覆盖率与告警）主链测试通过"

# 6af2 BLUE15 P0-2 运行稳定性基线（健康检查、优雅停机、配置校验）主链测试
echo "=== 步骤6af2: 运行BLUE15 P0-2 运行稳定性基线（健康检查、优雅停机、配置校验）主链测试 ==="
cargo test run_scenario_file_executes_runtime_stability_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 P0-2 运行稳定性基线（健康检查、优雅停机、配置校验）主链测试通过"

# 6ag BLUE15 P0-1 生产化安全基线（鉴权、限流、反向代理、TLS）主链测试
echo "=== 步骤6ag: 运行BLUE15 P0-1 生产化安全基线（鉴权、限流、反向代理、TLS）主链测试 ==="
cargo test run_scenario_file_executes_security_baseline_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 P0-1 生产化安全基线（鉴权、限流、反向代理、TLS）主链测试通过"

# 6ah BLUE15 X5 Harness 评测基座与回归挑战集主链测试
echo "=== 步骤6ah: 运行BLUE15 X5 Harness 评测基座与回归挑战集主链测试 ==="
cargo test run_scenario_file_executes_harness_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 X5 Harness 评测基座与回归挑战集主链测试通过"

# 6ai BLUE15 X1 自学习闭环质量门禁主链测试
echo "=== 步骤6ai: 运行BLUE15 X1 自学习闭环质量门禁主链测试 ==="
cargo test run_scenario_file_executes_learning_loop_guardrail_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 X1 自学习闭环质量门禁主链测试通过"

# 6aj BLUE15 X2 知识萃取分层与去噪主链测试
echo "=== 步骤6aj: 运行BLUE15 X2 知识萃取分层与去噪主链测试 ==="
cargo test run_scenario_file_executes_knowledge_distillation_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 X2 知识萃取分层与去噪主链测试通过"

# 6ak BLUE15 X3 强化学习奖励对齐与离线评估主链测试
echo "=== 步骤6ak: 运行BLUE15 X3 强化学习奖励对齐与离线评估主链测试 ==="
cargo test run_scenario_file_executes_rl_alignment_offline_eval_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 X3 强化学习奖励对齐与离线评估主链测试通过"

# 6al BLUE15 X4 Hardness 分级路由与预算编排主链测试
echo "=== 步骤6al: 运行BLUE15 X4 Hardness 分级路由与预算编排主链测试 ==="
cargo test run_scenario_file_executes_hardness_routing_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 X4 Hardness 分级路由与预算编排主链测试通过"

# 6am BLUE15 X6 Token 成本压缩与 Cost 治理主链测试
echo "=== 步骤6am: 运行BLUE15 X6 Token 成本压缩与 Cost 治理主链测试 ==="
cargo test run_scenario_file_executes_token_cost_governance_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 X6 Token 成本压缩与 Cost 治理主链测试通过"

# 6an BLUE15 X7 配置基线收敛与一次性清理主链测试
echo "=== 步骤6an: 运行BLUE15 X7 配置基线收敛与一次性清理主链测试 ==="
cargo test run_scenario_file_executes_config_baseline_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 X7 配置基线收敛与一次性清理主链测试通过"

# 6ao BLUE15 X8 错误码、重试语义与客户端契约统一主链测试
echo "=== 步骤6ao: 运行BLUE15 X8 错误码、重试语义与客户端契约统一主链测试 ==="
cargo test run_scenario_file_executes_error_contract_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 X8 错误码、重试语义与客户端契约统一主链测试通过"

# 6ap BLUE15 X9 依赖与构建可复现优化主链测试
echo "=== 步骤6ap: 运行BLUE15 X9 依赖与构建可复现优化主链测试 ==="
cargo test run_scenario_file_executes_build_repro_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 X9 依赖与构建可复现优化主链测试通过"

# 6aq BLUE15 X10 数据生命周期与存储治理主链测试
echo "=== 步骤6aq: 运行BLUE15 X10 数据生命周期与存储治理主链测试 ==="
cargo test run_scenario_file_executes_data_lifecycle_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 X10 数据生命周期与存储治理主链测试通过"

# 6ar BLUE15 X11 总体一次优化到顶主链测试
echo "=== 步骤6ar: 运行BLUE15 X11 总体一次优化到顶主链测试 ==="
cargo test run_scenario_file_executes_optimization_peak_benchmark_requests -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 X11 总体一次优化到顶主链测试通过"

# 6as BLUE15 Stage C 上线收口演练主链测试
echo "=== 步骤6as: 运行BLUE15 Stage C 上线收口演练主链测试 ==="
cargo test run_scenario_file_executes_release_readiness_drill_requests -- --nocapture
cargo test rpc_shutdown_waits_for_inflight_chat_completion -- --nocapture
cargo test ndjson_scenario_files_all_pass -- --nocapture
echo "✅ BLUE15 Stage C 上线收口演练主链测试通过"

# 5.2 GUI 契约烟测
echo "=== 步骤5.2: 运行GUI契约烟测 ==="
cd GUI
npm test
cd ..
echo "✅ GUI契约烟测通过"

# 5.3 VS Code Addon 编译 + 契约烟测
echo "=== 步骤5.3: 运行VS Code Addon编译 + 契约烟测 ==="
cd vscode-addon
npm test
cd ..
echo "✅ VS Code Addon 编译 + 契约烟测通过"

# 6. 验证语言文件
echo "=== 步骤6: 验证语言文件 ==="
if [ -f "languages/en_US.json" ]; then
    echo "✅ 英文语言文件存在"
    EN_KEY_COUNT=$(jq '.messages | length' languages/en_US.json 2>/dev/null || echo "0")
    echo "英文语言文件包含 $EN_KEY_COUNT 个翻译键"
else
    echo "❌ 英文语言文件不存在"
    exit 1
fi

if [ -f "languages/zh_CN.json" ]; then
    echo "✅ 中文语言文件存在"
    ZH_KEY_COUNT=$(jq '.messages | length' languages/zh_CN.json 2>/dev/null || echo "0")
    echo "中文语言文件包含 $ZH_KEY_COUNT 个翻译键"
else
    echo "❌ 中文语言文件不存在"
    exit 1
fi

# 7. 检查关键翻译键是否存在
echo "=== 步骤7: 检查关键翻译键 ==="
REQUIRED_KEYS=(
    "app.name"
    "app.description"
    "error.fatal"
    "error.handling_request"
    "info.resources_reloaded"
    "ui.legacy_health_score"
)

for key in "${REQUIRED_KEYS[@]}"; do
    if jq -e ".messages.\"$key\"" languages/en_US.json > /dev/null 2>&1; then
        echo "✅ 键 '$key' 存在于英文语言文件"
    else
        echo "❌ 键 '$key' 不存在于英文语言文件"
        exit 1
    fi
done

echo "=== CI测试完成 ==="
echo "✅ 所有CI步骤都通过了！"
echo ""
echo "总结:"
echo "1. 项目构建成功"
echo "2. 所有测试通过"
echo "3. 代码检查通过 (clippy 严格模式)"
echo "4. 代码格式化正确"
echo "5. i18n系统工作正常"
echo "6. 语言文件完整"
echo "7. 关键翻译键都存在"
echo "8. 文档关键章节完整"
echo ""
echo "GitHub Actions CI流程应该能成功运行。"
