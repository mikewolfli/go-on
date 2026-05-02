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
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::{error, info, warn};

fn read_guard<'a, T>(lock: &'a RwLock<T>, label: &str) -> RwLockReadGuard<'a, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => {
            error!("{} poisoned during read; recovering state", label);
            poisoned.into_inner()
        }
    }
}

fn write_guard<'a, T>(lock: &'a RwLock<T>, label: &str) -> RwLockWriteGuard<'a, T> {
    match lock.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            error!("{} poisoned during write; recovering state", label);
            poisoned.into_inner()
        }
    }
}

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
    /// Get language code string
    pub fn code(&self) -> &'static str {
        match self {
            Language::ZhCN => "zh_CN",
            Language::ZhTW => "zh_TW",
            Language::EnUS => "en_US",
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

/// Global i18n manager
pub struct I18nManager {
    /// Current language
    current_language: Arc<RwLock<Language>>,
    /// Loaded translations (language -> message key -> content)
    translations: Arc<RwLock<HashMap<Language, HashMap<String, String>>>>,
    /// Path to language files directory
    languages_dir: PathBuf,
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
            current_language: Arc::new(RwLock::new(Language::detect_system())),
            translations: Arc::new(RwLock::new(HashMap::new())),
            languages_dir: dir,
        };

        // Load all available translations
        manager.load_all_languages()?;

        let current = *read_guard(&manager.current_language, "i18n.current_language");
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
    pub fn load_language(&self, language: Language) -> Result<()> {
        let file_path = self.languages_dir.join(format!("{}.json", language.code()));

        if !file_path.exists() {
            warn!("Language file not found: {:?}", file_path);
            return Ok(());
        }

        let content = fs::read_to_string(&file_path)
            .context(format!("Failed to read language file: {:?}", file_path))?;

        let translations_data: Translations = serde_json::from_str(&content)
            .context(format!("Failed to parse language file: {:?}", file_path))?;

        let mut translations = write_guard(&self.translations, "i18n.translations");
        translations.insert(language, translations_data.messages);

        info!(
            "Loaded language: {:?} with {} messages",
            language,
            translations[&language].len()
        );

        Ok(())
    }

    /// Set current language
    pub fn set_language(&self, language: Language) {
        let mut current = write_guard(&self.current_language, "i18n.current_language");
        *current = language;
        info!("Language changed to: {:?}", language);
    }

    /// Get current language
    pub fn current_language(&self) -> Language {
        *read_guard(&self.current_language, "i18n.current_language")
    }

    /// Get translated message
    ///
    /// # Arguments
    /// * `key` - Message key
    /// * `args` - Optional format arguments
    ///
    /// # Returns
    /// Translated message or key if not found
    pub fn get(&self, key: &str) -> String {
        let lang = self.current_language();
        self.get_lang(key, lang)
    }

    /// Get translated message for specific language
    pub fn get_lang(&self, key: &str, language: Language) -> String {
        let translations = read_guard(&self.translations, "i18n.translations");

        if let Some(lang_messages) = translations.get(&language) {
            if let Some(message) = lang_messages.get(key) {
                return message.clone();
            }
        }

        // Fallback to English if translation not found
        if language != Language::EnUS {
            if let Some(en_messages) = translations.get(&Language::EnUS) {
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

        for (placeholder, value) in format_args {
            message = message.replace(&format!("{{{}}}", placeholder), value);
        }

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
        let translations = read_guard(&self.translations, "i18n.translations");

        if let Some(en_messages) = translations.get(&Language::EnUS) {
            Ok(en_messages.keys().cloned().collect())
        } else {
            Ok(Vec::new())
        }
    }

    /// Get available languages
    pub fn available_languages(&self) -> Vec<(Language, usize)> {
        let translations = read_guard(&self.translations, "i18n.translations");

        let mut languages: Vec<_> = translations
            .iter()
            .map(|(lang, messages)| (*lang, messages.len()))
            .collect();

        languages.sort_by_key(|(lang, _)| lang.code());
        languages
    }
}

lazy_static::lazy_static! {
    /// Global i18n manager instance
    pub static ref I18N: Arc<RwLock<Option<I18nManager>>> = Arc::new(RwLock::new(None));
}

/// Initialize global i18n system
///
/// # Arguments
/// * `languages_dir` - Path to directory containing language JSON files
///
/// # Returns
/// Result indicating success
pub fn init_i18n<P: AsRef<Path>>(languages_dir: P) -> Result<()> {
    let manager = I18nManager::new(languages_dir)?;
    let mut i18n = write_guard(&I18N, "i18n.global");
    *i18n = Some(manager);
    Ok(())
}

/// Translate message using global i18n instance
pub fn t(key: &str) -> String {
    let i18n = read_guard(&I18N, "i18n.global");
    if let Some(manager) = i18n.as_ref() {
        manager.get(key)
    } else {
        key.to_string()
    }
}

/// Translate message with formatting
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    let i18n = read_guard(&I18N, "i18n.global");
    if let Some(manager) = i18n.as_ref() {
        manager.get_formatted(key, args)
    } else {
        key.to_string()
    }
}

/// Set global language
pub fn set_language(language: Language) {
    let i18n = read_guard(&I18N, "i18n.global");
    if let Some(manager) = i18n.as_ref() {
        manager.set_language(language);
    }
}

/// Get current global language
pub fn current_language() -> Language {
    let i18n = read_guard(&I18N, "i18n.global");
    if let Some(manager) = i18n.as_ref() {
        manager.current_language()
    } else {
        Language::EnUS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::path::PathBuf;

    #[test]
    fn test_language_detection() {
        let en = Language::from_code("en");
        assert_eq!(en, Language::EnUS);

        let zh = Language::from_code("zh_cn");
        assert_eq!(zh, Language::ZhCN);

        let tw = Language::from_code("zh_tw");
        assert_eq!(tw, Language::ZhTW);
    }

    #[test]
    fn test_language_code() {
        assert_eq!(Language::ZhCN.code(), "zh_CN");
        assert_eq!(Language::ZhTW.code(), "zh_TW");
        assert_eq!(Language::EnUS.code(), "en_US");
    }

    #[test]
    fn test_fallback_to_english() {
        let unknown = Language::from_code("unknown_lang");
        assert_eq!(unknown, Language::EnUS);
    }

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

        for lang in ["en_US", "zh_CN", "zh_TW"] {
            let path = root.join("languages").join(format!("{}.json", lang));
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
