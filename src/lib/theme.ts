import type { ThemeSetting } from "./types";

export type ResolvedTheme = "light" | "dark";

const darkQuery = "(prefers-color-scheme: dark)";

/** Maps the theme setting to a concrete theme, using the OS preference for "system". */
export function resolveTheme(setting: ThemeSetting): ResolvedTheme {
  if (setting !== "system") return setting;
  return window.matchMedia(darkQuery).matches ? "dark" : "light";
}

/** Writes the resolved theme to data-theme on the root element. */
export function applyTheme(setting: ThemeSetting): void {
  document.documentElement.dataset.theme = resolveTheme(setting);
}

/** Calls cb when the OS color scheme changes. Returns an unsubscribe function. */
export function watchSystemTheme(cb: () => void): () => void {
  const mq = window.matchMedia(darkQuery);
  mq.addEventListener("change", cb);
  return () => mq.removeEventListener("change", cb);
}
