#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod backend;
mod config;
mod fs_util;
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
    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.len() > 4096 {
        eprintln!(
            "WARNING: font cache file too large ({} bytes), ignoring",
            metadata.len()
        );
        return None;
    }
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

/// Returns `true` if the file at `path` has a `.ttc` extension.
/// TTC (TrueType Collection) files contain multiple fonts in one container
/// and are NOT supported by egui::FontData. Loading them corrupts the atlas.
fn is_ttc(path: &str) -> bool {
    path.ends_with(".ttc") || path.ends_with(".TTC")
}

/// Load a CJK font into egui's FontDefinitions, trying known system paths.
///
/// Skips `.ttc` files because egui::FontData does not handle multi-font
/// containers — loading a TTC corrupts the font atlas (black screen).
fn load_cjk_font(fonts: &mut egui::FontDefinitions) -> bool {
    let home_dir = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
        .unwrap_or_default();

    // Known CJK font paths across Linux, macOS, and Windows.
    // IMPORTANT: .ttc paths are kept for cross-platform coverage but are
    // SKIPPED at load time if a .ttf/.otf alternative is found first.
    // On some systems (e.g. macOS) all CJK fonts are .ttc — those systems
    // will fall through to the user font directory search below.
    let cjk_fonts = [
        // ── Linux (TTF/OTF — safe) ──
        "/usr/share/fonts/opentype/noto/NotoSansCJKSC-Regular.otf",
        "/usr/share/fonts/truetype/noto/NotoSansCJKsc-Regular.otf",
        "/usr/share/fonts/opentype/wqy-microhei/WenQuanYiMicroHei.ttf",
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        // ── macOS (TTF — safe) ──
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        // ── Linux (TTC — fallback, may be skipped) ──
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/local/share/fonts/noto/NotoSansCJK-Regular.ttc",
        "/usr/local/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        // ── macOS (TTC — fallback) ──
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        // ── Windows (TTC — fallback) ──
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simsun.ttc",
    ];
    let user_fonts: Vec<String> = if !home_dir.is_empty() {
        vec![
            // TTF/OTF user fonts first (safe)
            format!("{home_dir}/.fonts/WenQuanYiMicroHei.ttf"),
            format!("{home_dir}/.fonts/NotoSansCJKSC-Regular.otf"),
            // TTC user fonts (fallback, may be skipped)
            format!("{home_dir}/.fonts/NotoSansCJK-Regular.ttc"),
            format!("{home_dir}/.fonts/wqy-microhei.ttc"),
            format!("{home_dir}/.local/share/fonts/NotoSansCJK-Regular.ttc"),
            format!("{home_dir}/.local/share/fonts/wqy-microhei.ttc"),
        ]
    } else {
        Vec::new()
    };

    let mut cjk_found = false;
    let mut loaded_font_path: Option<String> = None;

    // Helper: attempt to load a font from `path`. Returns true on success.
    // Silently skips TTC files (egui does not support multi-font containers).
    let try_load = |fonts: &mut egui::FontDefinitions, path: &str| -> bool {
        if is_ttc(path) {
            return false;
        }
        let font_data = match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => return false,
        };
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
    };

    // 1. Try cached font path first
    if let Some(cached_path) = read_cached_font_path() {
        if std::path::Path::new(&cached_path).exists() && try_load(fonts, &cached_path) {
            cjk_found = true;
            loaded_font_path = Some(cached_path.clone());
            eprintln!("Loaded CJK font from cache: {}", cached_path);
        }
    }

    // 2. Try system paths
    if !cjk_found {
        for path in &cjk_fonts {
            if try_load(fonts, path) {
                cjk_found = true;
                loaded_font_path = Some((*path).to_string());
                break;
            }
        }
    }

    // 3. Try user font directories
    if !cjk_found {
        for path in &user_fonts {
            if try_load(fonts, path) {
                cjk_found = true;
                loaded_font_path = Some(path.clone());
                break;
            }
        }
    }

    // 4. If no CJK font found yet, try scanning common system font directories
    //    for any .ttf/.otf file that looks like a CJK font.
    if !cjk_found {
        let search_dirs = [
            "/usr/share/fonts",
            "/usr/local/share/fonts",
            "/System/Library/Fonts",
        ];
        for dir in &search_dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let path_str = path.to_string_lossy();
                    if is_ttc(&path_str) {
                        continue;
                    }
                    // Check if filename suggests CJK
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if name.contains("cjk")
                        || name.contains("chinese")
                        || name.contains("han")
                        || name.contains("noto")
                        || name.contains("wqy")
                        || name.contains("wenquan")
                        || name.contains("droid")
                        || name.contains("fallback")
                        || name.contains("arialuni")
                    {
                        if try_load(fonts, &path_str) {
                            cjk_found = true;
                            loaded_font_path = Some(path_str.to_string());
                            break;
                        }
                    }
                }
                if cjk_found {
                    break;
                }
            }
        }
    }

    if let Some(path) = loaded_font_path {
        write_cached_font_path(&path);
    }
    cjk_found
}

fn auto_detect_proxy() {
    if std::env::var("HTTPS_PROXY").is_ok() || std::env::var("https_proxy").is_ok() {
        return;
    }
    let proxies: &[&str] = &[
        "http://127.0.0.1:15732",
        "http://127.0.0.1:7890",
        "http://127.0.0.1:25519",
        "http://127.0.0.1:10809",
        "http://127.0.0.1:10809",
        "http://127.0.0.1:1087",
        "http://127.0.0.1:1080",
    ];
    for proxy_url in proxies {
        let addr = proxy_url
            .trim_start_matches("http://")
            .trim_start_matches("socks5://");
        if let Some(port_str) = addr.split(':').nth(1) {
            if let Ok(port) = port_str.parse::<u16>() {
                if let Ok(socket_addr) = format!("127.0.0.1:{port}").parse() {
                    if std::net::TcpStream::connect_timeout(
                        &socket_addr,
                        std::time::Duration::from_millis(100),
                    )
                    .is_ok()
                    {
                        std::env::set_var("HTTPS_PROXY", proxy_url);
                        std::env::set_var("https_proxy", proxy_url);
                        eprintln!(
                            "auto_detect_proxy: found proxy at {proxy_url}, set HTTPS_PROXY."
                        );
                        return;
                    }
                }
            }
        }
    }
}

fn make_icon() -> egui::IconData {
    let (w, h) = (64u32, 64u32);
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let r = cx - 2.0;
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let dist = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
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

#[cfg(has_app_icon)]
fn load_embedded_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!(env!("GOON_ICON_PATH"));
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Ico).ok()?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}
#[cfg(not(has_app_icon))]
fn load_embedded_icon() -> Option<egui::IconData> {
    None
}

#[tokio::main]
async fn main() -> eframe::Result<()> {
    auto_detect_proxy();
    let icon = load_embedded_icon().unwrap_or_else(make_icon);
    let config = crate::config::load_app_config();
    let title = app::GoOnApp::detect_initial_window_title(&config);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([640.0, 480.0])
            .with_icon(icon),
        vsync: true,
        // Explicitly use glow (OpenGL) backend for Linux compatibility
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let result = eframe::run_native(
        "Go-On GUI",
        options,
        Box::new(|cc| {
            let mut fonts = egui::FontDefinitions::default();
            if !load_cjk_font(&mut fonts) {
                eprintln!("WARNING: No CJK font found! Text may show as boxes.");
            }
            cc.egui_ctx.set_fonts(fonts);
            Ok(Box::new(GoOnApp::new(config)))
        }),
    );
    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("FATAL: GUI error: {e}");
            Err(e)
        }
    }
}
