#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod backend;
mod config;
mod i18n;
mod keyring_util;
mod theme;
mod views;

use app::GoOnApp;

fn font_cache_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("com", "goon", "go-on-gui")
        .map(|dirs| dirs.config_dir().join("font_path.cache"))
}

fn read_cached_font_path() -> Option<String> {
    let path = font_cache_path()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn write_cached_font_path(path: &str) {
    let Some(cache_path) = font_cache_path() else {
        return;
    };
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(cache_path, path);
}

/// Generate a simple 64×64 RGBA icon programmatically:
/// blue circle with "GO" letters in the center
fn make_icon() -> egui::IconData {
    let w: u32 = 64;
    let h: u32 = 64;
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let r = cx - 2.0;
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < r - 1.0 {
                rgba.extend_from_slice(&[32, 120, 220, 255]);
            } else if dist < r + 1.0 {
                let t = ((r + 1.0 - dist) * 255.0) as u8;
                rgba.extend_from_slice(&[
                    (32u16 * t as u16 / 255) as u8,
                    (120u16 * t as u16 / 255) as u8,
                    (220u16 * t as u16 / 255) as u8,
                    t,
                ]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    egui::IconData {
        rgba,
        width: w,
        height: h,
    }
}

#[tokio::main]
async fn main() -> eframe::Result<()> {
    let icon = make_icon();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Go-On GUI")
            .with_inner_size([1200.0, 800.0])
            .with_icon(icon),
        // Explicitly keep vsync on to avoid tearing/jitter on most desktops.
        vsync: true,
        // Pin renderer choice to avoid backend switching differences across environments.
        renderer: eframe::Renderer::default(),
        ..Default::default()
    };

    eframe::run_native(
        "Go-On GUI",
        options,
        Box::new(|cc| {
            // Load Chinese-capable font for CJK text rendering
            let mut fonts = egui::FontDefinitions::default();
            // Try common Chinese fonts on Linux and macOS, plus user-installed paths
            let home_dir = std::env::var("HOME")
                .ok()
                .or_else(|| std::env::var("USERPROFILE").ok())
                .unwrap_or_default();
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
            let mut loaded_font_path: Option<String> = None;

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

            // First, try the cached path from previous successful startup.
            if let Some(cached_path) = read_cached_font_path() {
                if std::path::Path::new(&cached_path).exists()
                    && load_font(&mut fonts, &cached_path)
                {
                    cjk_found = true;
                    loaded_font_path = Some(cached_path.clone());
                    eprintln!("Loaded CJK font from cache: {}", cached_path);
                }
            }

            // Check system font paths first (cached list)
            if !cjk_found {
                for path in &cjk_fonts {
                    if load_font(&mut fonts, path) {
                        cjk_found = true;
                        loaded_font_path = Some((*path).to_string());
                        eprintln!("Loaded CJK font from: {}", path);
                        break;
                    }
                }
            }
            // Then check user font paths if system fonts not found
            if !cjk_found {
                for path in &user_fonts {
                    if load_font(&mut fonts, path) {
                        cjk_found = true;
                        loaded_font_path = Some(path.clone());
                        eprintln!("Loaded CJK font from user dir: {}", path);
                        break;
                    }
                }
            }

            if let Some(path) = loaded_font_path {
                write_cached_font_path(&path);
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
