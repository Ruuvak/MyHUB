import { useT } from "../../i18n/useT";
import { BatteryBar } from "../../components/device/BatteryBar";
import { DeviceIllustration } from "../../components/device/DeviceIllustration";
import { ModeChip } from "../../components/device/ModeChip";
import { ReadOnlyNote } from "../../components/device/ReadOnlyNote";
import { RefreshButton } from "../../components/device/RefreshButton";
import { useUpdatedAgo } from "../../components/device/useUpdatedAgo";
import type { DeviceTab } from "../../lib/deepLink";
import { batteryOf, isOnPower, levelKey } from "../../lib/device";
import { ChargingLabel } from "../../components/device/ChargingLabel";
import type { DeviceDetail, DeviceSummary } from "../../lib/types";
import { useDeviceDetail } from "../../state/devices";
import type { DeviceUi } from "../../state/deviceUi";
import { Badge, SegmentedControl, Skeleton } from "../../ui";
import { WirelessReceiverIcon } from "../../ui/icons";
import { ButtonsTab } from "./ButtonsTab";
import { PerformanceTab } from "./PerformanceTab";
import { ProfilesTab } from "./ProfilesTab";

interface DeviceDetailViewProps {
  summary: DeviceSummary;
  ui: DeviceUi;
  onUiChange: (patch: Partial<DeviceUi>) => void;
  onOpenSettings: () => void;
}

/** Page for one device: hero row, tabs and the footer note. Buttons and profiles are read-only; DPI and report rate are editable when the backend allows it. */
export function DeviceDetailView({ summary, ui, onUiChange, onOpenSettings }: DeviceDetailViewProps) {
  const t = useT();
  const { detail, refreshing, refresh, applyDetail } = useDeviceDetail(summary.id);

  const tabOptions: Array<{ value: DeviceTab; label: string }> = [
    { value: "performance", label: t("tab.performance") },
    { value: "buttons", label: t("tab.buttons") },
    { value: "profiles", label: t("tab.profiles") },
  ];

  if (detail === undefined) return <DeviceDetailSkeleton />;
  if (detail === null) {
    return <p className="text-base text-text-muted">{t("flyout.noData")}</p>;
  }

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6">
      <Hero detail={detail} summary={summary} refreshing={refreshing} onRefresh={refresh} />

      <div className="flex">
        <SegmentedControl
          label={t("tabs.label")}
          value={ui.tab}
          options={tabOptions}
          onChange={(tab) => onUiChange({ tab })}
        />
      </div>

      <div key={ui.tab} className="animate-page">
        {ui.tab === "performance" ? <PerformanceTab detail={detail} onDetail={applyDetail} onOpenSettings={onOpenSettings} /> : null}
        {ui.tab === "buttons" ? <ButtonsTab detail={detail} ui={ui} onUiChange={onUiChange} /> : null}
        {ui.tab === "profiles" ? <ProfilesTab detail={detail} ui={ui} onUiChange={onUiChange} /> : null}
      </div>

      {detail.readOnly ? (
        <ReadOnlyNote />
      ) : (
        <ReadOnlyNote icon="info" text={detail.onboard?.mode === "host" ? t("note.temporary") : t("note.flash")} />
      )}
    </div>
  );
}

function Hero({
  detail,
  summary,
  refreshing,
  onRefresh,
}: {
  detail: DeviceDetail;
  summary: DeviceSummary;
  refreshing: boolean;
  onRefresh: () => void;
}) {
  const t = useT();
  const ago = useUpdatedAgo(detail.updatedAtMs);
  const connected = detail.connection === "connected";
  const percent = batteryOf(detail, summary);
  const onPower = connected && isOnPower(detail.battery);
  const levelWord = levelKey(detail.battery);

  return (
    <div className="flex flex-wrap items-center gap-x-6 gap-y-4">
      <DeviceIllustration illustration={detail.illustration} kind={detail.kind} size={120} dimmed={!connected} asleep={!connected} compact />
      <div className="min-w-[240px] flex-1">
        <div className="flex items-center gap-3">
          <h1 className="min-w-0 truncate text-xl font-semibold text-text">{detail.name}</h1>
          {connected ? <ModeChip mode={detail.onboard?.mode} activeProfile={detail.onboard?.activeProfile} /> : null}
        </div>
        <p className="flex items-center gap-1.5 text-sm text-text-muted">
          {detail.transport === "LIGHTSPEED" ? <WirelessReceiverIcon size={16} className="shrink-0" /> : null}
          <span className="truncate">
            {t("device.subline", { vendor: detail.vendor, transport: detail.transport, protocol: detail.protocol })}
          </span>
        </p>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Badge tone={connected ? "accent" : "neutral"}>
            {connected ? t("device.connection.connected") : t("device.connection.disconnected")}
          </Badge>
        </div>
        <div className="mt-4 flex items-end gap-4">
          <div className="w-44">
            {onPower ? (
              <ChargingLabel battery={detail.battery} className="text-xs" />
            ) : (
              <div className="text-xs text-text-muted">
                {connected ? t("device.battery.label") : `${t("device.battery.label")}, ${t("device.lastRead")}`}
              </div>
            )}
            {percent !== null ? (
              <>
                <div className="flex items-baseline gap-1">
                  <span className="font-mono text-2xl font-light tabular-nums text-text">{percent}</span>
                  <span className="text-base font-medium text-accent">%</span>
                </div>
                {onPower ? <div className="text-xs text-text-faint">{t("battery.reported")}</div> : null}
                <BatteryBar percent={percent} className="mt-1.5" />
              </>
            ) : levelWord ? (
              <div className="text-2xl font-light text-text">{t(levelWord)}</div>
            ) : (
              <div className="font-mono text-2xl font-light text-text-faint">--</div>
            )}
          </div>
          <RefreshButton pending={refreshing} onClick={onRefresh} />
        </div>
        {ago ? <p className="mt-2 text-xs text-text-faint">{ago}</p> : null}
      </div>
    </div>
  );
}

function DeviceDetailSkeleton() {
  const t = useT();
  return (
    <div role="status" aria-busy="true" aria-label={t("devices.loading")} className="mx-auto flex w-full max-w-3xl flex-col gap-6">
      <div className="flex items-center gap-6">
        <Skeleton className="h-[134px] w-[148px] shrink-0" />
        <div className="flex flex-1 flex-col gap-2">
          <Skeleton className="h-6 w-56" />
          <Skeleton className="h-4 w-44" />
          <Skeleton className="mt-2 h-5 w-32" />
        </div>
      </div>
      <Skeleton className="h-8 w-72" />
      <Skeleton className="h-40 w-full" />
    </div>
  );
}
