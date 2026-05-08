fn main() {
    // Embed application icon for the .exe on Windows (optional)
    if cfg!(target_os = "windows") {
        let icon_path = std::path::Path::new("../icon.ico");
        if icon_path.exists() {
            let mut res = winres::WindowsResource::new();
            res.set_icon("../icon.ico");
            if let Err(e) = res.compile() {
                eprintln!("Warning: icon embed failed: {}.", e);
            }
        }
    }
}
