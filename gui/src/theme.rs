use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, Style, TextStyle, Vec2, Visuals};

pub enum Theme {
    Minimal,
    GuoFeng,
    Wuxia,
    ShanShui,
    HelloKitty,
    ServeThePeople,
}

impl Theme {
    /// Compute a font size from a base size and a scale factor.
    /// Apply this to all font sizes for accessibility (user-configurable scaling).
    pub fn font_size(base: f32, scale: f64) -> f32 {
        base * scale as f32
    }
    pub fn all() -> &'static [(Theme, &'static str)] {
        &[
            (Theme::Minimal, "简约"),
            (Theme::ServeThePeople, "为人民服务"),
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
            "为人民服务" | "serve_the_people" | "serve-the-people" => Theme::ServeThePeople,
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
            Theme::ServeThePeople => i18n.t("theme.serveThePeople"),
        }
    }

    pub fn apply(&self, ctx: &egui::Context, scale: f64) {
        let mut style = (*ctx.global_style()).clone();
        // Common base: typography + spacing + interaction density
        // All font sizes are scaled by the user-configurable `scale` factor.
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(Self::font_size(25.0, scale), FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Name("Title".into()),
            FontId::new(Self::font_size(21.0, scale), FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Body,
            FontId::new(Self::font_size(16.0, scale), FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(Self::font_size(15.0, scale), FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(Self::font_size(14.0, scale), FontFamily::Monospace),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(Self::font_size(13.0, scale), FontFamily::Proportional),
        );

        style.spacing.item_spacing = Vec2::new(10.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 7.0);
        style.spacing.indent = 20.0;
        style.spacing.combo_width = 0.0;
        style.spacing.interact_size = Vec2::new(44.0, 26.0);
        style.spacing.text_edit_width = 320.0;
        style.spacing.scroll.bar_width = 6.0;
        style.spacing.scroll.bar_inner_margin = 2.0;
        style.spacing.scroll.bar_outer_margin = 1.0;
        style.spacing.window_margin = egui::Margin::symmetric(12, 8);

        // Anti-jitter: stable selection without size changes
        style.visuals.selection.stroke = Stroke::new(1.0, style.visuals.selection.stroke.color);

        match self {
            Theme::Minimal => Self::apply_minimal(&mut style),
            Theme::GuoFeng => Self::apply_guofeng(&mut style),
            Theme::Wuxia => Self::apply_wuxia(&mut style),
            Theme::ShanShui => Self::apply_shanshui(&mut style),
            Theme::HelloKitty => Self::apply_hellokitty(&mut style),
            Theme::ServeThePeople => Self::apply_serve_the_people(&mut style),
        }
        ctx.set_global_style(style);
    }

    // ── 简约 ──────────────────────────────────────────────────
    // Modern, clean, professional light theme
    fn apply_minimal(style: &mut Style) {
        style.visuals = Visuals::light();
        let r8 = CornerRadius::same(8);
        let text_pri = Color32::from_rgb(28, 28, 32);
        let accent = Color32::from_rgb(0, 106, 255);

        style.visuals.window_fill = Color32::from_rgb(255, 255, 255);
        style.visuals.panel_fill = Color32::from_rgb(244, 245, 248);
        style.visuals.override_text_color = Some(text_pri);
        style.visuals.hyperlink_color = accent;
        style.visuals.selection.stroke = Stroke::new(1.0, accent);

        let w = &mut style.visuals.widgets;
        w.noninteractive.bg_fill = Color32::from_rgb(255, 255, 255);
        // All stroke widths are uniform to prevent hover/active layout jitter.
        w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(224, 226, 232));
        w.noninteractive.corner_radius = r8;

        w.inactive.bg_fill = Color32::from_rgb(240, 241, 245);
        w.inactive.fg_stroke = Stroke::new(1.0, text_pri);
        w.inactive.corner_radius = r8;

        w.hovered.bg_fill = Color32::from_rgb(240, 241, 245);
        w.hovered.fg_stroke = Stroke::new(1.0, accent);
        w.hovered.corner_radius = r8;

        w.active.bg_fill = accent;
        w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.active.corner_radius = r8;

        w.open.bg_fill = Color32::from_rgb(255, 255, 255);
    }

    // ── 为人民服务 ───────────────────────────────────
    fn apply_serve_the_people(style: &mut Style) {
        style.visuals = Visuals::dark();
        let r7 = CornerRadius::same(7);
        let text_pri = Color32::from_rgb(246, 232, 205);
        let accent = Color32::from_rgb(196, 28, 37);
        let accent_soft = Color32::from_rgb(132, 22, 30);
        let gold = Color32::from_rgb(232, 183, 68);

        style.visuals.window_fill = Color32::from_rgb(34, 10, 12);
        style.visuals.panel_fill = Color32::from_rgb(56, 14, 18);
        style.visuals.override_text_color = Some(text_pri);
        style.visuals.hyperlink_color = gold;
        style.visuals.selection.bg_fill = accent_soft;
        style.visuals.selection.stroke = Stroke::new(1.5, gold);

        let w = &mut style.visuals.widgets;
        w.noninteractive.bg_fill = Color32::from_rgb(62, 18, 23);
        w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(122, 48, 56));
        w.noninteractive.corner_radius = r7;

        w.inactive.bg_fill = Color32::from_rgb(88, 22, 29);
        w.inactive.fg_stroke = Stroke::new(1.0, text_pri);
        w.inactive.corner_radius = r7;

        w.hovered.bg_fill = Color32::from_rgb(108, 26, 34);
        w.hovered.fg_stroke = Stroke::new(1.0, gold);
        w.hovered.corner_radius = r7;

        w.active.bg_fill = accent;
        w.active.fg_stroke = Stroke::new(1.0, Color32::from_rgb(255, 242, 214));
        w.active.corner_radius = r7;

        w.open.bg_fill = Color32::from_rgb(96, 24, 31);
    }

    // ── 国风 ──────────────────────────────────────────────────
    fn apply_guofeng(style: &mut Style) {
        style.visuals = Visuals::light();
        let r6 = CornerRadius::same(6);
        let text_pri = Color32::from_rgb(55, 35, 20);
        let accent = Color32::from_rgb(190, 45, 55);

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
        w.hovered.fg_stroke = Stroke::new(1.0, accent);
        w.active.bg_fill = accent;
        w.active.fg_stroke = Stroke::new(1.0, Color32::from_rgb(255, 248, 231));
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
        w.hovered.fg_stroke = Stroke::new(1.0, accent);
        w.active.bg_fill = accent;
        w.active.fg_stroke = Stroke::new(1.0, Color32::from_rgb(240, 220, 185));
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
        w.hovered.fg_stroke = Stroke::new(1.0, accent);
        w.active.bg_fill = accent;
        w.active.fg_stroke = Stroke::new(1.0, Color32::from_rgb(230, 248, 240));
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
        w.hovered.fg_stroke = Stroke::new(1.0, accent);
        w.active.bg_fill = accent;
        w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.active.corner_radius = r6;
        style.visuals.selection.bg_fill = Color32::from_rgb(250, 220, 230);
    }
}
