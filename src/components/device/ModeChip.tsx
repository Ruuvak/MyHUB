import { useT } from "../../i18n/useT";
import { HostModeIcon, MemoryProfileIcon } from "../../ui/icons";

/** Read-only chip next to the device name: "Profil N" while onboard, "Tryb HOST" in temporary mode. Renders nothing when unknown. */
export function ModeChip({
  mode,
  activeProfile,
}: {
  mode: "host" | "onboard" | null | undefined;
  activeProfile?: number | null;
}) {
  const t = useT();
  if (!mode) return null;
  const host = mode === "host";
  const Icon = host ? HostModeIcon : MemoryProfileIcon;
  const label = host
    ? t("device.mode.host")
    : activeProfile != null
      ? t("device.profile", { n: activeProfile })
      : t("device.mode.onboard");
  return (
    <span
      title={t("device.mode.label")}
      className="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-pill border border-border bg-surface-2 px-2.5 text-sm font-medium whitespace-nowrap text-text-muted"
    >
      <Icon size={14} />
      {label}
    </span>
  );
}
