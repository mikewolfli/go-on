use std::hash::{DefaultHasher, Hash, Hasher};

use crate::widgets::cache::Section;

/// Label that skips text shaping when content hash matches.
/// On cache hit, allocates the previously rendered size (empty space).
/// Reserved for future use — kept to avoid re-adding later.
#[allow(dead_code)]
pub struct CachedLabel {
    section: Section,
}

#[allow(dead_code)]
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

        if let Some(size) = cache.check(&self.section, hash) {
            ui.allocate_space(size);
        } else {
            let resp = ui.label(text);
            cache.store(&self.section, hash, resp.rect.size());
        }
    }
}
