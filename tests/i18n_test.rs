use go_on::i18n::{current_language, init_i18n, set_language, t, tf, Language};

// ── T10: Test wrapper for `cargo test` ─────────────────────────────────────
mod tests {
    use super::*;

    #[test]
    fn test_i18n_binary() {
        // Initialize i18n with a temp directory
        let languages_dir = std::env::temp_dir().join("go-on-i18n-test");
        assert!(
            init_i18n(languages_dir).is_ok(),
            "i18n initialization should succeed"
        );

        // Verify current language is set
        assert!(
            matches!(current_language(), Language::EnUS | Language::ZhCN),
            "Should have a valid language after init"
        );

        // Verify basic translations return non-empty strings
        let app_name = t("app.name");
        assert!(
            !app_name.is_empty(),
            "app.name translation should not be empty"
        );

        // Verify formatted translations work without panicking
        let fatal = tf("error.fatal", &[("error", "test error")]);
        assert!(
            !fatal.is_empty(),
            "tf() should return a non-empty string, got: '{}'",
            fatal
        );

        // Verify language switching works
        set_language(Language::ZhCN);
        assert_eq!(
            current_language(),
            Language::ZhCN,
            "Should switch to Chinese"
        );

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
        let en_name = t("app.name");
        assert!(
            !en_name.is_empty(),
            "English translation should not be empty"
        );
    }
}
