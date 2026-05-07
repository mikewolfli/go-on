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
        Box::new(|_cc| Ok(Box::new(GoOnApp::new()))),
    )
}
