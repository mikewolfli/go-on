use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

use crate::i18n::I18n;

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
}

impl AutoTuneView {
    pub fn new() -> Self {
        Self {
            state: Self::load_state(),
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
        match std::fs::read_to_string(Self::state_path()) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => AutoTuneState::default(),
        }
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
                if let Err(e) = std::fs::write(&path, content) {
                    eprintln!("Failed to write autotune state {}: {e}", path.display());
                }
            }
            Err(e) => eprintln!("Failed to serialize autotune state: {e}"),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        ui.heading(i18n.t("tab.autotune"));
        ui.label(i18n.t("autotune.hint"));
        ui.separator();

        let mut changed = false;
        changed |= ui
            .add(egui::Slider::new(&mut self.state.temperature, 0.0..=2.0).text(i18n.t("autotune.temperature")))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut self.state.top_p, 0.1..=1.0).text(i18n.t("autotune.topP")))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut self.state.max_tokens, 128..=8192).text(i18n.t("autotune.maxTokens")))
            .changed();
        changed |= ui
            .checkbox(&mut self.state.aggressive, i18n.t("autotune.aggressive"))
            .changed();

        if ui.button(i18n.t("autotune.resetDefaults")).clicked() {
            self.state = AutoTuneState::default();
            changed = true;
        }

        if changed {
            self.save_state();
        }
    }
}
