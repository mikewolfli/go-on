use crate::widgets::cache::Section;

/// Frame whose background fill is cached. Skips frame widget when
/// the visual style (dark/light mode) hasn't changed.
/// Reserved for future use — kept to avoid re-adding later.
#[allow(dead_code)]
pub struct CachedFrame {
    section: Section,
}

#[allow(dead_code)]
impl CachedFrame {
    pub fn new(section: Section) -> Self {
        Self { section }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        cache: &mut crate::widgets::cache::SectionCache,
        content: impl FnOnce(&mut egui::Ui),
    ) {
        let hash = if ui.visuals().dark_mode { 1u64 } else { 0u64 };

        if let Some(size) = cache.check(&self.section, hash) {
            let (_id, rect) = ui.allocate_space(size);
            let bg = ui.style().visuals.window_fill();
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(0), bg);
        } else {
            let resp = egui::Frame::NONE.show(ui, content);
            cache.store(&self.section, hash, resp.response.rect.size());
        }
    }
}
