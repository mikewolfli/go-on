use egui::{Color32, CornerRadius, Stroke, Style, Vec2, Visuals};

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
        // Common base: better spacing & scrollbar
        style.spacing.item_spacing = Vec2::new(10.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 7.0);
        style.spacing.indent = 20.0;
        style.spacing.combo_width = 0.0;
        style.spacing.scroll.bar_width = 6.0;
        style.spacing.scroll.bar_inner_margin = 2.0;
        style.spacing.scroll.bar_outer_margin = 1.0;
        style.spacing.window_margin = egui::Margin::symmetric(12, 8);

        match self {
            Theme::Minimal => Self::apply_minimal(&mut style),
            Theme::GuoFeng => Self::apply_guofeng(&mut style),
            Theme::Wuxia => Self::apply_wuxia(&mut style),
            Theme::ShanShui => Self::apply_shanshui(&mut style),
            Theme::HelloKitty => Self::apply_hellokitty(&mut style),
        }
        ctx.set_style(style);
    }

    // ── 简约 ──────────────────────────────────────────────────
    // Modern, clean, professional light theme
    fn apply_minimal(style: &mut Style) {
        style.visuals = Visuals::light();
        let r6 = CornerRadius::same(6);
        let text_pri = Color32::from_rgb(28, 28, 32);
        let accent = Color32::from_rgb(0, 106, 255);

        style.visuals.window_fill = Color32::from_rgb(255, 255, 255);
        style.visuals.panel_fill = Color32::from_rgb(244, 245, 248);
        style.visuals.override_text_color = Some(text_pri);
        style.visuals.hyperlink_color = accent;
        style.visuals.selection.stroke = Stroke::new(1.0, accent);

        let w = &mut style.visuals.widgets;
        w.noninteractive.bg_fill = Color32::from_rgb(255, 255, 255);
        w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(224, 226, 232));
        w.noninteractive.corner_radius = r6;

        w.inactive.bg_fill = Color32::from_rgb(240, 241, 245);
        w.inactive.fg_stroke = Stroke::new(1.0, text_pri);
        w.inactive.corner_radius = r6;

        w.hovered.bg_fill = Color32::from_rgb(240, 241, 245);
        w.hovered.fg_stroke = Stroke::new(1.5, accent);
        w.hovered.corner_radius = r6;

        w.active.bg_fill = accent;
        w.active.fg_stroke = Stroke::new(2.0, Color32::WHITE);
        w.active.corner_radius = r6;

        w.open.bg_fill = Color32::from_rgb(255, 255, 255);
    }

    // ── 国风 ──────────────────────────────────────────────────
    fn apply_guofeng(style: &mut Style) {
        style.visuals = Visuals::light();
        let r6 = CornerRadius::same(6);
        let text_pri = Color32::from_rgb(55, 35, 20);
        let accent = Color32::from_rgb(190, 45, 55);
        let gold = Color32::from_rgb(200, 165, 50);

        style.visuals.window_fill = Color32::from_rgb(252, 248, 238);
        style.visuals.panel_fill = Color32::from_rgb(248, 240, 225);
        style.visuals.override_text_color = Some(text_pri);
        style.visuals.hyperlink_color = accent;

        let w = &mut style.visuals.widgets;
        w.noninteractive.bg_fill = Color32::from_rgb(252, 248, 238);
        w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(200, 185, 160));
        w.noninteractive.corner_radius = r6;
        w.inactive.bg_fill = Color32::from_rgb(240, 228, 210);
        w.inactive.fg_stroke = Stroke::new(1.0, text_pri);
        w.inactive.corner_radius = r6;
        w.hovered.bg_fill = Color32::from_rgb(235, 220, 200);
        w.hovered.fg_stroke = Stroke::new(1.5, accent);
        w.active.bg_fill = accent;
        w.active.fg_stroke = Stroke::new(2.0, gold);
        w.active.corner_radius = r6;
        style.visuals.selection.bg_fill = Color32::from_rgb(240, 215, 210);
    }

    // ── 武侠 ──────────────────────────────────────────────────
    fn apply_wuxia(style: &mut Style) {
        style.visuals = Visuals::dark();
        let r6 = CornerRadius::same(6);
        let text_pri = Color32::from_rgb(220, 210, 190);
        let accent = Color32::from_rgb(195, 42, 48);

        style.visuals.window_fill = Color32::from_rgb(28, 25, 22);
        style.visuals.panel_fill = Color32::from_rgb(18, 16, 14);
        style.visuals.override_text_color = Some(text_pri);
        style.visuals.hyperlink_color = Color32::from_rgb(210, 60, 65);

        let w = &mut style.visuals.widgets;
        w.noninteractive.bg_fill = Color32::from_rgb(28, 25, 22);
        w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(55, 50, 42));
        w.noninteractive.corner_radius = r6;
        w.inactive.bg_fill = Color32::from_rgb(42, 38, 34);
        w.inactive.fg_stroke = Stroke::new(1.0, text_pri);
        w.inactive.corner_radius = r6;
        w.hovered.bg_fill = Color32::from_rgb(52, 46, 40);
        w.hovered.fg_stroke = Stroke::new(1.5, accent);
        w.active.bg_fill = accent;
        w.active.fg_stroke = Stroke::new(2.0, Color32::from_rgb(240, 220, 185));
        w.active.corner_radius = r6;
        style.visuals.selection.bg_fill = Color32::from_rgb(80, 40, 40);
    }

    // ── 山水 ──────────────────────────────────────────────────
    fn apply_shanshui(style: &mut Style) {
        style.visuals = Visuals::light();
        let r6 = CornerRadius::same(6);
        let text_pri = Color32::from_rgb(35, 38, 35);
        let accent = Color32::from_rgb(38, 148, 130);

        style.visuals.window_fill = Color32::from_rgb(248, 250, 248);
        style.visuals.panel_fill = Color32::from_rgb(235, 242, 240);
        style.visuals.override_text_color = Some(text_pri);
        style.visuals.hyperlink_color = accent;

        let w = &mut style.visuals.widgets;
        w.noninteractive.bg_fill = Color32::from_rgb(248, 250, 248);
        w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(200, 215, 210));
        w.noninteractive.corner_radius = r6;
        w.inactive.bg_fill = Color32::from_rgb(225, 236, 232);
        w.inactive.fg_stroke = Stroke::new(1.0, text_pri);
        w.inactive.corner_radius = r6;
        w.hovered.bg_fill = Color32::from_rgb(215, 230, 225);
        w.hovered.fg_stroke = Stroke::new(1.5, accent);
        w.active.bg_fill = accent;
        w.active.fg_stroke = Stroke::new(2.0, Color32::from_rgb(230, 248, 240));
        w.active.corner_radius = r6;
        style.visuals.selection.bg_fill = Color32::from_rgb(210, 235, 228);
    }

    // ── Hello Kitty ───────────────────────────────────────────
    fn apply_hellokitty(style: &mut Style) {
        style.visuals = Visuals::light();
        let r6 = CornerRadius::same(6);
        let text_pri = Color32::from_rgb(55, 38, 42);
        let accent = Color32::from_rgb(240, 72, 142);

        style.visuals.window_fill = Color32::from_rgb(255, 248, 250);
        style.visuals.panel_fill = Color32::from_rgb(255, 240, 244);
        style.visuals.override_text_color = Some(text_pri);
        style.visuals.hyperlink_color = accent;

        let w = &mut style.visuals.widgets;
        w.noninteractive.bg_fill = Color32::from_rgb(255, 248, 250);
        w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(240, 210, 218));
        w.noninteractive.corner_radius = r6;
        w.inactive.bg_fill = Color32::from_rgb(250, 230, 237);
        w.inactive.fg_stroke = Stroke::new(1.0, text_pri);
        w.inactive.corner_radius = r6;
        w.hovered.bg_fill = Color32::from_rgb(248, 222, 232);
        w.hovered.fg_stroke = Stroke::new(1.5, accent);
        w.active.bg_fill = accent;
        w.active.fg_stroke = Stroke::new(2.0, Color32::WHITE);
        w.active.corner_radius = r6;
        style.visuals.selection.bg_fill = Color32::from_rgb(250, 220, 230);
    }
}
