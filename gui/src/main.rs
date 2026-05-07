mod app;
mod backend;
mod config;
mod i18n;
mod theme;
mod views;

use app::GoOnApp;

#[tokio::main]
async fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Go-On GUI")
            .with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Go-On GUI",
        options,
        Box::new(|cc| {
            // Load Chinese-capable font for CJK text rendering
            let mut fonts = egui::FontDefinitions::default();
            // Try common Chinese fonts on Linux and macOS
            let cjk_fonts = [
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/opentype/wqy-microhei/WenQuanYiMicroHei.ttf",
                "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
                "/System/Library/Fonts/PingFang.ttc",
                "/System/Library/Fonts/STHeiti Light.ttc",
                "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
                "C:\\Windows\\Fonts\\msyh.ttc",
                "C:\\Windows\\Fonts\\simsun.ttc",
            ];
            for path in &cjk_fonts {
                if let Ok(font_data) = std::fs::read(path) {
                    fonts.font_data.insert(
                        "cjk".to_owned(),
                        std::sync::Arc::new(egui::FontData::from_owned(font_data)),
                    );
                    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                        family.insert(0, "cjk".to_owned());
                    }
                    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                        family.insert(0, "cjk".to_owned());
                    }
                    break;
                }
            }
            cc.egui_ctx.set_fonts(fonts);
            Ok(Box::new(GoOnApp::new()))
        }),
    )
}
