use std::sync::Arc;

use super::AppConfig;
use crate::acp::server::AcpServer;

/// Filter out unavailable agents from the candidate list.
/// An agent is considered unavailable if its health check fails.
pub(crate) async fn filter_unavailable_agents(
    server: &AcpServer,
    _app_config: &AppConfig,
    candidates: &mut Vec<(String, Arc<dyn crate::agent::Agent>)>,
) -> Vec<String> {
    let mut unavailable = Vec::new();
    let mut available = Vec::new();

    for (name, agent) in candidates.drain(..) {
        let is_available = match server.model_deps.agent_registry.as_ref() {
            Some(registry) => registry.get(&name).is_some(),
            None => true,
        };

        if is_available {
            available.push((name, agent));
        } else {
            unavailable.push(name);
        }
    }

    *candidates = available;
    unavailable
}
