import { ref, watch } from "vue";

export type Theme = "light" | "dark";

const THEME_KEY = "go-on-gui-theme";

function getSystemTheme(): Theme {
    if (typeof window === "undefined") return "light";
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export const currentTheme = ref<Theme>(getTheme());

function getTheme(): Theme {
    const stored = localStorage.getItem(THEME_KEY);
    if (stored === "dark" || stored === "light") {
        return stored;
    }
    return getSystemTheme();
}

export function setTheme(theme: Theme) {
    currentTheme.value = theme;
    localStorage.setItem(THEME_KEY, theme);
    applyTheme(theme);
}

export function toggleTheme() {
    const nextTheme = currentTheme.value === "light" ? "dark" : "light";
    setTheme(nextTheme);
}

function applyTheme(theme: Theme) {
    const html = document.documentElement;
    if (theme === "dark") {
        html.classList.add("dark");
        html.style.colorScheme = "dark";
    } else {
        html.classList.remove("dark");
        html.style.colorScheme = "light";
    }
}

// Apply theme on initialization
applyTheme(currentTheme.value);

// Watch for system theme changes
if (typeof window !== "undefined") {
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    mediaQuery.addEventListener("change", (e) => {
        if (localStorage.getItem(THEME_KEY) === null) {
            setTheme(e.matches ? "dark" : "light");
        }
    });
}
