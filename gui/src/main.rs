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
    // Limit font cache read to 4KB to prevent OOM on corrupted/malicious files
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

fn load_cjk_font(fonts: &mut egui::FontDefinitions) -> bool {
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
        if std::path::Path::new(&cached_path).exists() && load_font(fonts, &cached_path) {
            cjk_found = true;
            loaded_font_path = Some(cached_path.clone());
            eprintln!("Loaded CJK font from cache: {}", cached_path);
        }
    }

    // Check system font paths first (cached list)
    if !cjk_found {
        for path in &cjk_fonts {
            if load_font(fonts, path) {
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
            if load_font(fonts, path) {
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

    cjk_found
}

/// Auto-detect common VPN proxy ports and set HTTPS_PROXY if found.
/// Works on Linux, macOS, and Windows — covers ClashX, v2ray/Xray, SS/SSR, Surge, Quantumult, etc.
/// This allows the app to access GitHub through the user's VPN without manual config.
///
/// # Safety
///
/// This function uses `std::env::set_var()` which is documented as **undefined behavior**
/// in multi-threaded contexts. The backend provides a thread-safe alternative:
/// [`set_secret_override`](crate::shared::secret_override::set_secret_override).
/// However, this function runs **before any threads are spawned** (called from `main()`
/// before `eframe::run_native`), so there is exactly one thread at this point.
/// The HTTPS_PROXY env var must be set as a real environment variable for `reqwest` to
/// pick it up automatically (the override map is only consulted by `get_secret()` calls
/// within go-on's own code). If the override map were used here, `reqwest`'s proxy resolution
/// would not see it, defeating the purpose.
///
/// For these two reasons — single-threaded context and the need for a real env var visible
/// to external libraries — `set_var()` is sound here.
fn auto_detect_proxy() {
    if std::env::var("HTTPS_PROXY").is_ok() || std::env::var("https_proxy").is_ok() {
        return; // User already configured a proxy
    }

    // Cross-platform common proxy ports (Linux, macOS, Windows)
    let common_proxies: &[&str] = &[
        // ── ViewTurbo (all platforms) ──
        "http://127.0.0.1:15732",
        // ── Clash / Clash Meta (all platforms) ──
        "http://127.0.0.1:7890",
        "socks5://127.0.0.1:7890",
        // ── ClashX / ClashX Pro (macOS) ──
        "http://127.0.0.1:25519",
        // ── clash-verge / Clash Nyanpasu (all platforms) ──
        "http://127.0.0.1:33210",
        // ── v2ray / Xray (all platforms) ──
        "http://127.0.0.1:10809",
        "socks5://127.0.0.1:10808",
        "http://127.0.0.1:10808",
        // ── V2RayU (macOS) ──
        "http://127.0.0.1:2080",
        // ── Qv2ray (all platforms) ──
        "http://127.0.0.1:11223",
        // ── SS / SSR (all platforms) ──
        "http://127.0.0.1:1087",
        "socks5://127.0.0.1:1086",
        // ── Standard HTTP/SOCKS proxy ──
        "http://127.0.0.1:1080",
        "socks5://127.0.0.1:1080",
        // ── Surge (macOS) ──
        "http://127.0.0.1:6152",
        // ── Quantumult X (macOS) ──
        "http://127.0.0.1:1082",
        // ── Stash (macOS) ──
        "http://127.0.0.1:9090",
        // ── Sing-box (all platforms) ──
        "http://127.0.0.1:11451",
        // ── Hiddify (all platforms) ──
        "http://127.0.0.1:9876",
        // ── Nekoray / Nekobox (all platforms) ──
        "http://127.0.0.1:10811",
        // ── Trojan (all platforms) ──
        "http://127.0.0.1:1081",
        // ── Windows VPN apps ──
        "http://127.0.0.1:51080", // SSTap
        "http://127.0.0.1:11280", // Netch
        "http://127.0.0.1:28080", // WinXray
        "http://127.0.0.1:38443", // Proxifier
        "http://127.0.0.1:8222",  // ProxyCap
    ];

    for proxy_url in common_proxies {
        // Extract host:port from URL
        let addr = proxy_url
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_start_matches("socks5://")
            .trim_start_matches("socks4://");
        if let Some(port_str) = addr.split(':').nth(1) {
            if let Ok(port) = port_str.parse::<u16>() {
                let socket_addr = match format!("127.0.0.1:{port}").parse() {
                    Ok(addr) => addr,
                    Err(_) => continue,
                };
                // Quick TCP connect to see if anything is listening
                if std::net::TcpStream::connect_timeout(
                    &socket_addr,
                    std::time::Duration::from_millis(100),
                )
                .is_ok()
                {
                    // SAFETY: single-threaded context (called before any threads are spawned
                    // in main()) and we need real env vars for reqwest's proxy resolution.
                    // See the doc comment on `auto_detect_proxy` for full rationale.
                    std::env::set_var("HTTPS_PROXY", proxy_url);
                    std::env::set_var("https_proxy", proxy_url);
                    eprintln!("auto_detect_proxy: found proxy at {proxy_url}, set HTTPS_PROXY.");
                    return;
                }
            }
        }
    }
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
    // Auto-detect VPN proxy so reqwest can reach GitHub for Copilot auth
    auto_detect_proxy();

    let icon = load_embedded_icon().unwrap_or_else(make_icon);
    // Load config to detect language for localized window title
    let config = crate::config::load_app_config();
    let title = app::GoOnApp::detect_initial_window_title(&config);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([640.0, 480.0])
            .with_icon(icon),
        // Explicitly keep vsync on to avoid tearing/jitter on most desktops.
        vsync: true,
        // Pin renderer choice to avoid backend switching differences across environments.
        renderer: eframe::Renderer::default(),
        ..Default::default()
    };

    match eframe::run_native(
        "Go-On GUI",
        options,
        Box::new(|cc| {
            // Load Chinese-capable font for CJK text rendering
            let mut fonts = egui::FontDefinitions::default();
            if !load_cjk_font(&mut fonts) {
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
            Ok(Box::new(GoOnApp::new(config)))
        }),
    ) {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("FATAL: Failed to start GUI: {}", e);
            eprintln!();
            eprintln!("Troubleshooting:");
            eprintln!("  - Ensure a display server is running (X11/Wayland on Linux)");
            eprintln!("  - On macOS, run from the .app bundle, not a symlink");
            eprintln!("  - On Windows, ensure DirectX or Vulkan drivers are installed");
            eprintln!("  - Try setting WINIT_UNIX_BACKEND=x11 on Wayland");
            Err(Into::into(e))
        }
    }
}
