import { resolveLanguage, type Language } from "../lib/language";
import { pluralCategory, type PluralCategory } from "../lib/plural";
import { useSettings } from "../state/settings";
import type { MessageKey } from "./en";
import { useT } from "./useT";

/** Message families that exist in four plural forms: `<base>.one`, `.few`, `.many`, `.other`. */
export type PluralBase =
  | "count.devicesConnected"
  | "count.levels"
  | "ago.minutes"
  | "ago.hours";

/** Current language, resolved from the settings. */
export function useLanguage(): Language {
  const { settings } = useSettings();
  return resolveLanguage(settings?.language ?? "auto");
}

/** Returns a function that picks the plural form of a message family for n and fills {n}. */
export function usePlural(): (base: PluralBase, n: number) => string {
  const t = useT();
  const language = useLanguage();
  return (base, n) => {
    const category: PluralCategory = pluralCategory(language, n);
    const key: MessageKey = `${base}.${category}`;
    return t(key, { n });
  };
}
