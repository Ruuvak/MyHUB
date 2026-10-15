import { Plugs } from "@phosphor-icons/react";
import type { ReactNode } from "react";
import { useT } from "../i18n/useT";
import type { DeviceSummary } from "../lib/types";
import type { DeviceUi } from "../state/deviceUi";
import { EmptyState } from "../ui";
import { DeviceDetailView } from "./device/DeviceDetailView";

interface DevicesViewProps {
  devices: DeviceSummary[] | null;
  selected: DeviceSummary | null;
  ui: DeviceUi;
  onUiChange: (patch: Partial<DeviceUi>) => void;
  onOpenSettings: () => void;
}

/** Main area for the devices section: empty states, or the read-only page of the selected device. */
export function DevicesView({ devices, selected, ui, onUiChange, onOpenSettings }: DevicesViewProps) {
  const t = useT();

  if (devices === null) return null;

  if (devices.length === 0) {
    return (
      <Centered>
        <EmptyState
          icon={<Plugs size={40} weight="regular" className="text-text-muted" aria-hidden="true" />}
          title={t("empty.title")}
          description={t("empty.body")}
        />
      </Centered>
    );
  }

  if (!selected) {
    return (
      <Centered>
        <EmptyState
          icon={<Plugs size={32} weight="regular" aria-hidden="true" />}
          title={t("devices.noSelection.title")}
          description={t("devices.noSelection.body")}
        />
      </Centered>
    );
  }

  return <DeviceDetailView key={selected.id} summary={selected} ui={ui} onUiChange={onUiChange} onOpenSettings={onOpenSettings} />;
}

function Centered({ children }: { children: ReactNode }) {
  return <div className="flex h-full min-h-[320px] items-center justify-center">{children}</div>;
}
