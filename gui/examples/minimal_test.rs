/// Minimal egui test to verify rendering works on this system.
/// Run with: cargo run --example minimal_test
use eframe::egui;

fn main() -> eframe::Result<()> {
    eprintln!("MINIMAL_TEST: Starting...");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([600.0, 400.0]),
        vsync: true,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Minimal Test",
        options,
        Box::new(|cc| {
            eprintln!("MINIMAL_TEST: CreationContext, gl={:?}", cc.gl.is_some());
            Ok(Box::new(MinimalApp::default()))
        }),
    )
}

#[derive(Default)]
struct MinimalApp {
    counter: u32,
}

impl eframe::App for MinimalApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.counter += 1;
        if self.counter < 10 {
            eprintln!(
                "MINIMAL_TEST: Frame #{} screen_rect={:?}",
                self.counter,
                ctx.screen_rect()
            );
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Hello World!");
            ui.label(format!("Frame #{}", self.counter));
            if ui.button("Click me").clicked() {
                eprintln!("MINIMAL_TEST: Button clicked!");
            }
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}
