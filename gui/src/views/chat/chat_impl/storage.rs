use super::*;

impl ChatView {
    pub(super) fn templates_path() -> PathBuf {
        if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
            dirs.config_dir().join("chat_prompt_templates.json")
        } else {
            PathBuf::from("chat_prompt_templates.json")
        }
    }

    pub(super) fn load_templates_from_disk() -> Vec<PromptTemplate> {
        let path = Self::templates_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                serde_json::from_str::<Vec<PromptTemplate>>(&content).unwrap_or_default()
            }
            Err(_) => Vec::new(),
        }
    }

    pub(super) fn save_templates_to_disk(&self) {
        let in_flight = self.template_save_in_flight.clone();
        if in_flight.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        in_flight.store(true, std::sync::atomic::Ordering::Relaxed);
        let templates = self.prompt_templates.clone();
        let path = Self::templates_path();
        tokio::spawn(async move {
            if let Some(parent) = path.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    eprintln!(
                        "Failed to create chat template directory {}: {e}",
                        parent.display()
                    );
                    in_flight.store(false, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
            }
            match serde_json::to_string_pretty(&templates) {
                Ok(content) => {
                    if let Err(e) = tokio::fs::write(&path, content).await {
                        eprintln!("Failed to write chat templates to {}: {e}", path.display());
                    }
                }
                Err(e) => {
                    eprintln!("Failed to serialize chat templates: {e}");
                }
            }
            in_flight.store(false, std::sync::atomic::Ordering::Relaxed);
        });
    }

    pub(super) fn sessions_path() -> PathBuf {
        if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
            dirs.config_dir().join("chat_sessions.json")
        } else {
            PathBuf::from("chat_sessions.json")
        }
    }

    pub(super) fn load_sessions_from_disk() -> Vec<Session> {
        let path = Self::sessions_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str::<Vec<Session>>(&content).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    pub(super) fn save_sessions_to_disk(&self) {
        let in_flight = self.session_save_in_flight.clone();
        if in_flight.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        in_flight.store(true, std::sync::atomic::Ordering::Relaxed);
        // Clone data for the background task to avoid blocking the UI thread.
        let sessions = self.sessions.clone();
        let path = Self::sessions_path();
        tokio::spawn(async move {
            if let Some(parent) = path.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    eprintln!(
                        "Failed to create chat session directory {}: {e}",
                        parent.display()
                    );
                    in_flight.store(false, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
            }
            // Serialize on the async task (off UI thread).
            match serde_json::to_string_pretty(&sessions) {
                Ok(content) => {
                    if let Err(e) = tokio::fs::write(&path, content).await {
                        eprintln!("Failed to write chat sessions to {}: {e}", path.display());
                    }
                }
                Err(e) => {
                    eprintln!("Failed to serialize chat sessions: {e}");
                }
            }
            in_flight.store(false, std::sync::atomic::Ordering::Relaxed);
        });
    }
}
