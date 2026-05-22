use crate::agent::Message;
use crate::intelligence::token_cache::TokenMultiLevelCache;
use std::sync::Arc;

pub(crate) fn should_bypass_for_execution(mode: &str, messages: &[Message]) -> bool {
    crate::acp::helpers::autonomy::is_execution_like_request(mode, messages)
}

pub(crate) fn should_serve_cache_hit(confidence: f32, bypass_for_execution: bool) -> bool {
    confidence > 0.95 && !bypass_for_execution
}

pub(crate) fn should_refuse_cache_hit(confidence: f32, bypass_for_execution: bool) -> bool {
    confidence > 0.95 && bypass_for_execution
}

pub(crate) fn store_async(
    cache: Arc<TokenMultiLevelCache>,
    input_text: String,
    output_text: String,
    token_count: usize,
    agent_name: Option<String>,
    model_name: Option<String>,
) {
    tokio::spawn(async move {
        cache
            .store(
                &input_text,
                &output_text,
                token_count,
                agent_name,
                model_name,
            )
            .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Message;
    use tempfile::NamedTempFile;

    #[test]
    fn cache_hit_served_only_when_not_bypassed() {
        assert!(should_serve_cache_hit(0.99, false));
        assert!(!should_serve_cache_hit(0.80, false));
        assert!(!should_serve_cache_hit(0.99, true));
    }

    #[test]
    fn cache_hit_refused_when_execution_like() {
        assert!(should_refuse_cache_hit(0.99, true));
        assert!(!should_refuse_cache_hit(0.99, false));
    }

    #[test]
    fn execution_like_bypass_uses_autonomy_classifier() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: "please apply patch to src/main.rs".to_string(),
        }];
        assert!(should_bypass_for_execution("execute", &messages));
    }

    #[test]
    fn store_async_is_non_blocking_entrypoint() {
        // This test verifies the API shape and that calling the helper does not panic.
        // Runtime behavior is covered by integration tests around token cache use.
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let file = NamedTempFile::new().expect("tempfile");
            let path = file.path().to_string_lossy().to_string();
            let cache = Arc::new(TokenMultiLevelCache::new(16, 16, &path));
            store_async(
                cache,
                "in".to_string(),
                "out".to_string(),
                8,
                Some("agent-a".to_string()),
                Some("model-a".to_string()),
            );
            tokio::task::yield_now().await;
        });
    }
}
