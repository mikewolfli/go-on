// Request dispatch for the runtime method family.
//
// Extracted from the single dispatch table in `request/mod.rs` (module split
// M0.4): each family matches the normalized method name exactly as the
// original `handle_request` match did, so dispatch behavior is unchanged.
// Runtime-family dispatch: chat/phase, metrics, health/lifecycle,
// observability, harness/breaker/cache maintenance, capabilities, models,
// and runtime restart.
use anyhow::Result;
use serde_json::Value;

use crate::acp::server::AcpServer;
use crate::rpc_protocol::{JsonRpcRequest, RequestTraceContext};

use super::config_handlers;
use super::diagnostic_pack::{lock_status_payload, observability_alerts_payload};
use super::dispatch_to_client;
use super::health_pack::{
    breaker_recovery_payload, breaker_reset_payload, breaker_status_payload, cache_clear_payload,
    maintenance_gc_payload, vector_clear_payload,
};
use super::lifecycle_handlers;
use super::lifecycle_pack::data_lifecycle_payload;
use super::metrics_pack;
use super::protocol_pack;
use super::runtime_pack;
use super::status_pack::{
    harness_status_payload, release_readiness_payload, security_baseline_payload,
};
use super::trace_pack::trace_metrics_snapshot;

pub(crate) async fn dispatch_runtime(
    server: &AcpServer,
    request: JsonRpcRequest,
    request_id: Option<Value>,
    _http_headers: Option<&str>,
    trace: &RequestTraceContext,
    method: &str,
) -> Result<()> {
    match method {
        "chat" => {
            dispatch_to_client(
                server,
                request_id.clone(),
                protocol_pack::handle_chat(
                    server,
                    request_id,
                    request.params.unwrap_or_default(),
                    trace,
                )
                .await,
            )
            .await
        }
        "phase" | "phase.status" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::phase_payload(server, request.params.unwrap_or_default(), trace)
                    .await,
            )
            .await
        }
        "metrics.get" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                metrics_pack::metrics_get_payload(server).await,
            )
            .await
        }
        "metrics" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                metrics_pack::metrics_payload(server).await,
            )
            .await
        }
        "metrics.prometheus" => {
            dispatch_to_client(
                server,
                request_id,
                metrics_pack::handle_metrics_prometheus(server).await,
            )
            .await
        }
        "metrics.window.query" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                Ok(runtime_pack::metrics_window_query_payload(
                    server,
                    &request.params.unwrap_or_default(),
                )),
            )
            .await
        }
        "metrics.errors.summary" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                Ok(runtime_pack::metrics_errors_summary_payload(
                    server,
                    &request.params.unwrap_or_default(),
                )),
            )
            .await
        }
        "metrics.reset" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                metrics_pack::metrics_reset_payload(server).await,
            )
            .await
        }
        "debug_panel.get" | "debug.panel.get" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                config_handlers::debug_panel_payload(server).await,
            )
            .await
        }
        "trace.get" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                config_handlers::trace_payload_result(&request.params.unwrap_or_default()),
            )
            .await
        }
        "trace.metrics" => {
            crate::acp::r#impl::io::respond(server, request_id, Ok(trace_metrics_snapshot(server)))
                .await
        }
        "shutdown" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                lifecycle_handlers::shutdown_payload(server),
            )
            .await
        }
        "health" | "runtime.health" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                lifecycle_handlers::health_payload(server).await,
            )
            .await
        }
        "health.probes" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                lifecycle_handlers::build_health_probes_payload(server).await,
            )
            .await
        }
        "lock.status" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                lock_status_payload(server, request.params.unwrap_or_default()).await,
            )
            .await
        }
        "runtime.self_model" => {
            let params = request.params.unwrap_or_default();
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                lifecycle_handlers::build_runtime_self_model_payload(server, &params).await,
            )
            .await
        }
        "provider.status" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                lifecycle_handlers::build_provider_status_payload(server).await,
            )
            .await
        }
        "release.readiness" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                release_readiness_payload(server, request.params.unwrap_or_default()).await,
            )
            .await
        }
        "runtime.stability" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                lifecycle_handlers::build_runtime_stability_payload(server).await,
            )
            .await
        }
        "runtime.features" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                lifecycle_handlers::runtime_features_payload(server),
            )
            .await
        }
        "observability.alerts" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                observability_alerts_payload(server, request.params.unwrap_or_default()).await,
            )
            .await
        }
        "security.baseline" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                security_baseline_payload(server, request.params.unwrap_or_default()).await,
            )
            .await
        }
        "harness.status" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                harness_status_payload(server, request.params.unwrap_or_default()).await,
            )
            .await
        }
        "breaker.status" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                breaker_status_payload(server).await,
            )
            .await
        }
        "breaker.reset" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                breaker_reset_payload(server, request.params.unwrap_or_default()).await,
            )
            .await
        }
        "breaker.recovery" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                breaker_recovery_payload(server, request.params.unwrap_or_default()).await,
            )
            .await
        }
        "cache.clear" => {
            crate::acp::r#impl::io::respond(server, request_id, cache_clear_payload(server).await)
                .await
        }
        "vector.clear" => {
            crate::acp::r#impl::io::respond(server, request_id, vector_clear_payload(server).await)
                .await
        }
        "maintenance.gc" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                maintenance_gc_payload(server).await,
            )
            .await
        }
        "data.lifecycle" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                data_lifecycle_payload(server, request.params.unwrap_or_default()).await,
            )
            .await
        }
        "action.check" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::action_check_payload(server, request.params.unwrap_or_default()),
            )
            .await
        }
        "approval.list" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::approval_list_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }

        "capabilities.list" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                lifecycle_handlers::capabilities_list_payload(server).await,
            )
            .await
        }
        "models.list" | "models/list" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::models_list_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }

        "runtime.restart" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                lifecycle_handlers::runtime_restart_payload(server),
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
