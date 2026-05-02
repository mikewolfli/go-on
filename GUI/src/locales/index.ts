import { createI18n } from "vue-i18n";
import enUS from "./en-US.json";
import zhCN from "./zh-CN.json";
import zhTW from "./zh-TW.json";

function safeGetItem(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function safeSetItem(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // localStorage unavailable (private browsing etc.) — silently ignore
  }
}

const KEY = "goon.gui.locale";

type Locale = "en-US" | "zh-CN" | "zh-TW";

function resolveInitialLocale(): Locale {
  const saved = safeGetItem(KEY);
  if (saved === "en-US" || saved === "zh-CN" || saved === "zh-TW") {
    return saved;
  }

  const lang = navigator.language.toLowerCase();
  if (lang === "zh-tw" || lang === "zh-hk" || lang === "zh-mo") {
    return "zh-TW";
  }
  if (lang.startsWith("zh")) {
    return "zh-CN";
  }
  return "en-US";
}

export const i18n = createI18n({
  legacy: false,
  locale: resolveInitialLocale(),
  fallbackLocale: "en-US",
  messages: {
    "en-US": enUS,
    "zh-CN": zhCN,
    "zh-TW": zhTW,
  },
});

export function setLocale(locale: Locale) {
  i18n.global.locale.value = locale;
  safeSetItem(KEY, locale);
}

export function getLocale(): Locale {
  return i18n.global.locale.value as Locale;
}
