import type { Language } from "./language";

export type PluralCategory = "one" | "few" | "many" | "other";

/** CLDR plural category for a whole number. Polish: one, few, many. English: one, other. */
export function pluralCategory(language: Language, n: number): PluralCategory {
  if (language === "pl") {
    if (n === 1) return "one";
    const last = n % 10;
    const lastTwo = n % 100;
    if (last >= 2 && last <= 4 && !(lastTwo >= 12 && lastTwo <= 14)) return "few";
    return "many";
  }
  return n === 1 ? "one" : "other";
}
