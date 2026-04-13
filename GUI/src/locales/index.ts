import { createI18n } from "vue-i18n";
import enUS from "./en-US.json";
import zhCN from "./zh-CN.json";

const KEY = "goon.gui.locale";

function resolveInitialLocale(): "en-US" | "zh-CN" {
    const saved = localStorage.getItem(KEY);
    if (saved === "en-US" || saved === "zh-CN") {
        return saved;
    }

    const lang = navigator.language.toLowerCase();
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
    },
});

export function setLocale(locale: "en-US" | "zh-CN") {
    i18n.global.locale.value = locale;
    localStorage.setItem(KEY, locale);
}

export function getLocale(): "en-US" | "zh-CN" {
    return i18n.global.locale.value as "en-US" | "zh-CN";
}
