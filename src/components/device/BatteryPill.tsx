import { useT } from "../../i18n/useT";
import { batteryTone } from "../../lib/device";
import { BatteryLevelIcon } from "../../ui/icons";

const tones = { accent: "text-accent", warning: "text-warning", danger: "text-danger" } as const;

/** Small pill with the battery percent in mono. The icon follows the level and takes the tone color while connected. */
export function BatteryPill({ percent, connected, charging = false }: { percent: number; connected: boolean; charging?: boolean }) {
  const t = useT();
  return (
    <span className="inline-flex h-6 items-center gap-1.5 rounded-pill border border-border bg-surface px-2 text-xs">
      <BatteryLevelIcon percent={percent} charging={charging} size={16} className={connected ? tones[batteryTone(percent)] : "text-text-faint"} />
      <span aria-hidden="true" className="font-mono tabular-nums text-text">
        {percent}%
      </span>
      <span className="sr-only">{t("devices.battery", { n: percent })}</span>
    </span>
  );
}
