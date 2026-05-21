/// Widget-level cache metadata.
///
/// NOTE:
/// egui/eframe is an immediate-mode UI: widgets must be rebuilt every
/// frame that repaints. Skipping rebuild and only reserving layout space
/// causes missing draw commands (blank/white areas). Therefore this cache
/// stores metadata only and does not short-circuit rendering.
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

// ── Section ID ─────────────────────────────────────────────────────

/// Identifies a sub-region within any view.  Used as the key in
/// [`SectionCache`] to decide whether that region's content has
/// changed since the last frame.
///
/// String-based variants (`View`, `Dialog`, `Frame`) let callers
/// create ad-hoc cache slots without modifying this enum.
///
/// Many variants are reserved for future use; `#[allow(dead_code)]`
/// suppresses the warning so CI (`-D warnings`) passes.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    // ── Existing variants ────────────────────────────────────
    Sidebar,
    SidebarSession(usize),
    Toolbar,
    Messages,
    SymbolCount(usize),

    // ── Tab-level ────────────────────────────────────────────
    TabContent,

    // ── Settings view sections ───────────────────────────────
    SettingsCoreGrid,
    SettingsAdvancedGrid,
    SettingsSystemGrid,
    SettingsStabilityGrid,
    SettingsLanguage,
    SettingsTheme,
    SettingsEnterprise,
    SettingsBackendUrl,

    // ── Monitor view sections ────────────────────────────────
    MonitorHealthCard,
    MonitorProviderList,
    MonitorTrends,
    MonitorErrors,

    // ── Providers view sections ──────────────────────────────
    ProvidersAddNew,
    ProvidersSavedList,

    // ── Chat view sections ───────────────────────────────────
    ChatModeRow,
    ChatMessages,
    ChatInputArea,
    ChatModelPicker,
    ChatPromptBrowser,

    // ── Skills view sections ─────────────────────────────────
    SkillsCreateDialog,
    SkillsImportDialog,
    SkillsEditor,
    SkillsDefaultCreator,

    // ── Workflow view sections ───────────────────────────────
    WorkflowStepEditor,
    WorkflowRunList,
    WorkflowRunDetail,

    // ── Prompts view sections ────────────────────────────────
    PromptsToolbar,
    PromptsCategories,
    PromptsTemplateList,
    PromptsDetail,
    PromptsCreateDialog,

    // ── Skills view sections ────────────────────────────────
    SkillsList,

    // ── Generic views ────────────────────────────────────────
    AboutFrame,
    SecurityPrefs,
    ConfigEditor,
    AutotuneParams,

    // ── Generic dynamic ──────────────────────────────────────
    View(String),
    Dialog(String),
    Frame(String),
}

// ── Cached size entry ──────────────────────────────────────────────

#[allow(dead_code)]
struct CacheEntry {
    hash: u64,
    size: egui::Vec2,
}

// ── Section cache ──────────────────────────────────────────────────

pub struct SectionCache {
    entries: HashMap<Section, CacheEntry>,
}

impl SectionCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Always returns `None`.
    ///
    /// Historical code paths used this to skip rendering on hash hit, which
    /// is unsafe in immediate-mode rendering. Keeping the method preserves API
    /// compatibility while forcing callers to render every frame.
    pub fn check(&self, _section: &Section, _hash: u64) -> Option<egui::Vec2> {
        None
    }

    /// Store the rendered size for a section + content hash.
    pub fn store(&mut self, section: &Section, hash: u64, rendered_size: egui::Vec2) {
        self.entries.insert(
            section.clone(),
            CacheEntry {
                hash,
                size: rendered_size,
            },
        );
    }

    /// Remove a section from the cache (force re-render).
    #[allow(dead_code)]
    pub fn invalidate(&mut self, section: Section) {
        self.entries.remove(&section);
    }

    /// Clear all cached entries.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

// ── CachedView: simple key-based sub-region cache ──────────────────

/// A simpler, string-keyed sub-region cache.
///
/// Unlike [`SectionCache`] (which uses the typed [`Section`] enum),
/// `CachedView` accepts a dynamic `(key, hash)` pair so that callers
/// can cache *any* sub-region without adding a variant to the enum.
///
/// # Usage
///
/// ```ignore
/// let mut cached_view = CachedView::new();
///
/// let size = cached_view.check_or_render(ui, "my_region", my_hash, |ui| {
///     ui.label("expensively computed content");
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

// ── Fast content hash helpers ──────────────────────────────────────

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

/// Compute a 64-bit hash from a string slice.
///
/// Convenience wrapper — equivalent to `section_hash!(my_str)`.
#[allow(dead_code)]
pub fn hash_str(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Compute a 64-bit hash from a boolean (yields `0u64` or `1u64`).
#[allow(dead_code)]
pub fn hash_bool(b: bool) -> u64 {
    if b {
        1
    } else {
        0
    }
}

/// Combine two 64-bit hashes into one.
///
/// Uses the same mixing strategy as `boost::hash_combine` to spread
/// both hash values across the output range.
#[allow(dead_code)]
pub fn hash_combine(h1: u64, h2: u64) -> u64 {
    // 0x9e3779b97f4a7c15 is the golden-ratio reciprocal for 64-bit.
    h1.wrapping_mul(0x9e3779b97f4a7c15)
        .wrapping_add(h2)
        .rotate_left(31)
}
