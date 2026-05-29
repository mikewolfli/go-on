// DEPRECATED: This widget is retained for reference but is NOT currently used.
// The caching approach doesn't work with immediate-mode rendering.
// Remove in a future cleanup round.

use crate::widgets::cache::{Section, SectionCache};

/// Wraps a section render with cache check + store.
/// On cache hit: allocates the previous size (no widget rebuild).
/// On miss: calls `render_fn`, captures size, caches it.
/// Reserved for future use — kept to avoid re-adding later.
#[allow(dead_code)] // F-GAP-48: Widget caching layer, kept for future UI optimization
pub fn cached_section(
    ui: &mut egui::Ui,
    cache: &mut SectionCache,
    section: Section,
    hash: u64,
    render_fn: impl FnOnce(&mut egui::Ui),
) {
    let resp = egui::Frame::NONE.show(ui, render_fn);
    cache.store(&section, hash, resp.response.rect.size());
}
