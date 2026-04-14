#!/bin/bash

# CI测试脚本
# 这个脚本模拟GitHub Actions CI流程中的所有步骤

set -e  # 任何命令失败则退出
echo "=== 开始CI测试 ==="

# 1. 构建项目
echo "=== 步骤1: 构建项目 ==="
cargo build --verbose
echo "✅ 构建成功"

# 2. 运行测试
echo "=== 步骤2: 运行测试 ==="
cargo test --all --verbose
echo "✅ 测试通过"

# 3. 代码检查 (允许未使用代码警告)
echo "=== 步骤3: 代码检查 ==="
cargo clippy --all -- -D warnings -A dead_code
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
echo "3. 代码检查通过 (已忽略未使用代码警告)"
echo "4. 代码格式化正确"
echo "5. i18n系统工作正常"
echo "6. 语言文件完整"
echo "7. 关键翻译键都存在"
echo ""
echo "GitHub Actions CI流程应该能成功运行。"
