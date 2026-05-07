fn main() {
    if cfg!(target_os = "windows") {
        let mut res = winres::WindowsResource::new();
        // Embed manifest for Common Controls 6.0
        res.set_manifest_file("../go-on-gui.exe.manifest");
        // Embed icon if the .ico file exists
        let icon_path = std::path::Path::new("../icon.ico");
        if icon_path.exists() {
            res.set_icon("../icon.ico");
        }
        if let Err(e) = res.compile() {
            eprintln!("Warning: winres compile failed: {}.", e);
        }
    }
}
