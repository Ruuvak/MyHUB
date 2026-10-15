import { GearSix, Info, Plugs } from "@phosphor-icons/react";
import type { ReactNode } from "react";
import { useT } from "../i18n/useT";
import { cx } from "../lib/cx";
import type { DeviceSummary } from "../lib/types";
import { Badge, LogoLockup, Skeleton } from "../ui";
import { Battery } from "./Battery";
import { DeviceIcon } from "./DeviceIcon";

export type View = "devices" | "settings" | "about";

interface SidebarProps {
  /** Null while the first load is running. */
  devices: DeviceSummary[] | null;
  selectedDeviceId: string | null;
  view: View;
  onSelectDevice: (id: string) => void;
  onNavigate: (view: "settings" | "about") => void;
}

/** Fixed 240px sidebar: wordmark, device list, and the Settings and About entries. */
export function Sidebar({ devices, selectedDeviceId, view, onSelectDevice, onNavigate }: SidebarProps) {
  const t = useT();
  return (
    <aside className="flex h-full min-h-0 flex-col border-r border-border bg-surface">
      <div className="px-5 pt-5 pb-4">
        <LogoLockup size={28} label="MyHUB" />
      </div>
      <nav aria-label={t("nav.primary")} className="flex min-h-0 flex-1 flex-col">
        <h2 id="devices-heading" className="px-5 pb-2 text-xs font-medium text-text-muted">
          {t("nav.devices")}
        </h2>
        <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
          <DeviceList
            devices={devices}
            selectedDeviceId={view === "devices" ? selectedDeviceId : null}
            onSelect={onSelectDevice}
          />
        </div>
        <div className="flex flex-col gap-1 border-t border-border p-3">
          <NavItem
            label={t("nav.settings")}
            icon={(weight) => <GearSix size={18} weight={weight} aria-hidden="true" />}
            active={view === "settings"}
            onClick={() => onNavigate("settings")}
          />
          <NavItem
            label={t("nav.about")}
            icon={(weight) => <Info size={18} weight={weight} aria-hidden="true" />}
            active={view === "about"}
            onClick={() => onNavigate("about")}
          />
        </div>
      </nav>
    </aside>
  );
}

function DeviceList({
  devices,
  selectedDeviceId,
  onSelect,
}: {
  devices: DeviceSummary[] | null;
  selectedDeviceId: string | null;
  onSelect: (id: string) => void;
}) {
  const t = useT();

  if (devices === null) {
    return (
      <div role="status" aria-label={t("devices.loading")} className="flex flex-col gap-2">
        <Skeleton className="h-14" />
        <Skeleton className="h-14" />
      </div>
    );
  }

  if (devices.length === 0) {
    return (
      <div className="flex items-center gap-2 rounded-control border border-dashed border-border px-3 py-3 text-sm text-text-muted">
        <Plugs size={18} weight="regular" aria-hidden="true" className="shrink-0" />
        <span>{t("devices.none")}</span>
      </div>
    );
  }

  return (
    <ul aria-labelledby="devices-heading" className="flex flex-col gap-1">
      {devices.map((device) => (
        <li key={device.id}>
          <DeviceRow
            device={device}
            selected={device.id === selectedDeviceId}
            onClick={() => onSelect(device.id)}
          />
        </li>
      ))}
    </ul>
  );
}

function DeviceRow({ device, selected, onClick }: { device: DeviceSummary; selected: boolean; onClick: () => void }) {
  const t = useT();
  const connected = device.connection === "connected";
  return (
    <button
      type="button"
      aria-current={selected ? "true" : undefined}
      onClick={onClick}
      className={cx(
        "pressable grid w-full grid-cols-[18px_1fr_auto] items-start gap-x-3 gap-y-1 rounded-control px-3 py-2 text-left",
        selected ? "bg-surface-2 text-text" : "text-text-muted hover-fine:bg-surface-2 hover-fine:text-text",
      )}
    >
      <span className={cx("mt-0.5", selected && "text-accent")}>
        <DeviceIcon kind={device.kind} />
      </span>
      <span className="min-w-0 truncate text-base font-medium text-text">{device.name}</span>
      <span className="mt-0.5">{device.batteryPercent !== null ? <Battery percent={device.batteryPercent} /> : null}</span>
      <span className="col-start-2 col-end-4">
        <Badge tone={connected ? "accent" : "neutral"}>
          {connected ? t("device.connection.connected") : t("device.connection.disconnected")}
        </Badge>
      </span>
    </button>
  );
}

function NavItem({
  label,
  icon,
  active,
  onClick,
}: {
  label: string;
  icon: (weight: "regular" | "fill") => ReactNode;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      aria-current={active ? "page" : undefined}
      onClick={onClick}
      className={cx(
        "pressable flex h-9 w-full items-center gap-3 rounded-control px-3 text-base font-medium",
        active ? "bg-surface-2 text-text" : "text-text-muted hover-fine:bg-surface-2 hover-fine:text-text",
      )}
    >
      <span className={cx(active && "text-accent")}>{icon(active ? "fill" : "regular")}</span>
      {label}
    </button>
  );
}
