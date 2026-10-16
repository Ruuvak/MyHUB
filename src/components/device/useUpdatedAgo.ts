import { usePlural } from "../../i18n/usePlural";
import { useT } from "../../i18n/useT";
import { useNow } from "../../state/devices";

/**
 * Text like "Read: just now", then minutes and hours ago, in the current language. With `style` "since" the
 * text has no "Read:" prefix ("just now", "5 minutes ago") so it can sit inside a longer sentence.
 */
export function useUpdatedAgo(
  updatedAtMs: number | null | undefined,
  style: "read" | "since" = "read",
): string | null {
  const t = useT();
  const plural = usePlural();
  const now = useNow();
  if (updatedAtMs === null || updatedAtMs === undefined) return null;
  const minutes = Math.max(0, Math.floor((now - updatedAtMs) / 60_000));
  if (minutes < 1) return t(style === "since" ? "since.justNow" : "ago.justNow");
  const base = style === "since" ? "since" : "ago";
  if (minutes < 60) return plural(`${base}.minutes`, minutes);
  return plural(`${base}.hours`, Math.floor(minutes / 60));
}
