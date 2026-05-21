use std::hash::{DefaultHasher, Hash, Hasher};

use crate::widgets::cache::Section;

/// Button that skips widget rebuild when label hasn't changed.
/// Reserved for future use — kept to avoid re-adding later.
#[allow(dead_code)]
pub struct CachedButton {
    section: Section,
}

#[allow(dead_code)]
impl CachedButton {
    pub fn new(section: Section) -> Self {
        Self { section }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        cache: &mut crate::widgets::cache::SectionCache,
        label: &str,
    ) -> bool {
        let mut hasher = DefaultHasher::new();
        label.hash(&mut hasher);
        let hash = hasher.finish();

        if let Some(size) = cache.check(&self.section, hash) {
            ui.allocate_space(size);
            false
        } else {
            let resp = ui.button(label);
            cache.store(&self.section, hash, resp.rect.size());
            resp.clicked()
        }
    }
}
