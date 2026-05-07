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
            // Try common Chinese fonts on Linux and macOS, plus user-installed paths
            let home_dir = std::env::var("HOME").unwrap_or_default();
            let cjk_fonts = [
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/opentype/noto/NotoSansCJKSC-Regular.otf",
                "/usr/share/fonts/truetype/noto/NotoSansCJKsc-Regular.otf",
                "/usr/share/fonts/opentype/wqy-microhei/WenQuanYiMicroHei.ttf",
                "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
                "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
                "/System/Library/Fonts/PingFang.ttc",
                "/System/Library/Fonts/STHeiti Light.ttc",
                "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
                "C:\\Windows\\Fonts\\msyh.ttc",
                "C:\\Windows\\Fonts\\simsun.ttc",
                "/usr/local/share/fonts/noto/NotoSansCJK-Regular.ttc",
                "/usr/local/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            ];

            // Also check user home directory font paths
            let user_fonts: Vec<String> = if !home_dir.is_empty() {
                vec![
                    format!("{home_dir}/.fonts/NotoSansCJK-Regular.ttc"),
                    format!("{home_dir}/.fonts/wqy-microhei.ttc"),
                    format!("{home_dir}/.fonts/WenQuanYiMicroHei.ttf"),
                    format!("{home_dir}/.local/share/fonts/NotoSansCJK-Regular.ttc"),
                    format!("{home_dir}/.local/share/fonts/wqy-microhei.ttc"),
                ]
            } else {
                Vec::new()
            };

            let mut cjk_found = false;

            // Helper: load a single font file into the egui font definitions
            let load_font = |fonts: &mut egui::FontDefinitions, path: &str| -> bool {
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
                    true
                } else {
                    false
                }
            };

            // Check system font paths first
            for path in &cjk_fonts {
                if load_font(&mut fonts, path) {
                    cjk_found = true;
                    break;
                }
            }
            // Then check user font paths
            if !cjk_found {
                for path in &user_fonts {
                    if load_font(&mut fonts, path) {
                        cjk_found = true;
                        break;
                    }
                }
            }

            if !cjk_found {
                eprintln!(
                    "WARNING: No CJK font found! Chinese/Japanese/Korean text may show as boxes.\n"
                );
                eprintln!("  Install a CJK font such as noto-fonts-cjk or wqy-microhei:");
                eprintln!("    Debian/Ubuntu: sudo apt install fonts-noto-cjk");
                eprintln!("    Fedora:         sudo dnf install google-noto-cjk-fonts");
                eprintln!("    Arch:           sudo pacman -S noto-fonts-cjk");
                eprintln!("    macOS:          Already bundled with the system.");
                eprintln!();
                eprintln!("  Or place a .ttf/.ttc file in ~/.fonts/ or /usr/local/share/fonts/");
            }
            cc.egui_ctx.set_fonts(fonts);
            Ok(Box::new(GoOnApp::new()))
        }),
    )
}
