//! Internationalization (i18n) System
//!
//! Supports multiple languages (Simplified Chinese, Traditional Chinese, English)
//! with automatic system language detection and hot-reloading capabilities.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::{debug, info, warn};

fn read_guard<'a, T>(lock: &'a RwLock<T>, _label: &str) -> RwLockReadGuard<'a, T> {
    crate::read_or_recover!(lock)
}

fn write_guard<'a, T>(lock: &'a RwLock<T>, _label: &str) -> RwLockWriteGuard<'a, T> {
    crate::write_or_recover!(lock)
}

/// Sentinel used during brace-escaping in formatted messages.
const ESCAPED_SENTINEL: &str = "\x00ESCAPED_BRACE\x00";

/// Language enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    /// Simplified Chinese
    #[serde(rename = "zh_CN")]
    ZhCN,
    /// Traditional Chinese
    #[serde(rename = "zh_TW")]
    ZhTW,
    /// English
    #[serde(rename = "en_US")]
    EnUS,
}

impl Language {
    /// Get language code string (underscore format for internal use)
    pub fn code(&self) -> &'static str {
        match self {
            Language::ZhCN => "zh_CN",
            Language::ZhTW => "zh_TW",
            Language::EnUS => "en_US",
        }
    }

    /// Get BCP 47 language code string (hyphen format for file naming)
    pub fn bcp47_code(&self) -> &'static str {
        match self {
            Language::ZhCN => "zh-CN",
            Language::ZhTW => "zh-TW",
            Language::EnUS => "en-US",
        }
    }

    /// Detect system language
    pub fn detect_system() -> Self {
        let locale = std::env::var("LANG")
            .or_else(|_| std::env::var("LANGUAGE"))
            .or_else(|_| std::env::var("LC_ALL"))
            .unwrap_or_default()
            .to_lowercase();

        if locale.contains("zh_cn") || locale.contains("zh-cn") {
            Language::ZhCN
        } else if locale.contains("zh_tw") || locale.contains("zh-tw") || locale.contains("zh_hk") {
            Language::ZhTW
        } else if locale.contains("en") {
            Language::EnUS
        } else {
            // Default to English if system language not recognized
            Language::EnUS
        }
    }

    /// Parse from string
    pub fn from_code(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "zh_cn" | "zh-cn" | "chinese" | "simplified" => Language::ZhCN,
            "zh_tw" | "zh-tw" | "traditional" | "taiwanese" => Language::ZhTW,
            "en" | "en_us" | "en-us" | "english" => Language::EnUS,
            _ => Language::EnUS, // Default fallback
        }
    }
}

impl FromStr for Language {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Self::from_code(s))
    }
}

/// Language translations for a single language
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Translations {
    /// Language code
    pub language: String,
    /// Map of message keys to content
    pub messages: HashMap<String, String>,
}

/// Internal i18n state held behind a single lock
#[derive(Clone)]
struct I18nState {
    /// Current language
    current_language: Language,
    /// Loaded translations (language -> message key -> content)
    translations: HashMap<Language, HashMap<String, String>>,
}

/// Global i18n manager
pub struct I18nManager {
    /// Internal state behind Arc for cheap cloning
    inner: Arc<I18nInner>,
}

struct I18nInner {
    /// Translation state behind a single lock
    state: RwLock<I18nState>,
    /// Path to language files directory
    languages_dir: PathBuf,
}

impl Clone for I18nManager {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl I18nManager {
    /// Create new i18n manager
    ///
    /// # Arguments
    /// * `languages_dir` - Path to directory containing language JSON files
    ///
    /// # Returns
    /// Result with initialized I18nManager
    pub fn new<P: AsRef<Path>>(languages_dir: P) -> Result<Self> {
        let dir = languages_dir.as_ref().to_path_buf();

        if !dir.exists() {
            fs::create_dir_all(&dir).context("Failed to create languages directory")?;
        }

        let manager = I18nManager {
            inner: Arc::new(I18nInner {
                state: RwLock::new(I18nState {
                    current_language: Language::detect_system(),
                    translations: HashMap::new(),
                }),
                languages_dir: dir,
            }),
        };

        // Load only the detected language at startup.
        // Other languages are loaded on demand (via set_language / get_lang).
        let current = manager.current_language();
        if let Err(e) = manager.load_language(current) {
            warn!("Failed to load initial language {:?}: {}", current, e);
        }
        info!("i18n initialized with language: {:?}", current);

        Ok(manager)
    }

    /// Load all language files from directory
    pub fn load_all_languages(&self) -> Result<()> {
        let languages = vec![Language::ZhCN, Language::ZhTW, Language::EnUS];

        for lang in languages {
            if let Err(e) = self.load_language(lang) {
                warn!("Failed to load language {:?}: {}", lang, e);
            }
        }

        Ok(())
    }

    /// Load specific language file
    ///
    /// Tries BCP 47 (hyphen) filename first, falls back to underscore for backward compat.
    pub fn load_language(&self, language: Language) -> Result<()> {
        // Try BCP 47 hyphen format first (e.g., zh-CN.json)
        let bcp47_path = self
            .inner
            .languages_dir
            .join(format!("{}.json", language.bcp47_code()));
        let underscore_path = self
            .inner
            .languages_dir
            .join(format!("{}.json", language.code()));

        let file_path = if bcp47_path.exists() {
            bcp47_path
        } else {
            underscore_path
        };

        let content = match fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("Language file not found: {:?}", file_path);
                return Ok(());
            }
            Err(e) => {
                return Err(e).context(format!("Failed to read language file: {:?}", file_path))
            }
        };

        let translations_data: Translations = serde_json::from_str(&content)
            .context(format!("Failed to parse language file: {:?}", file_path))?;

        let mut state = write_guard(&self.inner.state, "i18n.state");
        state
            .translations
            .insert(language, translations_data.messages);

        info!(
            "Loaded language: {:?} with {} messages",
            language,
            state.translations[&language].len()
        );

        Ok(())
    }

    /// Set current language, lazily loading the translations if not yet loaded.
    pub fn set_language(&self, language: Language) {
        // Ensure translations are loaded for the new language.
        {
            let state = read_guard(&self.inner.state, "i18n.state");
            if !state.translations.contains_key(&language) {
                drop(state);
                if let Err(e) = self.load_language(language) {
                    warn!(
                        "Failed to load language {:?} in set_language: {}",
                        language, e
                    );
                }
            }
        }
        let mut state = write_guard(&self.inner.state, "i18n.state");
        state.current_language = language;
        info!("Language changed to: {:?}", language);
    }

    /// Get current language
    pub fn current_language(&self) -> Language {
        let state = read_guard(&self.inner.state, "i18n.state");
        state.current_language
    }

    /// Get translated message for the current language.
    ///
    /// Delegates to the shared `lookup()` which implements the canonical
    /// lookup chain: requested language → English fallback → return key.
    pub fn get(&self, key: &str) -> String {
        let state = read_guard(&self.inner.state, "i18n.state");
        Self::lookup(&state, key, state.current_language)
    }

    /// Get translated message for specific language
    pub fn get_lang(&self, key: &str, language: Language) -> String {
        // Lazily load the language if not yet loaded.
        {
            let state = read_guard(&self.inner.state, "i18n.state");
            if !state.translations.contains_key(&language) {
                drop(state);
                if let Err(e) = self.load_language(language) {
                    warn!(
                        "Failed to lazy-load language {:?} in get_lang: {}",
                        language, e
                    );
                }
            }
        }
        let state = read_guard(&self.inner.state, "i18n.state");
        Self::lookup(&state, key, language)
    }

    /// Look up a translation from a pre-acquired state guard (single lock acquisition).
    fn lookup(state: &I18nState, key: &str, language: Language) -> String {
        if let Some(lang_messages) = state.translations.get(&language) {
            if let Some(message) = lang_messages.get(key) {
                return message.clone();
            }
        }

        // Fallback to English if translation not found
        if language != Language::EnUS {
            if let Some(en_messages) = state.translations.get(&Language::EnUS) {
                if let Some(message) = en_messages.get(key) {
                    return message.clone();
                }
            }
        }

        // Return key if no translation found
        key.to_string()
    }

    /// Get translated message with format arguments
    pub fn get_formatted(&self, key: &str, format_args: &[(&str, &str)]) -> String {
        let mut message = self.get(key);

        // Escape {{ → sentinel so literal {name} is not substituted
        message = message.replace("{{", ESCAPED_SENTINEL);

        for (placeholder, value) in format_args {
            message = message.replace(&format!("{{{}}}", placeholder), value);
        }

        // Restore sentinel → {
        message = message.replace(ESCAPED_SENTINEL, "{");

        message
    }

    /// Hot reload language files (monitors for changes)
    pub fn hot_reload(&self) -> Result<()> {
        self.load_all_languages()?;
        info!("Languages reloaded");
        Ok(())
    }

    /// Export translatable keys (for translation work)
    pub fn export_keys(&self) -> Result<Vec<String>> {
        let state = read_guard(&self.inner.state, "i18n.state");
        if let Some(en_messages) = state.translations.get(&Language::EnUS) {
            Ok(en_messages.keys().cloned().collect())
        } else {
            Ok(Vec::new())
        }
    }

    /// Get available languages
    pub fn available_languages(&self) -> Vec<(Language, usize)> {
        let state = read_guard(&self.inner.state, "i18n.state");
        let mut languages: Vec<_> = state
            .translations
            .iter()
            .map(|(lang, messages)| (*lang, messages.len()))
            .collect();

        languages.sort_by_key(|(lang, _)| lang.code());
        languages
    }
}

/// Global i18n manager instance
pub static I18N: OnceLock<I18nManager> = OnceLock::new();

/// Initialize global i18n system
///
/// # Arguments
/// * `languages_dir` - Path to directory containing language JSON files
///
/// # Returns
/// Result indicating success
pub fn init_i18n<P: AsRef<Path>>(languages_dir: P) -> Result<()> {
    let manager = I18nManager::new(languages_dir)?;
    I18N.set(manager)
        .map_err(|_| anyhow::anyhow!("i18n already initialized"))?;
    Ok(())
}

/// Translate message using global i18n instance.
///
/// Acquires the I18N read lock once, then performs the full lookup (current
/// language + English fallback) under a single `manager.state` read lock.
pub fn t(key: &str) -> String {
    match I18N.get() {
        Some(manager) => {
            let state = read_guard(&manager.inner.state, "i18n.state");
            I18nManager::lookup(&state, key, state.current_language)
        }
        None => key.to_string(),
    }
}

/// Translate message with formatting
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    let template = t(key);

    let mut message = template;
    // Escape {{ → sentinel so literal {name} is not substituted
    message = message.replace("{{", ESCAPED_SENTINEL);
    for (placeholder, value) in args {
        message = message.replace(&format!("{{{}}}", placeholder), value);
    }
    // Restore sentinel → {
    message = message.replace(ESCAPED_SENTINEL, "{");
    message
}

/// Set global language
pub fn set_language(language: Language) {
    if let Some(manager) = I18N.get() {
        manager.set_language(language);
    }
}

/// Get current global language
pub fn current_language() -> Language {
    I18N.get()
        .map(|manager| manager.current_language())
        .unwrap_or(Language::EnUS)
}

#[cfg(test)]
mod tests {

    use serde_json::Value;
    use std::path::PathBuf;

    #[test]
    fn onboarding_and_status_keys_exist_in_all_languages() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let required = [
            "status.secret_line",
            "status.recommended_item",
            "setup.onboarding_intro",
            "setup.onboarding_option_1",
            "setup.onboarding_option_2",
            "setup.onboarding_option_3",
            "setup.onboarding_select",
            "setup.onboarding_done_next",
            "setup.onboarding_skipped",
            "setup.onboarding_next",
        ];

        for lang in ["en-US", "zh-CN", "zh-TW"] {
            let path = root
                .join("config")
                .join("languages")
                .join(format!("{}.json", lang));
            let raw = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
            let json: Value = serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("failed to parse {}: {}", path.display(), e));
            let messages = json
                .get("messages")
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("messages object missing in {}", path.display()));

            for key in required {
                assert!(
                    messages.contains_key(key),
                    "missing key '{}' in {}",
                    key,
                    path.display()
                );
            }
        }
    }
}
