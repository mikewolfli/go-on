/// Sub-region cache for egui immediate-mode rendering.
///
/// egui/eframe requires widgets to be rebuilt every frame. This cache stores
/// *rendered sizes* (not widget state) so that callers can query the last
/// rendered extent without re-rendering. It does **not** skip widget rebuilds.
use std::collections::HashMap;

// ── CachedView: key-based sub-region cache ─────────────────────────

/// A string-keyed sub-region cache.
///
/// Stores `(key, hash) → rendered_size` mappings. Callers can query
/// the last rendered extent of a sub-region without rebuilding widgets.
///
/// # Usage
///
/// ```ignore
/// let mut cache = CachedView::new();
/// let size = cache.check_or_render(ui, "my_region", my_hash, |ui| {
///     ui.label("expensive content");
/// });
/// ```
pub struct CachedView {
    cache: HashMap<(String, u64), egui::Vec2>,
}

impl CachedView {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Check whether the cache contains a valid entry for `(key, hash)`.
    /// Returns `Some(size)` on hit, `None` on miss.
    pub fn check_size(&self, key: &str, hash: u64) -> Option<egui::Vec2> {
        let cache_key = (key.to_owned(), hash);
        self.cache.get(&cache_key).copied()
    }

    /// Store the rendered size for `(key, hash)`.  Overwrites any
    /// previous entry for the same key+hash.
    pub fn store_size(&mut self, key: &str, hash: u64, size: egui::Vec2) {
        self.cache.insert((key.to_owned(), hash), size);
    }

    /// Render unconditionally and store the resulting size.
    ///
    /// This intentionally does not skip rendering on cache hit because egui
    /// requires rebuilding widgets every repaint.
    pub fn check_or_render(
        &mut self,
        ui: &mut egui::Ui,
        key: &str,
        hash: u64,
        render_fn: impl FnOnce(&mut egui::Ui),
    ) -> egui::Vec2 {
        let cache_key = (key.to_owned(), hash);
        let resp = egui::Frame::NONE.show(ui, render_fn);
        let size = resp.response.rect.size();
        self.cache.insert(cache_key, size);
        size
    }
}

// ── Fast content hash helper ───────────────────────────────────────

/// Compute a 64-bit content hash from any number of hashable values.
#[macro_export]
macro_rules! section_hash {
    ($($val:expr),+ $(,)?) => {{
        use std::hash::Hash as _;
        use std::hash::Hasher as _;
        let mut state = std::collections::hash_map::DefaultHasher::new();
        $(
            $val.hash(&mut state);
        )+
        state.finish()
    }};
}
