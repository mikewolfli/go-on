use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

use crate::i18n::I18n;
use crate::widgets::cache::CachedView;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AutoTuneState {
    temperature: f32,
    top_p: f32,
    max_tokens: u32,
    aggressive: bool,
}

impl Default for AutoTuneState {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            top_p: 0.95,
            max_tokens: 2048,
            aggressive: false,
        }
    }
}

pub struct AutoTuneView {
    state: AutoTuneState,
    /// Timestamp of the last successful save, used to show a brief "Saved ✓" flash.
    saved_at: Option<std::time::Instant>,
    cached_view: CachedView,
}

impl AutoTuneView {
    pub fn new() -> Self {
        Self {
            state: Self::load_state(),
            saved_at: None,
            cached_view: CachedView::new(),
        }
    }

    fn state_path() -> PathBuf {
        if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
            dirs.config_dir().join("autotune_state.json")
        } else {
            PathBuf::from("autotune_state.json")
        }
    }

    fn load_state() -> AutoTuneState {
        crate::fs_util::load_json_with_backup(&Self::state_path(), "autotune state")
    }

    pub fn load_runtime_options() -> Value {
        let state = Self::load_state();
        serde_json::json!({
            "temperature": state.temperature,
            "top_p": state.top_p,
            "max_tokens": state.max_tokens,
            "aggressive": state.aggressive,
        })
    }

    fn save_state(&self) {
        let path = Self::state_path();
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("Failed to create autotune state dir: {e}");
                return;
            }
        }
        match serde_json::to_string_pretty(&self.state) {
            Ok(content) => {
                if let Err(e) = crate::fs_util::atomic_write(&path, &content) {
                    eprintln!("Failed to write autotune state {}: {e}", path.display());
                }
            }
            Err(e) => eprintln!("Failed to serialize autotune state: {e}"),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.heading(i18n.t("tab.autotune"));
                ui.label(i18n.t("autotune.hint"));
                ui.separator();

                // Compute hash from render-relevant state
                let hash = crate::section_hash!(
                    self.state.temperature,
                    self.state.top_p,
                    self.state.max_tokens,
                    self.state.aggressive,
                    self.saved_at.map(|t| t.elapsed().as_secs() / 2),
                );

                self.cached_view
                    .check_or_render(ui, "autotune", hash, |ui| {
                        let mut changed = false;
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut self.state.temperature, 0.0..=2.0)
                                    .text(i18n.t("autotune.temperature")),
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut self.state.top_p, 0.1..=1.0)
                                    .text(i18n.t("autotune.topP")),
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut self.state.max_tokens, 128..=8192)
                                    .text(i18n.t("autotune.maxTokens")),
                            )
                            .changed();
                        changed |= ui
                            .checkbox(&mut self.state.aggressive, i18n.t("autotune.aggressive"))
                            .changed();

                        if ui.button(i18n.t("autotune.resetDefaults")).clicked() {
                            self.state = AutoTuneState::default();
                            changed = true;
                        }

                        if changed {
                            // Inline save_state to avoid borrowing &self while cached_view is mutably borrowed
                            let path = AutoTuneView::state_path();
                            if let Some(parent) = path.parent() {
                                if let Err(e) = std::fs::create_dir_all(parent) {
                                    eprintln!("Failed to create autotune state dir: {e}");
                                } else {
                                    match serde_json::to_string_pretty(&self.state) {
                                        Ok(content) => {
                                            if let Err(e) =
                                                crate::fs_util::atomic_write(&path, &content)
                                            {
                                                eprintln!(
                                                    "Failed to write autotune state {}: {e}",
                                                    path.display()
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            eprintln!("Failed to serialize autotune state: {e}")
                                        }
                                    }
                                }
                            }
                            self.saved_at = Some(std::time::Instant::now());
                        }

                        // Show a brief "Saved ✓" flash for 2 seconds after any change.
                        if let Some(at) = self.saved_at {
                            if at.elapsed() < std::time::Duration::from_secs(2) {
                                ui.colored_label(
                                    egui::Color32::from_rgb(60, 180, 100),
                                    i18n.t("autotune.saved"),
                                );
                                ui.ctx().request_repaint();
                            } else {
                                self.saved_at = None;
                            }
                        }
                    });
            });
    }
}
