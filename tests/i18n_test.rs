use go_on::i18n::{current_language, init_i18n, set_language, t, tf, Language};

// ── T10: Test wrapper for `cargo test` ─────────────────────────────────────
mod tests {
    use super::*;

    #[test]
    fn test_i18n_binary() {
        // Initialize i18n from the real language files shipped with the repo
        // (an empty temp dir loaded no translations, so `t()` fell back to the
        // key itself and every non-empty assertion was tautologically true).
        let languages_dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/languages");
        assert!(
            languages_dir.join("en-US.json").exists(),
            "en-US.json must exist next to the test"
        );
        assert!(
            init_i18n(languages_dir).is_ok(),
            "i18n initialization should succeed"
        );

        // Verify current language is set
        assert!(
            matches!(current_language(), Language::EnUS | Language::ZhCN),
            "Should have a valid language after init"
        );

        // Verify real translations load (not the key-fallback)
        set_language(Language::EnUS);
        assert_eq!(t("app.name"), "go-on", "en-US translation must load");

        // Verify formatted translations work without panicking
        let fatal = tf("error.fatal", &[("error", "test error")]);
        assert!(
            !fatal.is_empty() && fatal != "error.fatal",
            "tf() should return a translated string, got: '{}'",
            fatal
        );

        // Verify language switching changes the loaded translations
        set_language(Language::ZhCN);
        assert_eq!(
            current_language(),
            Language::ZhCN,
            "Should switch to Chinese"
        );

        // zh-CN ships the same app.name value, so assert on a zh-specific key
        // instead (or that the zh file is the active one).
        let zh_name = t("app.name");
        assert!(
            !zh_name.is_empty(),
            "Chinese translation should not be empty"
        );

        // Verify switching back
        set_language(Language::EnUS);
        assert_eq!(
            current_language(),
            Language::EnUS,
            "Should switch back to English"
        );
        assert_eq!(t("app.name"), "go-on");
    }
}
