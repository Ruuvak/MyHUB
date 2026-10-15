import { WarningCircle } from "@phosphor-icons/react";
import { useT } from "../../i18n/useT";
import { cx } from "../../lib/cx";
import { chargingKey } from "../../lib/device";
import type { BatteryInfo } from "../../lib/types";
import { BatteryChargingIcon } from "../../ui/icons";

/** Charging word with an icon, in accent (danger for a battery error). Renders nothing on battery power. */
export function ChargingLabel({ battery, className }: { battery: BatteryInfo | null | undefined; className?: string }) {
  const t = useT();
  const key = chargingKey(battery);
  if (!key) return null;
  const error = battery?.charging === "error";
  return (
    <span className={cx("inline-flex items-center gap-1 font-medium", error ? "text-danger" : "text-accent", className)}>
      {error ? (
        <WarningCircle size={16} weight="regular" aria-hidden="true" className="shrink-0" />
      ) : (
        <BatteryChargingIcon size={16} className="shrink-0" />
      )}
      <span className="truncate">{t(key)}</span>
    </span>
  );
}
