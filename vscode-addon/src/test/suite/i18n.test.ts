import * as assert from "node:assert";
import { i18n, t, MessageKeys } from "../../i18n";

suite("i18n", () => {
  suite("I18nManager", () => {
    test("getInstance returns a singleton", () => {
      // The singleton is created at module load time
      assert.ok(i18n, "i18n instance should be defined");
    });

    test("getCurrentLanguage returns a language string", () => {
      const lang = i18n.getCurrentLanguage();
      assert.ok(
        lang === "en_US" || lang === "zh_CN" || lang === "zh_TW",
        `unexpected language: ${lang}`,
      );
    });

    test("getLanguageCodeForApp returns current language", () => {
      const appCode = i18n.getLanguageCodeForApp();
      assert.strictEqual(appCode, i18n.getCurrentLanguage());
    });
  });

  suite("locale loading", () => {
    test("loadLocale changes current language", () => {
      const originalLang = i18n.getCurrentLanguage();
      i18n.loadLocale("zh_CN");
      assert.strictEqual(i18n.getCurrentLanguage(), "zh_CN");
      // Restore
      i18n.loadLocale(originalLang);
    });

    test("loadLocale to en_US works", () => {
      i18n.loadLocale("en_US");
      assert.strictEqual(i18n.getCurrentLanguage(), "en_US");
    });

    test("loadLocale to zh_TW works", () => {
      i18n.loadLocale("zh_TW");
      assert.strictEqual(i18n.getCurrentLanguage(), "zh_TW");
      i18n.loadLocale("en_US");
    });

    test("reloadMessages reloads the message table", () => {
      i18n.reloadMessages();
      // reloadMessages() re-reads the locale file; the message table must
      // remain functional afterwards (not just "not throw").
      assert.strictEqual(i18n.getCurrentLanguage(), "en_US");
      assert.strictEqual(t("general.goOn"), "Go-On");
      assert.strictEqual(t("messages.goOnStarted"), "Go-On proxy started.");
    });

    test("setLanguage changes language and reloads", () => {
      i18n.setLanguage("en_US");
      assert.strictEqual(i18n.getCurrentLanguage(), "en_US");
    });
  });

  suite("getMessage / t() - key lookup", () => {
    setup(() => {
      // Ensure English locale for predictable test results
      i18n.setLanguage("en_US");
    });

    test("t() returns translated string for a known general key", () => {
      const result = t("general.goOn");
      assert.strictEqual(result, "Go-On");
    });

    test("t() returns translated string for a known settings key", () => {
      const result = t("general.settings");
      assert.strictEqual(result, "Settings");
    });

    test("t() returns translated string for commands", () => {
      const result = t("commands.start.title");
      assert.strictEqual(result, "Start Go-On Proxy");
    });

    test("t() returns translated string for messages", () => {
      const result = t("messages.goOnStarted");
      assert.strictEqual(result, "Go-On proxy started.");
    });

    test("t() returns key when message is not found", () => {
      // getFallbackValue returns the key itself when nothing matches
      const result = t("nonexistent.key.path");
      assert.strictEqual(result, "nonexistent.key.path");
    });

    test("t() returns key for deeply nested missing key", () => {
      const result = t("general.nonexistent");
      assert.strictEqual(result, "general.nonexistent");
    });

    test("getMessage returns key for missing key", () => {
      const result = i18n.getMessage("does.not.exist");
      assert.strictEqual(result, "does.not.exist");
    });
  });

  suite("parameter substitution", () => {
    setup(() => {
      i18n.setLanguage("en_US");
    });

    test("t() with single parameter substitutes {0}", () => {
      // messages.goOnStartFailed = "Failed to start Go-On: {0}"
      const result = t("messages.goOnStartFailed", "timeout");
      assert.ok(
        result.includes("timeout") && result.includes("Failed to start Go-On"),
        `result should contain substitution: ${result}`,
      );
      assert.ok(!result.includes("{0}"), "placeholder must be substituted");
    });

    test("t() with numeric parameters", () => {
      // getMessage handles params generically via {0}, {1} etc.
      const result = i18n.getMessage("general.testParam", "hello");
      // If the key doesn't exist, it falls back and returns the key
      assert.strictEqual(result, "general.testParam");
    });
  });

  suite("fallback behavior", () => {
    test("getMessage falls back to hardcoded messages for known keys", () => {
      // Even without locale files, hardcoded fallbacks should work
      i18n.setLanguage("en_US");
      const result = i18n.getMessage("general.goOn");
      assert.strictEqual(result, "Go-On");
    });

    test("getMessage falls back for commands section", () => {
      i18n.setLanguage("en_US");
      const result = i18n.getMessage("commands.stop.title");
      assert.strictEqual(result, "Stop Go-On Proxy");
    });

    test("getMessage returns key for completely unknown paths", () => {
      const result = i18n.getMessage("completely.unknown.path");
      assert.strictEqual(result, "completely.unknown.path");
    });

    test("getMessage handles non-leaf (object) values by returning key", () => {
      // "commands" is an object, not a leaf string
      const result = i18n.getMessage("commands");
      assert.strictEqual(result, "commands");
    });
  });

  suite("fallback messages content", () => {
    setup(() => {
      i18n.setLanguage("en_US");
    });

    test("hardcoded fallback messages have general section", () => {
      assert.strictEqual(t("general.goOn"), "Go-On");
      assert.strictEqual(t("general.start"), "Start");
      assert.strictEqual(t("general.stop"), "Stop");
      assert.strictEqual(t("general.ok"), "OK");
      assert.strictEqual(t("general.close"), "Close");
      assert.strictEqual(t("general.error"), "Error");
    });

    test("hardcoded fallback messages have commands section", () => {
      assert.strictEqual(t("commands.start.title"), "Start Go-On Proxy");
      assert.strictEqual(t("commands.stop.title"), "Stop Go-On Proxy");
      assert.strictEqual(t("commands.openChat.title"), "Open Go-On Chat");
    });

    test("hardcoded fallback messages have messages section", () => {
      assert.strictEqual(t("messages.successfullySaved"), "Successfully saved");
      assert.strictEqual(t("messages.goOnStarted"), "Go-On proxy started.");
      assert.strictEqual(t("messages.goOnStopped"), "Go-On proxy stopped.");
    });

    test("hardcoded fallback includes workflow section", () => {
      assert.strictEqual(
        t("workflow.createNewWorkflow"),
        "Create New Workflow",
      );
    });

    test("hardcoded fallback includes processFlow section", () => {
      assert.strictEqual(
        t("processFlow.noProcessSelected"),
        "No Process Selected",
      );
      assert.strictEqual(t("processFlow.createProcess"), "Create");
      assert.strictEqual(t("processFlow.runProcess"), "Run");
    });

    test("empty sections return empty objects (non-leaf -> key)", () => {
      const result = t("runtime");
      assert.strictEqual(result, "runtime");
    });
  });

  suite("MessageKeys enum", () => {
    test("MessageKeys has general keys defined", () => {
      assert.ok(MessageKeys.goOn, "goOn should be defined");
      assert.ok(MessageKeys.settings, "settings should be defined");
      assert.ok(MessageKeys.start, "start should be defined");
      assert.ok(MessageKeys.stop, "stop should be defined");
    });

    test("MessageKeys has command-related keys", () => {
      assert.ok(
        MessageKeys.startGoOn,
        "startGoOn command key should be defined",
      );
      assert.ok(MessageKeys.stopGoOn, "stopGoOn command key should be defined");
    });

    test("MessageKeys has i18n language keys", () => {
      assert.strictEqual(MessageKeys.language, "language.language");
      assert.strictEqual(
        MessageKeys.simplifiedChinese,
        "language.simplifiedChinese",
      );
      assert.strictEqual(
        MessageKeys.traditionalChinese,
        "language.traditionalChinese",
      );
      assert.strictEqual(MessageKeys.english, "language.english");
    });
  });

  suite("language detection helpers (implied)", () => {
    test("current language round-trips via setLanguage/loadLocale", () => {
      const langs = ["en_US", "zh_CN", "zh_TW"] as const;
      for (const lang of langs) {
        i18n.setLanguage(lang);
        assert.strictEqual(i18n.getCurrentLanguage(), lang);
      }
    });
  });
});
