use egui::{Color32, Stroke, Style, Visuals};

pub enum Theme {
    Minimal,
    GuoFeng,
    Wuxia,
    ShanShui,
    HelloKitty,
}

impl Theme {
    pub fn all() -> &'static [(Theme, &'static str)] {
        &[
            (Theme::Minimal, "简约"),
            (Theme::GuoFeng, "国风"),
            (Theme::Wuxia, "武侠"),
            (Theme::ShanShui, "山水"),
            (Theme::HelloKitty, "Hello Kitty"),
        ]
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "简约" | "minimal" => Theme::Minimal,
            "国风" | "guofeng" => Theme::GuoFeng,
            "武侠" | "wuxia" => Theme::Wuxia,
            "山水" | "shanshui" => Theme::ShanShui,
            "Hello Kitty" | "hellokitty" => Theme::HelloKitty,
            _ => Theme::Minimal,
        }
    }

    pub fn display_name<'a>(&self, i18n: &'a crate::i18n::I18n) -> std::borrow::Cow<'a, str> {
        match self {
            Theme::Minimal => i18n.t("theme.minimal"),
            Theme::GuoFeng => i18n.t("theme.guofeng"),
            Theme::Wuxia => i18n.t("theme.wuxia"),
            Theme::ShanShui => i18n.t("theme.shanshui"),
            Theme::HelloKitty => i18n.t("theme.hellokitty"),
        }
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let mut style = (*ctx.style()).clone();
        match self {
            Theme::Minimal => Self::apply_minimal(&mut style),
            Theme::GuoFeng => Self::apply_guofeng(&mut style),
            Theme::Wuxia => Self::apply_wuxia(&mut style),
            Theme::ShanShui => Self::apply_shanshui(&mut style),
            Theme::HelloKitty => Self::apply_hellokitty(&mut style),
        }
        ctx.set_style(style);
    }

    // ── 简约 ──────────────────────────────────────────────
    // Clean, high-contrast light theme
    fn apply_minimal(style: &mut Style) {
        style.visuals = Visuals::light();
        let bg = Color32::from_rgb(245, 245, 248);
        let panel = Color32::from_rgb(255, 255, 255);
        let text_main = Color32::from_rgb(30, 30, 30);
        let accent = Color32::from_rgb(40, 117, 224);
        let border = Color32::from_rgb(215, 218, 225);

        style.visuals.window_fill = panel;
        style.visuals.panel_fill = bg;
        style.visuals.widgets.noninteractive.bg_fill = panel;
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, border);
        style.visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(240, 241, 245);
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text_main);
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(235, 236, 242);
        style.visuals.widgets.active.bg_fill = accent;
        style.visuals.widgets.active.fg_stroke = Stroke::new(1.5, Color32::from_rgb(255, 255, 255));
        style.visuals.selection.bg_fill = accent;
        style.visuals.hyperlink_color = accent;
        style.visuals.override_text_color = Some(text_main);
        style.spacing.item_spacing = egui::vec2(12.0, 8.0);
    }

    // ── 国风 ──────────────────────────────────────────────
    // Warm red-gold high contrast
    fn apply_guofeng(style: &mut Style) {
        style.visuals = Visuals::light();
        let bg = Color32::from_rgb(248, 240, 225);
        let panel = Color32::from_rgb(252, 248, 238);
        let text_main = Color32::from_rgb(45, 30, 20);
        let accent = Color32::from_rgb(190, 45, 55);
        let gold = Color32::from_rgb(210, 170, 50);

        style.visuals.window_fill = panel;
        style.visuals.panel_fill = bg;
        style.visuals.widgets.noninteractive.bg_fill = panel;
        style.visuals.widgets.noninteractive.fg_stroke =
            Stroke::new(1.0, Color32::from_rgb(200, 185, 160));
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(240, 228, 210);
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text_main);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(235, 220, 200);
        style.visuals.widgets.active.bg_fill = accent;
        style.visuals.widgets.active.fg_stroke = Stroke::new(2.0, gold);
        style.visuals.selection.bg_fill = accent;
        style.visuals.hyperlink_color = accent;
        style.visuals.override_text_color = Some(text_main);
        style.spacing.item_spacing = egui::vec2(14.0, 10.0);
    }

    // ── 武侠 ──────────────────────────────────────────────
    // Dark theme with high contrast text
    fn apply_wuxia(style: &mut Style) {
        style.visuals = Visuals::dark();
        let bg = Color32::from_rgb(22, 20, 18);
        let panel = Color32::from_rgb(30, 27, 24);
        let text_main = Color32::from_rgb(225, 215, 195);
        let accent = Color32::from_rgb(185, 40, 45);
        let border = Color32::from_rgb(60, 55, 48);

        style.visuals.window_fill = panel;
        style.visuals.panel_fill = bg;
        style.visuals.widgets.noninteractive.bg_fill = panel;
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, border);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(45, 40, 36);
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text_main);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(55, 48, 42);
        style.visuals.widgets.active.bg_fill = accent;
        style.visuals.widgets.active.fg_stroke = Stroke::new(1.5, Color32::from_rgb(235, 215, 180));
        style.visuals.selection.bg_fill = accent;
        style.visuals.hyperlink_color = Color32::from_rgb(200, 60, 65);
        style.visuals.override_text_color = Some(text_main);
        style.spacing.item_spacing = egui::vec2(12.0, 8.0);
    }

    // ── 山水 ──────────────────────────────────────────────
    // Cool teal-green with high readability
    fn apply_shanshui(style: &mut Style) {
        style.visuals = Visuals::light();
        let bg = Color32::from_rgb(235, 242, 240);
        let panel = Color32::from_rgb(248, 250, 248);
        let text_main = Color32::from_rgb(35, 35, 35);
        let accent = Color32::from_rgb(40, 145, 130);
        let border = Color32::from_rgb(200, 215, 212);

        style.visuals.window_fill = panel;
        style.visuals.panel_fill = bg;
        style.visuals.widgets.noninteractive.bg_fill = panel;
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, border);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(225, 236, 232);
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text_main);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(215, 230, 225);
        style.visuals.widgets.active.bg_fill = accent;
        style.visuals.widgets.active.fg_stroke = Stroke::new(1.5, Color32::from_rgb(230, 245, 240));
        style.visuals.selection.bg_fill = accent;
        style.visuals.hyperlink_color = accent;
        style.visuals.override_text_color = Some(text_main);
        style.spacing.item_spacing = egui::vec2(14.0, 10.0);
    }

    // ── Hello Kitty ───────────────────────────────────────
    // Light pink theme with clear dark text
    fn apply_hellokitty(style: &mut Style) {
        style.visuals = Visuals::light();
        let bg = Color32::from_rgb(255, 242, 246);
        let panel = Color32::from_rgb(255, 248, 250);
        let text_main = Color32::from_rgb(50, 40, 42);
        let accent = Color32::from_rgb(235, 70, 140);
        let border = Color32::from_rgb(240, 210, 218);

        style.visuals.window_fill = panel;
        style.visuals.panel_fill = bg;
        style.visuals.widgets.noninteractive.bg_fill = panel;
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, border);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(250, 232, 238);
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text_main);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(248, 225, 232);
        style.visuals.widgets.active.bg_fill = accent;
        style.visuals.widgets.active.fg_stroke = Stroke::new(2.0, Color32::from_rgb(255, 255, 255));
        style.visuals.selection.bg_fill = accent;
        style.visuals.hyperlink_color = accent;
        style.visuals.override_text_color = Some(text_main);
        style.spacing.item_spacing = egui::vec2(16.0, 12.0);
    }
}
