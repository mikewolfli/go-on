// Request dispatch for the learning method family.
//
// Extracted from the single dispatch table in `request/mod.rs` (module split
// M0.4): each family matches the normalized method name exactly as the
// original `handle_request` match did, so dispatch behavior is unchanged.
// Learning-family dispatch: learning/knowledge/alignment methods,
// governance handlers, and health checks.
use anyhow::Result;
use serde_json::Value;

use crate::acp::server::AcpServer;
use crate::rpc_protocol::{JsonRpcRequest, RequestTraceContext};

use super::governance_handlers;
use super::learning_pack::{self, learning_summary_payload};
use crate::acp::background::run_health_check;
use serde_json::json;

pub(crate) async fn dispatch_learning(
    server: &AcpServer,
    request: JsonRpcRequest,
    request_id: Option<Value>,
    _http_headers: Option<&str>,
    _trace: &RequestTraceContext,
    method: &str,
) -> Result<()> {
    match method {
        "learning.summary" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                learning_summary_payload(server, request.params.unwrap_or_default()).await,
            )
            .await
        }
        "learning.replay" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                learning_pack::learning_replay_payload(server, request.params.unwrap_or_default()),
            )
            .await
        }
        "learning.guardrail" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                learning_pack::learning_guardrail_payload(
                    server,
                    request.params.unwrap_or_default(),
                ),
            )
            .await
        }
        "knowledge.distill" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                learning_pack::knowledge_distill_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "rl.alignment.offline_eval" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                learning_pack::rl_alignment_offline_eval_payload(
                    server,
                    request.params.unwrap_or_default(),
                ),
            )
            .await
        }
        "governance.status" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                governance_handlers::governance_status_payload(
                    server,
                    request.params.unwrap_or_default(),
                ),
            )
            .await
        }

        "governance.plan.get" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                governance_handlers::governance_plan_get_payload(server),
            )
            .await
        }
        "governance.plan.update" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                governance_handlers::governance_plan_update_payload(
                    server,
                    request.params.unwrap_or_default(),
                ),
            )
            .await
        }
        "governance.audit.recent" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                governance_handlers::governance_audit_recent_payload(
                    server,
                    request.params.unwrap_or_default(),
                ),
            )
            .await
        }
        "governance.audit.verify" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                governance_handlers::governance_audit_verify_payload(
                    server,
                    request.params.unwrap_or_default(),
                ),
            )
            .await
        }
        "phase.policy.replay" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                learning_pack::phase_policy_replay_payload(
                    server,
                    request.params.unwrap_or_default(),
                ),
            )
            .await
        }
        "primary_secondary.summary" | "summary/primary_secondary" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                learning_pack::primary_secondary_summary_payload(
                    server,
                    request.params.unwrap_or_default(),
                ),
            )
            .await
        }
        "health.check" => match run_health_check(server).await {
            Ok(_) => {
                crate::acp::r#impl::io::respond(server, request_id, Ok(json!({ "ok": true }))).await
            }
            Err(e) => {
                tracing::warn!("health.check: run_health_check failed: {}", e);
                crate::acp::r#impl::io::respond(
                    server,
                    request_id,
                    Ok(json!({ "ok": false, "error": e.to_string() })),
                )
                .await
            }
        },
        "governance.remediate" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                governance_handlers::governance_remediate_payload(
                    server,
                    request.params.unwrap_or_default(),
                ),
            )
            .await
        }
        "governance.config.save" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                governance_handlers::governance_config_save_payload(
                    server,
                    request.params.unwrap_or_default(),
                ),
            )
            .await
        }
        _ => {
            // Unreachable: handle_request routes only methods belonging to this
            // family to this dispatcher; the MethodNotFound fallback stays in
            // the parent module's dispatch table.
            Ok(())
        }
    }
}
