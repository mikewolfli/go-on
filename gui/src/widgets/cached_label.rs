// DEPRECATED: This widget is retained for reference but is NOT currently used.
// The caching approach doesn't work with immediate-mode rendering.
// Remove in a future cleanup round.

use std::hash::{DefaultHasher, Hash, Hasher};

use crate::widgets::cache::Section;

/// Label that skips text shaping when content hash matches.
/// On cache hit, allocates the previously rendered size (empty space).
/// Reserved for future use — kept to avoid re-adding later.
#[allow(dead_code)] // F-GAP-48: Widget caching layer, kept for future UI optimization
pub struct CachedLabel {
    section: Section,
}

#[allow(dead_code)] // F-GAP-48: Widget caching layer, kept for future UI optimization
impl CachedLabel {
    pub fn new(section: Section) -> Self {
        Self { section }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        cache: &mut crate::widgets::cache::SectionCache,
        text: &str,
    ) {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        text.len().hash(&mut hasher);
        let hash = hasher.finish();

        let resp = ui.label(text);
        cache.store(&self.section, hash, resp.rect.size());
    }
}
