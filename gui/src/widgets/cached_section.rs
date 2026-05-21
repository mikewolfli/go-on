use crate::widgets::cache::{Section, SectionCache};

/// Wraps a section render with cache check + store.
/// On cache hit: allocates the previous size (no widget rebuild).
/// On miss: calls `render_fn`, captures size, caches it.
pub fn cached_section(
    ui: &mut egui::Ui,
    cache: &mut SectionCache,
    section: Section,
    hash: u64,
    render_fn: impl FnOnce(&mut egui::Ui),
) {
    if let Some(size) = cache.check(section, hash) {
        ui.allocate_space(size);
    } else {
        let resp = egui::Frame::NONE.show(ui, render_fn);
        cache.store(section, hash, resp.response.rect.size());
    }
}
