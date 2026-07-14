//! Model picker sub-module for the Chat UI.
//!
//! Handles the model selection combo box and agent → model two-level picker.
//! Extracted from the monolithic ui.rs for better organization.

use super::super::*;

/// Render a model picker combo box showing the currently selected model.
/// Returns true if the selection changed.
pub fn render_model_picker(chat: &mut ChatView, ui: &mut egui::Ui, _i18n: &I18n) -> bool {
    let prev_model = chat.selected_model.clone();

    let agent_keys: Vec<String> = chat.available_agent_models.keys().cloned().collect();

    // Level 1: Agent (ComboBox)
    let agent_text = if chat.selected_agent.is_empty() {
        "All Agents".to_string()
    } else {
        chat.selected_agent.clone()
    };
    egui::ComboBox::from_id_salt("agent_sel")
        .selected_text(&agent_text)
        .show_ui(ui, |ui| {
            if ui
                .selectable_value(&mut chat.selected_agent, String::new(), "All Agents")
                .clicked()
            {
                chat.selected_model = "auto".to_string();
                chat.sync_model_selection();
            }
            for agent in &agent_keys {
                if ui
                    .selectable_value(&mut chat.selected_agent, agent.clone(), agent.as_str())
                    .clicked()
                {
                    chat.selected_model = "auto".to_string();
                    chat.sync_model_selection();
                }
            }
        });

    ui.add_space(4.0);

    // Level 2: Model (ComboBox, filtered by selected agent)
    // Show the model dropdown for ALL agents including copilot.
    // Copilot models (gpt-4.1, claude-sonnet-4, etc.) are listed alongside
    // the "copilot/auto" sentinel which allows Copilot server-side auto-selection.
    let model_options: Vec<String> = if chat.selected_agent.is_empty() {
        let mut all: Vec<String> = chat.available_models.clone();
        all.insert(0, "auto".to_string());
        if chat.available_agent_models.contains_key("copilot") {
            all.push(ChatView::COPILOT_AUTO_MODEL.to_string());
        }
        all
    } else {
        let mut agent_models = chat
            .available_agent_models
            .get(&chat.selected_agent)
            .cloned()
            .unwrap_or_default();
        agent_models.insert(0, "auto".to_string());
        agent_models
    };

    egui::ComboBox::from_id_salt("model_sel")
        .selected_text(if chat.selected_model == "auto" {
            "AUTO".to_string()
        } else if chat.selected_model == ChatView::COPILOT_AUTO_MODEL {
            "copilot/auto".to_string()
        } else {
            chat.selected_model.clone()
        })
        .show_ui(ui, |ui| {
            for m in &model_options {
                let label = if m == "auto" {
                    "AUTO"
                } else if m == ChatView::COPILOT_AUTO_MODEL {
                    "copilot/auto"
                } else {
                    m.as_str()
                };
                ui.selectable_value(&mut chat.selected_model, m.to_string(), label);
            }
        });

    chat.selected_model != prev_model
}
