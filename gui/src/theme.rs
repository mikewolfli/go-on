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
    fn apply_minimal(style: &mut Style) {
        style.visuals = Visuals::light();
        style.visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(245, 247, 250);
        style.visuals.widgets.noninteractive.fg_stroke =
            Stroke::new(1.0, Color32::from_rgb(100, 110, 130));
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(230, 234, 240);
        style.visuals.widgets.active.bg_fill = Color32::from_rgb(40, 120, 220);
        style.visuals.selection.bg_fill = Color32::from_rgb(40, 120, 220);
        style.visuals.hyperlink_color = Color32::from_rgb(40, 100, 200);
        style.spacing.item_spacing = egui::vec2(12.0, 8.0);
    }

    // ── 国风 ──────────────────────────────────────────────
    fn apply_guofeng(style: &mut Style) {
        style.visuals = Visuals::light();
        let crimson = Color32::from_rgb(180, 40, 50);
        let gold = Color32::from_rgb(200, 160, 60);
        let cream = Color32::from_rgb(250, 242, 226);
        let ink = Color32::from_rgb(60, 40, 30);

        style.visuals.window_fill = cream;
        style.visuals.panel_fill = cream;
        style.visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(245, 235, 215);
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, ink);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(235, 220, 195);
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.5, ink);
        style.visuals.widgets.active.bg_fill = crimson;
        style.visuals.widgets.active.fg_stroke = Stroke::new(2.0, gold);
        style.visuals.selection.bg_fill = crimson;
        style.visuals.hyperlink_color = crimson;
        style.visuals.selection.stroke = Stroke::new(1.0, gold);
        style.spacing.item_spacing = egui::vec2(14.0, 10.0);
    }

    // ── 武侠 ──────────────────────────────────────────────
    fn apply_wuxia(style: &mut Style) {
        style.visuals = Visuals::dark();
        let ink_black = Color32::from_rgb(25, 22, 20);
        let blood_red = Color32::from_rgb(160, 30, 35);
        let parchment = Color32::from_rgb(210, 190, 160);
        let steel = Color32::from_rgb(140, 140, 145);

        style.visuals.window_fill = ink_black;
        style.visuals.panel_fill = ink_black;
        style.visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(35, 30, 28);
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, steel);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(50, 42, 38);
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.5, steel);
        style.visuals.widgets.active.bg_fill = blood_red;
        style.visuals.widgets.active.fg_stroke = Stroke::new(2.0, parchment);
        style.visuals.selection.bg_fill = blood_red;
        style.visuals.selection.stroke = Stroke::new(1.0, steel);
        style.visuals.hyperlink_color = blood_red;
        style.visuals.override_text_color = Some(parchment);
        style.spacing.item_spacing = egui::vec2(12.0, 8.0);
    }

    // ── 山水 ──────────────────────────────────────────────
    fn apply_shanshui(style: &mut Style) {
        style.visuals = Visuals::light();
        let mist = Color32::from_rgb(230, 238, 240);
        let teal = Color32::from_rgb(50, 140, 130);
        let ink_wash = Color32::from_rgb(80, 90, 95);
        let bamboo = Color32::from_rgb(100, 160, 100);
        let cloud = Color32::from_rgb(245, 248, 245);

        style.visuals.window_fill = cloud;
        style.visuals.panel_fill = cloud;
        style.visuals.widgets.noninteractive.bg_fill = mist;
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, ink_wash);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(215, 228, 230);
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, teal);
        style.visuals.widgets.active.bg_fill = teal;
        style.visuals.widgets.active.fg_stroke = Stroke::new(1.5, Color32::from_rgb(200, 230, 225));
        style.visuals.selection.bg_fill = bamboo;
        style.visuals.selection.stroke = Stroke::new(1.0, teal);
        style.visuals.hyperlink_color = teal;
        style.spacing.item_spacing = egui::vec2(14.0, 10.0);
    }

    // ── Hello Kitty ───────────────────────────────────────
    fn apply_hellokitty(style: &mut Style) {
        style.visuals = Visuals::light();
        let pink = Color32::from_rgb(245, 180, 200);
        let hot_pink = Color32::from_rgb(230, 80, 140);
        let white = Color32::from_rgb(255, 248, 250);
        let _red_bow = Color32::from_rgb(220, 50, 80);

        style.visuals.window_fill = white;
        style.visuals.panel_fill = white;
        style.visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(252, 235, 240);
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, pink);
        style.visuals.widgets.inactive.bg_fill = pink;
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.5, hot_pink);
        style.visuals.widgets.active.bg_fill = hot_pink;
        style.visuals.widgets.active.fg_stroke = Stroke::new(2.0, white);
        style.visuals.selection.bg_fill = hot_pink;
        style.visuals.selection.stroke = Stroke::new(1.0, pink);
        style.visuals.hyperlink_color = hot_pink;
        style.spacing.item_spacing = egui::vec2(16.0, 12.0);
    }
}
