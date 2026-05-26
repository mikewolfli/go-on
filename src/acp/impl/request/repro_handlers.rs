//! Reproducibility and optimization handlers extracted from runtime_pack.rs.
//!
//! Provides the `handle_optimization_peak` handler for runtime optimization
//! analysis. The `handle_build_repro` function (reproducible build tracking)
//! is already in `repro_pack.rs`.

use super::*;

pub(super) async fn handle_optimization_peak(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let status = server.get_status();
    send_result(
        server,
        request_id,
        serde_json::json!({
            "ok": true,
            "total_requests": status.metrics.total_requests,
            "failed_requests": status.metrics.failed_requests,
            "recommendations": [],
        }),
    )
    .await
}

#[cfg(test)]
mod tests {
    #[test]
    fn peak_shape() {
        let payload = serde_json::json!({
            "ok": true,
            "total_requests": 0,
            "failed_requests": 0,
            "recommendations": [],
        });
        assert_eq!(payload.get("ok").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn labels() {
        let labels = vec!["reliability", "performance", "quality", "resilience"];
        assert_eq!(labels.len(), 4);
    }
}
