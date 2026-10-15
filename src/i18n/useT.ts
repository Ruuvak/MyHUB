import { useCallback } from "react";
import { resolveLanguage, type Language } from "../lib/language";
import { useSettings } from "../state/settings";
import { en, type MessageKey, type Messages } from "./en";
import { pl } from "./pl";


const dictionaries: Record<Language, Messages> = { en, pl };


/** Returns a translate function bound to the current language setting. Supports {name} placeholders. */
export function useT(): (key: MessageKey, vars?: Record<string, string | number>) => string {
  const { settings } = useSettings();
  const language = resolveLanguage(settings?.language ?? "auto");
  return useCallback(
    (key, vars) => {
      const template = dictionaries[language][key];
      if (!vars) return template;
      return template.replace(/\{(\w+)\}/g, (match, name: string) =>
        name in vars ? String(vars[name]) : match,
      );
    },
    [language],
  );
}
