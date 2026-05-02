import { ref } from "vue";

export type Theme = "default" | "meadow" | "ink" | "wuxia" | "kitty";

export interface ThemeMeta {
  value: Theme;
  labelKey: string;
}

const THEME_LIST: ThemeMeta[] = [
  { value: "default", labelKey: "theme.default" },
  { value: "meadow", labelKey: "theme.meadow" },
  { value: "ink", labelKey: "theme.ink" },
  { value: "wuxia", labelKey: "theme.wuxia" },
  { value: "kitty", labelKey: "theme.kitty" },
];

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
  } catch {}
}

const THEME_KEY = "go-on-gui-theme";

// Migrate old "light"/"dark" values to new names
function migrateTheme(stored: string | null): Theme {
  if (stored === "light" || stored === null) return "default";
  if (stored === "dark") return "ink";
  if (["default", "meadow", "ink", "wuxia", "kitty"].includes(stored))
    return stored as Theme;
  return "default";
}

function loadTheme(): Theme {
  return migrateTheme(safeGetItem(THEME_KEY));
}

export const currentTheme = ref<Theme>(loadTheme());

function setTheme(theme: Theme) {
  currentTheme.value = theme;
  safeSetItem(THEME_KEY, theme);
  applyTheme(theme);
}

export function toggleTheme() {
  const idx = THEME_LIST.findIndex((t) => t.value === currentTheme.value);
  const next = THEME_LIST[(idx + 1) % THEME_LIST.length];
  setTheme(next.value);
}

export function currentThemeLabelKey(): string {
  return (
    THEME_LIST.find((t) => t.value === currentTheme.value)?.labelKey ??
    "theme.default"
  );
}

function applyTheme(theme: Theme) {
  const html = document.documentElement;
  // dark color-scheme for ink and wuxia
  const isDark = theme === "ink" || theme === "wuxia";
  if (isDark) {
    html.classList.add("dark");
    html.style.colorScheme = "dark";
  } else {
    html.classList.remove("dark");
    html.style.colorScheme = "light";
  }
  html.setAttribute("data-theme", theme);
}

// Apply theme on initialization
applyTheme(currentTheme.value);
