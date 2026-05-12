fn main() {
    println!("cargo:rustc-check-cfg=cfg(has_app_icon)");
    println!("cargo:rerun-if-changed=../ICON.ICO");
    println!("cargo:rerun-if-changed=../icon.ico");

    let preferred = std::path::Path::new("../ICON.ICO");
    let fallback = std::path::Path::new("../icon.ico");
    let icon_path = if preferred.exists() {
        Some(preferred)
    } else if fallback.exists() {
        Some(fallback)
    } else {
        None
    };

    if let Some(path) = icon_path {
        // Make icon bytes available to Rust code at compile time.
        if let Ok(abs) = std::fs::canonicalize(path) {
            println!("cargo:rustc-cfg=has_app_icon");
            println!("cargo:rustc-env=GOON_ICON_PATH={}", abs.display());
        }

        // Embed application icon into the .exe resource on Windows.
        if cfg!(target_os = "windows") {
            let mut res = winres::WindowsResource::new();
            res.set_icon(path.to_string_lossy().as_ref());
            if let Err(e) = res.compile() {
                eprintln!("Warning: icon embed failed: {}.", e);
            }
        }
    }
}
