//! Message rendering sub-module for the Chat UI.
//!
//! Handles rendering of message bubbles, thinking sections, token stats,
//! role avatars, and the collapsed bubble cache.

use super::super::*;

/// Draw a colored circle avatar with the role initial letter.
/// User gets a blue circle with "U", AI gets a green circle with "A".
pub fn draw_role_avatar(ui: &mut egui::Ui, is_user: bool) {
    let size = 28.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter();
    let dark = ui.visuals().dark_mode;
    let color = if is_user {
        if dark {
            egui::Color32::from_rgb(40, 100, 200)
        } else {
            egui::Color32::from_rgb(0, 95, 240)
        }
    } else {
        if dark {
            egui::Color32::from_rgb(60, 64, 74)
        } else {
            egui::Color32::from_rgb(180, 183, 190)
        }
    };
    painter.circle_filled(rect.center(), size / 2.0, color);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        if is_user { "U" } else { "A" },
        egui::FontId::proportional(14.0),
        egui::Color32::WHITE,
    );
}

/// Render token statistics line for the current model.
pub fn render_token_stats(chat: &mut ChatView, ui: &mut egui::Ui, i18n: &I18n) {
    if !chat.show_token_details || chat.model_stats.is_empty() {
        return;
    }

    let Some(stats) = chat.model_stats.get(&chat.selected_model) else {
        return;
    };

    let success_count = stats.success_count as f64;
    let total_count = success_count + stats.error_count as f64;
    let success_rate = if total_count > 0.0 {
        (success_count / total_count * 100.0).round() as u32
    } else {
        0
    };

    let time_color = if stats.response_time_ms < 2_000 {
        egui::Color32::from_rgb(76, 175, 80)
    } else if stats.response_time_ms < 5_000 {
        egui::Color32::from_rgb(255, 193, 7)
    } else {
        egui::Color32::from_rgb(244, 67, 54)
    };

    egui::Frame::new()
        .fill(ui.visuals().window_fill().gamma_multiply(0.8))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8i8, 4i8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(i18n.t("chat.tokenStats"))
                        .strong()
                        .size(11.0),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "{}: {} ms",
                        i18n.t("chat.responseTime"),
                        stats.response_time_ms
                    ))
                    .color(time_color)
                    .size(11.0),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{}: {}",
                        i18n.t("chat.tokens"),
                        stats.token_count
                    ))
                    .size(11.0),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{}: {}%",
                        i18n.t("chat.successRate"),
                        success_rate
                    ))
                    .size(11.0),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{}: {:.0}",
                        i18n.t("chat.tokensPerMinute"),
                        stats.avg_tokens_per_minute
                    ))
                    .size(11.0)
                    .weak(),
                );
            });
        });
}

/// Render a thin collapsed bubble for unchanged messages (avoids expensive markdown re-render).
#[allow(clippy::too_many_arguments)]
pub fn render_collapsed_bubble(
    ui: &mut egui::Ui,
    i18n: &I18n,
    is_user: bool,
    timestamp: u64,
    model_name: &str,
    muted_text: egui::Color32,
    weak_text: egui::Color32,
    dark_mode: bool,
) {
    let (bubble_color, text_color) = if is_user {
        let bc = if dark_mode {
            egui::Color32::from_rgb(30, 100, 200)
        } else {
            egui::Color32::from_rgb(0, 100, 250)
        };
        (bc, egui::Color32::WHITE)
    } else {
        let bc = if dark_mode {
            egui::Color32::from_rgb(42, 46, 56)
        } else {
            egui::Color32::from_rgb(235, 237, 241)
        };
        let tc = if dark_mode {
            egui::Color32::from_rgb(212, 216, 226)
        } else {
            egui::Color32::from_rgb(30, 32, 38)
        };
        (bc, tc)
    };

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(if is_user { 60.0 } else { 8.0 });
        draw_role_avatar(ui, is_user);
        ui.add_space(6.0);

        egui::Frame::new()
            .fill(bubble_color)
            .corner_radius(12.0)
            .inner_margin(egui::Margin::symmetric(14i8, 6i8))
            .show(ui, |ui| {
                if !model_name.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("🤖 {}", model_name))
                                .size(11.0)
                                .color(weak_text),
                        );
                        if timestamp > 0 {
                            let ts = chrono::DateTime::from_timestamp(timestamp as i64, 0)
                                .map(|dt| dt.format("%H:%M").to_string())
                                .unwrap_or_default();
                            ui.label(egui::RichText::new(ts).size(10.0).color(muted_text));
                        }
                    });
                    ui.add_space(2.0);
                }
                ui.label(
                    egui::RichText::new(if is_user {
                        i18n.t("chat.userMessagePlaceholder")
                    } else {
                        i18n.t("chat.assistantMessagePlaceholder")
                    })
                    .color(text_color)
                    .size(11.0),
                );
            });
        ui.add_space(if is_user { 8.0 } else { 60.0 });
    });
    ui.add_space(4.0);
}
