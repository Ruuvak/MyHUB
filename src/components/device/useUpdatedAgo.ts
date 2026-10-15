import { usePlural } from "../../i18n/usePlural";
import { useT } from "../../i18n/useT";
import { useNow } from "../../state/devices";

/** Text like "Read: just now", then minutes and hours ago, in the current language. */
export function useUpdatedAgo(updatedAtMs: number | null | undefined): string | null {
  const t = useT();
  const plural = usePlural();
  const now = useNow();
  if (updatedAtMs === null || updatedAtMs === undefined) return null;
  const minutes = Math.max(0, Math.floor((now - updatedAtMs) / 60_000));
  if (minutes < 1) return t("ago.justNow");
  if (minutes < 60) return plural("ago.minutes", minutes);
  return plural("ago.hours", Math.floor(minutes / 60));
}
