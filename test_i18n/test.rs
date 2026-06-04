use go_on::i18n::{current_language, init_i18n, set_language, t, tf, Language};

fn run_tests() {
    // 初始化i18n系统
    let languages_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("languages");
    if let Err(e) = init_i18n(languages_dir) {
        eprintln!("Failed to initialize i18n: {}", e);
        return;
    }

    println!("=== i18n System Test ===");
    println!("Current language: {:?}", current_language());

    // 测试基本翻译
    println!("\n=== Basic Translations ===");
    println!("App name: {}", t("app.name"));
    println!("App description: {}", t("app.description"));

    // 测试带参数的翻译
    println!("\n=== Formatted Translations ===");
    println!(
        "Fatal error: {}",
        tf("error.fatal", &[("error", "test error")])
    );
    println!(
        "Legacy health score: {}",
        tf(
            "ui.legacy_health_score",
            &[
                ("score", "85"),
                ("critical", "2"),
                ("warn", "5"),
                ("info", "10"),
            ]
        )
    );

    // 测试错误消息
    println!("\n=== Error Messages ===");
    println!(
        "Invalid setup profile: {}",
        tf("error.invalid_setup_profile", &[("value", "invalid")])
    );
    println!(
        "Keyring open error: {}",
        tf("error.keyring_open", &[("error", "permission denied")])
    );
    println!(
        "Storage key empty: {}",
        tf("error.storage_key_empty", &[("field", "username")])
    );
    println!(
        "Storage key too long: {}",
        tf(
            "error.storage_key_too_long",
            &[("field", "api_key"), ("max_len", "256"),]
        )
    );

    // 测试咨询工作流消息
    println!("\n=== Consultation Messages ===");
    println!(
        "Lead prompt: {}",
        tf(
            "consultation.lead_prompt",
            &[
                ("task", "Implement user authentication"),
                ("trigger", "security requirements"),
            ]
        )
    );

    println!(
        "Reviewer alternative prompt: {}",
        tf(
            "consultation.reviewer_alternative_prompt",
            &[
                ("task", "Implement user authentication"),
                ("role", "security expert"),
            ]
        )
    );

    // 测试Blue5原因
    println!("\n=== Blue5 Reasons ===");
    println!(
        "Requirement clarification: {}",
        t("blue5.reason.requirement_clarification")
    );
    println!("Task complexity: {}", t("blue5.reason.task_complexity"));

    // 测试切换语言
    println!("\n=== Language Switching ===");
    println!(
        "Before switching - Current language: {:?}",
        current_language()
    );
    println!("App name (current): {}", t("app.name"));

    // 切换到中文
    set_language(Language::ZhCN);
    println!(
        "\nAfter switching to Chinese - Current language: {:?}",
        current_language()
    );
    println!("App name (Chinese): {}", t("app.name"));
    println!("App description (Chinese): {}", t("app.description"));

    // 切换回英文
    set_language(Language::EnUS);
    println!(
        "\nAfter switching back to English - Current language: {:?}",
        current_language()
    );
    println!("App name (English): {}", t("app.name"));

    // 测试信息消息
    println!("\n=== Info Messages ===");
    println!(
        "Config written: {}",
        tf("info.config_written", &[("path", "/etc/go-on/config.toml")])
    );
    println!("Resources reloaded: {}", t("info.resources_reloaded"));

    // 测试警告消息
    println!("\n=== Warning Messages ===");
    println!(
        "Invalid value: {}",
        tf("warning.invalid_value", &[("allowed", "yes, no, maybe")])
    );

    println!("\n=== Test Completed Successfully ===");
}

fn main() {
    run_tests();
}

// ── T10: Test wrapper for `cargo test` ─────────────────────────────────────
// This allows running the i18n binary test as a standard `cargo test` target.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_i18n_binary() {
        // Run the same logic as the standalone binary.  In a test context
        // we just verify it doesn't panic.
        run_tests();
    }
}
