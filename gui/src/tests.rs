use crate::config::AppConfig;
use crate::i18n::{I18n, Lang};
use crate::theme::Theme;

#[cfg(test)]
mod unit_tests {
    use super::*;

    // ── App config tests ──────────────────────────────────────────────

    #[test]
    fn test_app_config_defaults() {
        let cfg = AppConfig::default();
        // Default backend URL
        assert_eq!(cfg.backend_url, "http://127.0.0.1:8090");
        // Default language
        assert_eq!(cfg.language, "en");
        // Default theme (Chinese display name)
        assert_eq!(cfg.theme, "简约");
        // Default protocol mode — uses auto-detection by default
        assert_eq!(cfg.protocol_mode, "adaptive");
    }

    #[test]
    fn test_app_config_serialization_roundtrip() {
        let cfg = AppConfig::default();
        let json = serde_json::to_string_pretty(&cfg).expect("serialize AppConfig");
        let deserialized: AppConfig = serde_json::from_str(&json).expect("deserialize AppConfig");
        assert_eq!(cfg.backend_url, deserialized.backend_url);
        assert_eq!(cfg.language, deserialized.language);
        assert_eq!(cfg.theme, deserialized.theme);
        assert_eq!(cfg.protocol_mode, deserialized.protocol_mode);
        assert_eq!(
            cfg.ui_stability.backend_refresh_interval_secs,
            deserialized.ui_stability.backend_refresh_interval_secs
        );
    }

    #[test]
    fn test_enterprise_config_defaults() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.enterprise.active_environment, "dev");
        assert!(!cfg.enterprise.environments.is_empty());
        assert_eq!(cfg.enterprise.environments.len(), 3);
    }

    // ── I18n tests ────────────────────────────────────────────────────

    #[test]
    fn test_i18n_english_key_exists() {
        let i18n = I18n::new(Lang::En);
        // A well-known key should resolve to its English value
        let title = i18n.t("app.title");
        assert_eq!(title, "Go-On GUI", "expected English title");
        // A missing key should return the key itself
        let missing = i18n.t("nonexistent.key");
        assert_eq!(missing, "nonexistent.key");
    }

    #[test]
    fn test_i18n_simplified_chinese() {
        let i18n = I18n::new(Lang::ZhCn);
        assert_eq!(i18n.t("app.title"), "Go-On 图形界面");
        assert_eq!(i18n.t("app.start"), "启动");
        assert_eq!(i18n.t("app.stop"), "停止");
    }

    #[test]
    fn test_i18n_traditional_chinese() {
        let i18n = I18n::new(Lang::ZhTw);
        assert_eq!(i18n.t("app.title"), "Go-On 圖形界面");
        assert_eq!(i18n.t("app.start"), "啟動");
        assert_eq!(i18n.t("app.stop"), "停止");
    }

    #[test]
    fn test_i18n_switch_language() {
        let mut i18n = I18n::new(Lang::En);
        assert_eq!(i18n.t("app.title"), "Go-On GUI");
        i18n.switch(Lang::ZhCn);
        assert_eq!(i18n.t("app.title"), "Go-On 图形界面");
        i18n.switch(Lang::ZhTw);
        assert_eq!(i18n.t("app.title"), "Go-On 圖形界面");
    }

    #[test]
    fn test_i18n_all_keys_have_all_languages() {
        let en = I18n::new(Lang::En);
        let cn = I18n::new(Lang::ZhCn);
        let tw = I18n::new(Lang::ZhTw);

        // Known keys that should always have all three translations
        let known_keys = [
            "app.title",
            "app.start",
            "app.stop",
            "app.running",
            "app.stopped",
            "theme.minimal",
            "theme.guofeng",
            "theme.shanshui",
        ];

        for key in &known_keys {
            let en_val = en.t(key);
            let cn_val = cn.t(key);
            let tw_val = tw.t(key);
            // Each key should resolve to something other than the key itself
            assert_ne!(
                en_val.as_ref(),
                *key,
                "English translation missing for key: {key}"
            );
            assert_ne!(
                cn_val.as_ref(),
                *key,
                "Chinese translation missing for key: {key}"
            );
            assert_ne!(
                tw_val.as_ref(),
                *key,
                "Traditional Chinese translation missing for key: {key}"
            );
        }
    }

    #[test]
    fn test_i18n_format_strings() {
        let i18n = I18n::new(Lang::En);
        // Keys with format placeholders should not be rejected
        let exported = i18n.t("chat.exportedAt");
        assert!(exported.contains("{time}") || exported.contains("Exported"));
    }

    // ── Theme tests ───────────────────────────────────────────────────

    #[test]
    fn test_theme_from_name() {
        // English names
        assert!(matches!(Theme::from_name("minimal"), Theme::Minimal));
        assert!(matches!(Theme::from_name("guofeng"), Theme::GuoFeng));

        // Chinese names
        assert!(matches!(Theme::from_name("简约"), Theme::Minimal));
        assert!(matches!(Theme::from_name("国风"), Theme::GuoFeng));
        assert!(matches!(Theme::from_name("武侠"), Theme::Wuxia));
        assert!(matches!(Theme::from_name("山水"), Theme::ShanShui));
        assert!(matches!(
            Theme::from_name("为人民服务"),
            Theme::ServeThePeople
        ));

        // Special names
        assert!(matches!(Theme::from_name("Hello Kitty"), Theme::HelloKitty));
        assert!(matches!(Theme::from_name("hellokitty"), Theme::HelloKitty));

        // Unknown name defaults to Minimal
        assert!(matches!(Theme::from_name("unknown"), Theme::Minimal));
    }

    #[test]
    fn test_theme_all_returns_list() {
        let themes = Theme::all();
        assert!(!themes.is_empty(), "Theme list should not be empty");
        // Should include all defined themes
        let names: Vec<&str> = themes.iter().map(|(_, name)| *name).collect();
        assert!(names.contains(&"简约"));
        assert!(names.contains(&"国风"));
        assert!(names.contains(&"武侠"));
        assert!(names.contains(&"Hello Kitty"));
    }

    #[test]
    fn test_theme_display_name() {
        let i18n = I18n::new(Lang::En);
        let theme = Theme::Minimal;
        let name = theme.display_name(&i18n);
        assert_eq!(name, "Minimal");

        let i18n_cn = I18n::new(Lang::ZhCn);
        let name_cn = theme.display_name(&i18n_cn);
        assert_eq!(name_cn, "简约");
    }

    // ── Feature toggle tests ──────────────────────────────────────────

    #[test]
    fn test_feature_toggles_defaults() {
        let cfg = AppConfig::default();
        assert!(cfg.features.monitor, "Monitor should be enabled by default");
        assert!(cfg.features.chat, "Chat should be enabled by default");
        assert!(cfg.features.config, "Config should be enabled by default");
        assert!(cfg.features.skills, "Skills should be enabled by default");
        assert!(
            cfg.features.workflow,
            "Workflow should be enabled by default"
        );
        // These features may be disabled by default
        assert!(
            !cfg.features.autotune_chain_injection,
            "AutoTune chain injection should be disabled by default"
        );
    }

    // ── UI Stability tests ────────────────────────────────────────────

    #[test]
    fn test_ui_stability_defaults() {
        let cfg = AppConfig::default();
        assert!(cfg.ui_stability.backend_refresh_interval_secs > 0);
        assert!(cfg.ui_stability.backend_ui_commit_debounce_ms > 0);
        assert!(cfg.ui_stability.chat_repaint_interval_ms > 0);
        assert!(cfg.ui_stability.chat_max_pending_events_per_frame > 0);
    }

    // ── Language detection tests ──────────────────────────────────────

    #[test]
    fn test_detect_system_language_en() {
        // The function in app.rs checks env vars - we test Lang defaults
        let i18n = I18n::new(Lang::En);
        assert_eq!(i18n.lang, Lang::En);
    }

    #[test]
    fn test_lang_equality_and_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Lang::En);
        set.insert(Lang::ZhCn);
        set.insert(Lang::ZhTw);
        assert_eq!(set.len(), 3);
        assert!(set.contains(&Lang::En));
    }

    #[test]
    fn test_lang_debug_and_clone() {
        let lang = Lang::En;
        let cloned = lang;
        assert_eq!(format!("{:?}", lang), "En");
        assert_eq!(lang, cloned);
    }
}
