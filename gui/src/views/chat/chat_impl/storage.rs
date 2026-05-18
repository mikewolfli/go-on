use super::*;

impl ChatView {
    const SAVE_DEBOUNCE_MS: u64 = 120;

    async fn wait_for_save_slot(in_flight: &std::sync::atomic::AtomicBool) -> bool {
        const MAX_RETRIES: usize = 80;
        for _ in 0..MAX_RETRIES {
            if in_flight
                .compare_exchange(
                    false,
                    true,
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                )
                .is_ok()
            {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        false
    }

    /// Generic async save function with debounce, epoch-based dedup, and save-slot locking.
    /// Used by both `save_sessions_to_disk` and `save_templates_to_disk` to avoid code duplication.
    fn save_to_disk(
        in_flight: std::sync::Arc<std::sync::atomic::AtomicBool>,
        epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
        this_epoch: u64,
        path: std::path::PathBuf,
        label: &'static str,
        json_payload: String,
        pending_tx: mpsc::SyncSender<PendingResponse>,
    ) {
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(ChatView::SAVE_DEBOUNCE_MS)).await;
            if epoch.load(std::sync::atomic::Ordering::Acquire) != this_epoch {
                return;
            }

            if !ChatView::wait_for_save_slot(&in_flight).await {
                let _ = pending_tx.try_send(PendingResponse::UiMessage(format!(
                    "Failed to acquire save slot for {}: {}",
                    label,
                    path.display()
                )));
                return;
            }

            if epoch.load(std::sync::atomic::Ordering::Acquire) != this_epoch {
                in_flight.store(false, std::sync::atomic::Ordering::Relaxed);
                return;
            }

            if let Some(parent) = path.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    let _ = pending_tx.try_send(PendingResponse::UiMessage(format!(
                        "Failed to create {} directory {}: {e}",
                        label,
                        parent.display()
                    )));
                    in_flight.store(false, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
            }
            // Atomic write: .tmp then rename to prevent corruption
            let tmp_path = path.with_extension("tmp");
            if let Err(e) = tokio::fs::write(&tmp_path, &json_payload).await {
                let _ = pending_tx.try_send(PendingResponse::UiMessage(format!(
                    "Failed to write {} tmp: {e}",
                    label
                )));
                in_flight.store(false, std::sync::atomic::Ordering::Relaxed);
                return;
            }
            if let Err(e) = tokio::fs::rename(&tmp_path, &path).await {
                let _ = pending_tx.try_send(PendingResponse::UiMessage(format!(
                    "Failed to rename {} tmp: {e}",
                    label
                )));
                in_flight.store(false, std::sync::atomic::Ordering::Relaxed);
                return;
            }
            in_flight.store(false, std::sync::atomic::Ordering::Relaxed);
        });
    }

    pub(super) fn templates_path() -> PathBuf {
        crate::fs_util::project_config_dir()
            .map(|p| p.join("chat_prompt_templates.json"))
            .unwrap_or_else(|| PathBuf::from("chat_prompt_templates.json"))
    }

    pub(super) fn load_templates_from_disk() -> Vec<PromptTemplate> {
        let path = Self::templates_path();
        crate::fs_util::load_json_with_backup(&path, "chat templates")
    }

    pub(super) fn save_templates_to_disk(&self) {
        let in_flight = self.template_save_in_flight.clone();
        let epoch = self.template_save_epoch.clone();
        let this_epoch = epoch.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
        let templates = self.prompt_templates.clone();
        let path = Self::templates_path();
        let json_payload = match serde_json::to_string_pretty(&templates) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to serialize templates: {e}; save skipped");
                return;
            }
        };
        Self::save_to_disk(
            in_flight,
            epoch,
            this_epoch,
            path,
            "chat templates",
            json_payload,
            self.pending_tx.clone(),
        );
    }

    pub(super) fn sessions_path() -> PathBuf {
        crate::fs_util::project_config_dir()
            .map(|p| p.join("chat_sessions.json"))
            .unwrap_or_else(|| PathBuf::from("chat_sessions.json"))
    }

    pub(super) fn load_sessions_from_disk() -> Vec<Session> {
        let path = Self::sessions_path();
        let mut sessions: Vec<Session> =
            crate::fs_util::load_json_with_backup(&path, "chat sessions");
        // Enforce MAX_MESSAGES cap on each session loaded from disk
        for session in sessions.iter_mut() {
            if session.messages.len() > crate::views::chat::types::MAX_MESSAGES {
                let excess = session.messages.len() - crate::views::chat::types::MAX_MESSAGES;
                session.messages.drain(0..excess);
            }
        }
        sessions
    }

    pub(super) fn save_sessions_to_disk(&self) {
        let in_flight = self.session_save_in_flight.clone();
        let epoch = self.session_save_epoch.clone();
        let this_epoch = epoch.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
        let sessions = self.sessions.clone();
        let path = Self::sessions_path();
        let json_payload = match serde_json::to_string_pretty(&sessions) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to serialize sessions: {e}; save skipped");
                return;
            }
        };
        Self::save_to_disk(
            in_flight,
            epoch,
            this_epoch,
            path,
            "chat sessions",
            json_payload,
            self.pending_tx.clone(),
        );
    }
}
