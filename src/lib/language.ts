import type { LanguageSetting } from "./types";

export type Language = "pl" | "en";

/** Resolves "auto" from the browser language: Polish if it starts with "pl", else English. */
export function resolveLanguage(setting: LanguageSetting): Language {
  if (setting !== "auto") return setting;
  return navigator.language.toLowerCase().startsWith("pl") ? "pl" : "en";
}
