import { ArrowSquareOut, ChartBar, Info, Lock, Plugs, User, X } from "@phosphor-icons/react";
import { useEffect, useState, type ReactNode } from "react";
import { BatteryBar } from "../components/device/BatteryBar";
import { BatteryPill } from "../components/device/BatteryPill";
import { DeviceIllustration } from "../components/device/DeviceIllustration";
import { ModeSwitch } from "../components/device/ModeSwitch";
import { ModeChip } from "../components/device/ModeChip";
import { ReadOnlyNote } from "../components/device/ReadOnlyNote";
import { RefreshButton } from "../components/device/RefreshButton";
import { useUpdatedAgo } from "../components/device/useUpdatedAgo";
import { DeviceIcon } from "../components/DeviceIcon";
import { usePlural } from "../i18n/usePlural";
import { useT } from "../i18n/useT";
import { cx } from "../lib/cx";
import { activeUserProfile, batteryOf, isOnPower, levelKey } from "../lib/device";
import { ChargingLabel } from "../components/device/ChargingLabel";
import { setPendingTab, type DeviceTab } from "../lib/deepLink";
import { hideFlyout, openMainWindow, setNcAsm } from "../lib/ipc";
import type { DeviceDetail, DeviceSummary } from "../lib/types";
import { EmptyState, IconButton, LogoSymbol, ReadOnlyBadge, Skeleton } from "../ui";
import { DpiIcon, ReportRateIcon } from "../ui/icons";
import { TransportIcon } from "../components/device/TransportIcon";
import { cellChargingKey, lowestBud } from "../lib/headset";
import type { HeadsetCell } from "../lib/types";
import { useDeviceDetail, useDevices } from "../state/devices";
import { SoundModeSwitch } from "./device/HeadsetPanel";
import { useHeadsetWrite } from "./device/useHeadsetWrite";
import { useSettings } from "../state/settings";
import { activeHeProfile, brightnessPercent, formatMm, heModeKey, profileDisplayName } from "../lib/keyboard";
import { EffectName } from "./device/KeyboardPanel";
import { useKeyboardRefresh } from "./device/useKeyboardRefresh";

/** Compact tray flyout card, rendered when the window label is "flyout". The tiles are read-only; they open the main window. */
export function FlyoutView() {
  const t = useT();
  const devices = useDevices();
  const [pickedId, setPickedId] = useState<string | null>(null);

  const selected = devices?.find((d) => d.id === pickedId) ?? devices?.[0] ?? null;
  const { detail, refreshing, refresh, applyDetail } = useDeviceDetail(selected?.id ?? null);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") void hideFlyout();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  function openWindow(tab?: DeviceTab) {
    if (tab) setPendingTab(tab);
    const done = () => void hideFlyout();
    openMainWindow(selected?.id).then(done, done);
  }

  return (
    <div className="animate-page box-border h-full p-1.5">
      <div className="flex h-full min-h-0 flex-col overflow-hidden rounded-panel border border-border bg-surface shadow-float">
        <FlyoutHeader
          devices={devices}
          canRefresh={selected !== null}
          refreshing={refreshing}
          onRefresh={refresh}
          onOpen={() => openWindow()}
        />
        {devices === null ? (
          <FlyoutSkeleton />
        ) : selected === null ? (
          <div className="flex min-h-0 flex-1 items-center justify-center">
            <EmptyState
              icon={<Plugs size={40} weight="regular" className="text-text-muted" aria-hidden="true" />}
              title={t("empty.title")}
              description={t("empty.body")}
            />
          </div>
        ) : (
          <>
            <DeviceSwitcher devices={devices} selectedId={selected.id} onSelect={setPickedId} />
            {detail === undefined ? (
              <FlyoutSkeleton withoutHeader />
            ) : detail === null ? (
              <div className="flex min-h-0 flex-1 items-center justify-center px-6 text-center text-base text-text-muted">
                {t("flyout.noData")}
              </div>
            ) : (
              detail.headset ? (
                <HeadsetFlyoutBody detail={detail} summary={selected} onDetail={applyDetail} />
              ) : detail.keyboard ? (
                <KeyboardFlyoutBody detail={detail} onDetail={applyDetail} />
              ) : (
                <FlyoutBody detail={detail} summary={selected} onOpenTab={openWindow} onDetail={applyDetail} />
              )
            )}
          </>
        )}
        <FlyoutFooter updatedAtMs={detail?.updatedAtMs} host={detail?.onboard?.mode === "host"} />
      </div>
    </div>
  );
}

function FlyoutHeader({
  devices,
  canRefresh,
  refreshing,
  onRefresh,
  onOpen,
}: {
  devices: DeviceSummary[] | null;
  canRefresh: boolean;
  refreshing: boolean;
  onRefresh: () => void;
  onOpen: () => void;
}) {
  const t = useT();
  const plural = usePlural();
  const connectedCount = devices?.filter((d) => d.connection === "connected").length ?? 0;
  const subline =
    devices === null
      ? null
      : devices.length === 0
        ? t("empty.title")
        : connectedCount === 0
          ? t("devices.none")
          : plural("count.devicesConnected", connectedCount);

  return (
    <header className="flex items-center gap-3 px-4 pt-3 pb-1.5">
      <LogoSymbol size={36} />
      <div className="min-w-0 flex-1">
        <div className="text-base leading-5 font-semibold text-text">MyHUB</div>
        <div className="truncate text-xs text-text-muted">{subline ?? " "}</div>
      </div>
      <div className="flex items-center gap-0.5">
        <IconButton size="sm" aria-label={t("action.open")} title={t("action.open")} onClick={onOpen}>
          <ArrowSquareOut size={18} weight="regular" aria-hidden="true" />
        </IconButton>
        <RefreshButton size="sm" pending={refreshing} disabled={!canRefresh} onClick={onRefresh} />
        <IconButton size="sm" aria-label={t("action.close")} title={t("action.close")} onClick={() => void hideFlyout()}>
          <X size={18} weight="regular" aria-hidden="true" />
        </IconButton>
      </div>
    </header>
  );
}

/** Pill row, one pill per device: kind icon plus battery percent (the name for a device without a battery), or "asleep" while disconnected. */
function DeviceSwitcher({
  devices,
  selectedId,
  onSelect,
}: {
  devices: DeviceSummary[];
  selectedId: string;
  onSelect: (id: string) => void;
}) {
  const t = useT();
  return (
    <div role="group" aria-label={t("flyout.switcher")} className="flex shrink-0 gap-2 overflow-x-auto px-4 py-1">
      {devices.map((device) => {
        const selected = device.id === selectedId;
        const connected = device.connection === "connected";
        return (
          <button
            key={device.id}
            type="button"
            aria-pressed={selected}
            aria-label={device.name}
            title={device.name}
            onClick={() => onSelect(device.id)}
            className={cx(
              "pressable inline-flex h-7 shrink-0 items-center gap-2 rounded-pill border px-3 text-sm",
              selected
                ? "border-border bg-surface-2 text-text"
                : "cursor-pointer border-border/60 text-text-muted hover-fine:bg-surface-2 hover-fine:text-text",
            )}
          >
            <span className={cx(selected && "text-accent")}>
              <DeviceIcon kind={device.kind} size={16} />
            </span>
            {!connected ? (
              <span>{t("device.asleep")}</span>
            ) : device.batteryPercent !== null ? (
              <span className="font-mono tabular-nums">{device.batteryPercent}%</span>
            ) : (
              <span>{device.name}</span>
            )}
          </button>
        );
      })}
    </div>
  );
}

function FlyoutBody({
  detail,
  summary,
  onOpenTab,
  onDetail,
}: {
  detail: DeviceDetail;
  summary: DeviceSummary;
  onOpenTab: (tab: DeviceTab) => void;
  onDetail: (next: DeviceDetail) => void;
}) {
  const t = useT();
  const plural = usePlural();
  const connected = detail.connection === "connected";
  const percent = batteryOf(detail, summary);
  const onPower = connected && isOnPower(detail.battery);
  const levelWord = levelKey(detail.battery);
  const active = activeUserProfile(detail);
  const onboard = detail.onboard;
  const levels = active?.dpiLevels.length ?? null;

  return (
    <>
      <div className="px-4">
        <div className="relative flex justify-center">
          <DeviceIllustration illustration={detail.illustration} kind={detail.kind} size={150} dimmed={!connected} asleep={!connected} />
          {percent !== null ? (
            <div className="absolute top-[38%] left-1/2 ml-[96px]">
              <BatteryPill percent={percent} connected={connected} charging={connected && isOnPower(detail.battery)} />
            </div>
          ) : null}
        </div>
        <div className="text-center">
          <div className="flex items-center justify-center gap-2">
            <h1 className="min-w-0 truncate text-lg font-semibold text-text">{detail.name}</h1>
            {connected ? <ModeChip mode={onboard?.mode} activeProfile={onboard?.activeProfile} /> : null}
          </div>
          <p className="flex items-center justify-center gap-1.5 text-sm text-text-muted">
            <TransportIcon transport={detail.transport} />
            <span className="truncate">
              {t("device.subline", { vendor: detail.vendor, transport: detail.transport, protocol: detail.protocol })}
            </span>
          </p>
        </div>
      </div>

      {connected ? (
        <div className="px-4 pt-2">
          <ModeSwitch detail={detail} onDetail={onDetail} showInfo={false} />
        </div>
      ) : null}

      <div className="px-4 pt-2">
        <div className="flex items-end justify-between gap-3">
          <div className="min-w-0">
            {onPower ? (
              <ChargingLabel battery={detail.battery} className="text-xs" />
            ) : (
              <div className="text-xs text-text-muted">{t("device.battery.label")}</div>
            )}
            {percent !== null ? (
              <div className="flex items-baseline gap-1" title={onPower ? t("battery.reported") : undefined}>
                <span className="font-mono text-[48px] leading-[52px] font-light tabular-nums text-text">{percent}</span>
                <span className="text-lg font-medium text-accent">%</span>
                {onPower ? <span className="ml-1 text-xs text-text-faint">{t("battery.reported")}</span> : null}
              </div>
            ) : levelWord ? (
              <div className="font-mono text-[48px] leading-[52px] font-light text-text">{t(levelWord)}</div>
            ) : (
              <div className="font-mono text-[48px] leading-[52px] font-light text-text-faint">--</div>
            )}
          </div>
          <div className="flex flex-col items-end gap-1 pb-1">
            {!connected ? <span className="text-xs text-text-muted">{t("device.lastRead")}</span> : null}
          </div>
        </div>
        {percent !== null ? <BatteryBar percent={percent} className="mt-2" /> : <div className="mt-2 h-1.5" />}
      </div>

      <div className="px-4 pt-3 pb-3">
        <div className={cx("grid grid-cols-2 gap-2", !connected && "opacity-60")}>
          <Tile
            icon={<ReportRateIcon size={16} className="shrink-0" />}
            label={t("tile.reportRate")}
            value={detail.reportRate ? `${detail.reportRate.currentHz} Hz` : "--"}
            mono
            onClick={() => onOpenTab("performance")}
          />
          <Tile
            icon={<DpiIcon size={16} className="shrink-0" />}
            label={t("tile.dpi")}
            value={detail.dpi ? String(detail.dpi.current) : "--"}
            mono
            onClick={() => onOpenTab("performance")}
          />
          <Tile
            icon={<User size={16} weight="regular" aria-hidden="true" className="shrink-0" />}
            label={t("tile.activeProfile")}
            value={onboard?.activeProfile != null ? t("device.profile", { n: onboard.activeProfile }) : "--"}
            onClick={() => onOpenTab("profiles")}
          />
          <Tile
            icon={<ChartBar size={16} weight="regular" aria-hidden="true" className="shrink-0" />}
            label={t("tile.dpiLevels")}
            value={levels !== null ? plural("count.levels", levels) : "--"}
            onClick={() => onOpenTab("performance")}
          />
        </div>
      </div>
    </>
  );
}

/** Headset card: lowest bud percent, left, right and case, and the sound mode as the one editable control (when changes are allowed). */
function HeadsetFlyoutBody({
  detail,
  summary,
  onDetail,
}: {
  detail: DeviceDetail;
  summary: DeviceSummary;
  onDetail: (next: DeviceDetail) => void;
}) {
  const t = useT();
  const headset = detail.headset;
  const connected = detail.connection === "connected";
  const percent = (headset ? lowestBud(headset) : null) ?? batteryOf(detail, summary);
  const ncAsm = headset?.ncAsm ?? null;
  const write = useHeadsetWrite(detail.id, onDetail);
  const disabled = detail.readOnly || write.pending;

  return (
    <>
      <div className="px-4">
        <div className="relative flex justify-center">
          <DeviceIllustration illustration={detail.illustration} kind={detail.kind} size={150} dimmed={!connected} asleep={!connected} />
        </div>
        <div className="text-center">
          <h1 className="min-w-0 truncate text-lg font-semibold text-text">{detail.name}</h1>
          <p className="flex items-center justify-center gap-1.5 text-sm text-text-muted">
            <TransportIcon transport={detail.transport} />
            <span className="truncate">
              {t("device.subline", { vendor: detail.vendor, transport: detail.transport, protocol: detail.protocol })}
            </span>
          </p>
        </div>
      </div>

      <div className="px-4 pt-2">
        <div className="flex items-end justify-between gap-3">
          <div className="min-w-0">
            <div className="text-xs text-text-muted">{t("device.battery.label")}</div>
            {percent !== null ? (
              <div className="flex items-baseline gap-1">
                <span className="font-mono text-[48px] leading-[52px] font-light tabular-nums text-text">{percent}</span>
                <span className="text-lg font-medium text-accent">%</span>
              </div>
            ) : (
              <div className="font-mono text-[48px] leading-[52px] font-light text-text-faint">--</div>
            )}
          </div>
          <div className="flex flex-col items-end gap-1 pb-1">
            {!connected ? <span className="text-xs text-text-muted">{t("device.lastRead")}</span> : null}
          </div>
        </div>
        {percent !== null ? <BatteryBar percent={percent} className="mt-2" /> : <div className="mt-2 h-1.5" />}
      </div>

      <div className="px-4 pt-3 pb-3">
        <div className={cx("grid grid-cols-3 gap-2", !connected && "opacity-60")}>
          <MiniCell label={t("headset.battery.left")} cell={headset?.battery?.left ?? null} />
          <MiniCell label={t("headset.battery.right")} cell={headset?.battery?.right ?? null} />
          <MiniCell label={t("headset.battery.case")} cell={headset?.battery?.case ?? null} />
        </div>
        <div
          title={detail.readOnly && connected ? t("headset.note.writesOff", { setting: t("settings.allowWrites") }) : undefined}
          className={cx("mt-2 rounded-control border border-border bg-surface-2 px-3 py-2.5", !connected && "opacity-60")}
        >
          <span className="text-xs text-text-muted">{t("headset.sound.title")}</span>
          {ncAsm ? (
            <div className="mt-1 flex">
              <SoundModeSwitch
                ncAsm={ncAsm}
                disabled={disabled}
                onSet={(args) => write.run("sound", () => setNcAsm({ id: detail.id, ...args }))}
              />
            </div>
          ) : (
            <div className="text-base font-medium text-text">--</div>
          )}
          {write.error ? (
            <p role="alert" className="mt-1.5 text-xs text-danger">
              {write.error.text}
            </p>
          ) : null}
        </div>
      </div>
    </>
  );
}

/** Keyboard card: active HE profile with mode and actuation, lighting effect with brightness, and the other-configurator note. Read only. */
function KeyboardFlyoutBody({ detail, onDetail }: { detail: DeviceDetail; onDetail: (next: DeviceDetail) => void }) {
  const t = useT();
  useKeyboardRefresh(detail.id, onDetail);
  const keyboard = detail.keyboard;
  const connected = detail.connection === "connected";
  const profile = keyboard ? activeHeProfile(keyboard) : null;
  const lighting = keyboard?.lighting ?? null;

  return (
    <>
      <div className="px-4">
        <div className="relative flex justify-center">
          <DeviceIllustration illustration={detail.illustration} kind={detail.kind} size={150} dimmed={!connected} asleep={!connected} />
        </div>
        <div className="text-center">
          <h1 className="min-w-0 truncate text-lg font-semibold text-text">{detail.name}</h1>
          <p className="flex items-center justify-center gap-1.5 text-sm text-text-muted">
            <TransportIcon transport={detail.transport} />
            <span className="truncate">
              {t("device.subline", { vendor: detail.vendor, transport: detail.transport, protocol: detail.protocol })}
            </span>
          </p>
        </div>
      </div>

      <div className="px-4 pt-3 pb-3">
        <div className={cx("grid grid-cols-2 gap-2", !connected && "opacity-60")}>
          <InfoTile
            label={t("keyboard.he.title")}
            value={profile ? profileDisplayName(profile, t) : "--"}
            sub={profile ? `${t(heModeKey(profile.mode))}, ${formatMm(profile.actuationMm)}` : undefined}
          />
          <InfoTile
            label={t("keyboard.light.title")}
            value={lighting ? <EffectName effect={lighting.effect} /> : "--"}
            sub={lighting ? `${t("keyboard.light.brightness")} ${brightnessPercent(lighting.brightness)}%` : undefined}
          />
        </div>
        {keyboard?.otherConfigurator ? (
          <div className="mt-2">
            <ReadOnlyNote icon="info" text={t("keyboard.note.other")} />
          </div>
        ) : null}
      </div>
    </>
  );
}

function InfoTile({ label, value, sub }: { label: string; value: ReactNode; sub?: string }) {
  return (
    <div className="flex min-w-0 flex-col gap-1 rounded-control border border-border bg-surface-2 px-3 py-2.5">
      <span className="truncate text-xs text-text-muted">{label}</span>
      <span className="truncate text-base font-medium text-text">{value}</span>
      <span className="h-4 truncate text-xs text-text-muted">{sub}</span>
    </div>
  );
}

function MiniCell({ label, cell }: { label: string; cell: HeadsetCell | null }) {
  const t = useT();
  const key = cell ? cellChargingKey(cell.charging) : null;
  return (
    <div className="min-w-0 rounded-control border border-border bg-surface-2 px-3 py-2.5">
      <div className="truncate text-xs text-text-muted">{label}</div>
      <div className="truncate font-mono text-base font-medium tabular-nums text-text">{cell ? `${cell.percent}%` : "--"}</div>
      <div className="h-4 truncate text-xs font-medium text-accent">{key ? t(key) : null}</div>
    </div>
  );
}

/** Read-only setting tile. Clicking it opens the main window on the matching tab. */
function Tile({
  icon,
  label,
  value,
  mono,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  value: string;
  mono?: boolean;
  onClick: () => void;
}) {
  const t = useT();
  return (
    <button
      type="button"
      onClick={onClick}
      title={t("tile.readOnly")}
      className="pressable flex min-w-0 flex-col gap-1 rounded-control border border-border bg-surface-2 px-3 py-2.5 text-left hover-fine:border-text-faint"
    >
      <span className="flex items-center gap-2 text-text-muted">
        {icon}
        <span className="min-w-0 flex-1 truncate text-xs">{label}</span>
        <Lock size={12} weight="regular" aria-hidden="true" className="shrink-0 text-text-faint" />
      </span>
      <span className={cx("truncate text-base font-medium text-text", mono && "font-mono tabular-nums")}>{value}</span>
    </button>
  );
}

function FlyoutFooter({ updatedAtMs, host }: { updatedAtMs: number | undefined; host: boolean }) {
  const t = useT();
  const ago = useUpdatedAgo(updatedAtMs);
  const { settings } = useSettings();
  const temporary = settings?.allowDeviceWrites ?? false;
  return (
    <footer
      className="mt-auto flex items-center justify-between gap-3 border-t border-border px-4 py-1.5 text-xs text-text-muted"
      title={temporary ? (host ? t("note.temporary") : t("note.flash")) : t("note.readOnly")}
    >
      <span className="flex min-w-0 items-center gap-1.5">
        {temporary ? <Info size={14} weight="regular" aria-hidden="true" className="shrink-0" /> : <ReadOnlyBadge size={14} />}
        <span className="truncate">{temporary ? (host ? t("tile.temporary") : t("tile.flash")) : t("tile.readOnly")}</span>
      </span>
      {ago ? <span className="shrink-0 truncate text-text-faint">{ago}</span> : null}
    </footer>
  );
}

/** Placeholder with the same blocks as the loaded card. */
function FlyoutSkeleton({ withoutHeader }: { withoutHeader?: boolean }) {
  const t = useT();
  return (
    <div role="status" aria-busy="true" aria-label={t("devices.loading")} className="flex min-h-0 flex-1 flex-col">
      {withoutHeader ? null : <Skeleton className="mx-4 my-1.5 h-8 w-28 rounded-pill" />}
      <div className="flex flex-col items-center gap-2 px-4 pt-2">
        <Skeleton className="h-[140px] w-[88px]" />
        <Skeleton className="h-6 w-44" />
        <Skeleton className="h-4 w-52" />
      </div>
      <div className="px-4 pt-4">
        <Skeleton className="h-14 w-32" />
        <Skeleton className="mt-3 h-1.5 w-full rounded-pill" />
      </div>
      <div className="grid grid-cols-2 gap-2 px-4 py-3">
        <Skeleton className="h-[78px]" />
        <Skeleton className="h-[78px]" />
      </div>
    </div>
  );
}

