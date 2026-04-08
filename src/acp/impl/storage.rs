impl AcpServer {
    async fn cache_get(
        &self,
        cache: Arc<ResponseCache>,
        cache_key: String,
    ) -> Result<Option<crate::cache::CachedResponse>> {
        spawn_blocking(move || cache.get(&cache_key))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "cache_get"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn cache_put(
        &self,
        cache: Arc<ResponseCache>,
        cache_key: String,
        response_text: String,
        agent_name: String,
        ttl: Option<u64>,
    ) -> Result<()> {
        spawn_blocking(move || cache.put(&cache_key, &response_text, &agent_name, ttl))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "cache_put"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn cache_entry_count(&self, cache: Arc<ResponseCache>) -> Result<u64> {
        spawn_blocking(move || cache.entry_count())
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "cache_entry_count"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn cache_clear(&self, cache: Arc<ResponseCache>) -> Result<usize> {
        spawn_blocking(move || cache.clear_all())
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "cache_clear"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn vector_search(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
        query: String,
        top_k: usize,
        min_similarity: f32,
        max_snippet_chars: usize,
    ) -> Result<(Vec<VectorHit>, crate::vector::VectorPrecisionFeedback)> {
        spawn_blocking(move || {
            vector_store.search(&phase, &query, top_k, min_similarity, max_snippet_chars)
        })
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf(
                    "error.task_join",
                    &[("task", "vector_search"), ("error", &format!("{}", e))]
                )
            )
        })?
    }

    async fn vector_get_phase_summary(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
    ) -> Result<Option<String>> {
        spawn_blocking(move || vector_store.get_phase_summary(&phase))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[
                            ("task", "vector_get_phase_summary"),
                            ("error", &format!("{}", e))
                        ]
                    )
                )
            })?
    }

    async fn vector_upsert(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
        query: String,
        response_text: String,
    ) -> Result<()> {
        spawn_blocking(move || vector_store.upsert(&phase, &query, &response_text))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "vector_upsert"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn vector_entry_counts(&self, vector_store: Arc<VectorStore>) -> Result<(u64, u64)> {
        spawn_blocking(move || {
            let memory = vector_store.memory_entry_count()?;
            let summaries = vector_store.summary_entry_count()?;
            Ok::<(u64, u64), anyhow::Error>((memory, summaries))
        })
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf(
                    "error.task_join",
                    &[
                        ("task", "vector_entry_counts"),
                        ("error", &format!("{}", e))
                    ]
                )
            )
        })?
    }

    async fn vector_clear(&self, vector_store: Arc<VectorStore>) -> Result<(usize, usize)> {
        spawn_blocking(move || vector_store.clear_all())
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "vector_clear"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn vector_upsert_phase_summary(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
        summary: String,
    ) -> Result<()> {
        spawn_blocking(move || vector_store.upsert_phase_summary(&phase, &summary))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[
                            ("task", "vector_upsert_phase_summary"),
                            ("error", &format!("{}", e))
                        ]
                    )
                )
            })?
    }

}
