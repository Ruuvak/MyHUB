import { BatteryFull, Faders, Gear, Waveform } from "@phosphor-icons/react";
import { BatteryBar } from "../../components/device/BatteryBar";
import { ReadOnlyNote } from "../../components/device/ReadOnlyNote";
import { useT } from "../../i18n/useT";
import { cx } from "../../lib/cx";
import { bandDb, cellChargingKey, eqHalfRange, formatDb, presetKey } from "../../lib/headset";
import type { DeviceDetail, EqView, HeadsetCell, NcAsmView } from "../../lib/types";
import { Panel } from "../../ui";

/** Read-only headset view: battery per cell, sound mode, equalizer and settings. Nothing here writes to the headset. */
export function HeadsetPanel({ detail }: { detail: DeviceDetail }) {
  const t = useT();
  const headset = detail.headset;
  if (!headset) return null;
  const connected = detail.connection === "connected";

  return (
    <div className="flex flex-col gap-4">
      <div className={cx("flex flex-col gap-4", !connected && "opacity-60")}>
        <Panel title={t("headset.battery.title")} icon={<BatteryFull size={18} weight="regular" aria-hidden="true" />}>
          <div className="grid grid-cols-3 gap-3 px-4 pt-2 pb-4">
            <CellTile label={t("headset.battery.left")} cell={headset.battery?.left ?? null} />
            <CellTile label={t("headset.battery.right")} cell={headset.battery?.right ?? null} />
            <CellTile label={t("headset.battery.case")} cell={headset.battery?.case ?? null} />
          </div>
        </Panel>

        <Panel title={t("headset.sound.title")} icon={<Waveform size={18} weight="regular" aria-hidden="true" />}>
          <div className="px-4 pt-2 pb-4">
            {headset.ncAsm ? <SoundMode ncAsm={headset.ncAsm} /> : <p className="text-base text-text-muted">--</p>}
          </div>
        </Panel>

        <Panel title={t("headset.eq.title")} icon={<Faders size={18} weight="regular" aria-hidden="true" />}>
          <div className="px-4 pt-2 pb-4">
            {headset.eq ? <Equalizer eq={headset.eq} /> : <p className="text-base text-text-muted">--</p>}
          </div>
        </Panel>

        <Panel title={t("headset.settings.title")} icon={<Gear size={18} weight="regular" aria-hidden="true" />}>
          <dl className="flex flex-col divide-y divide-border px-4 pb-2">
            <SettingRow label={t("headset.multipoint")} value={onOff(headset.multipoint, t)} />
            <SettingRow label={t("headset.sidetone")} value={onOff(headset.sidetone, t)} />
            <SettingRow label={t("headset.codec")} value={headset.codec ?? "--"} />
            <SettingRow label={t("headset.firmware")} value={headset.firmware ?? "--"} mono />
          </dl>
        </Panel>
      </div>

      <ReadOnlyNote icon="info" text={t("headset.note")} />
    </div>
  );
}

function onOff(value: boolean | null, t: ReturnType<typeof useT>): string {
  return value === null ? "--" : value ? t("headset.on") : t("headset.off");
}

export function CellTile({ label, cell }: { label: string; cell: HeadsetCell | null }) {
  const t = useT();
  const key = cell ? cellChargingKey(cell.charging) : null;
  return (
    <div className="min-w-0 rounded-control border border-border bg-surface-2 px-3 py-2.5">
      <div className="truncate text-xs text-text-muted">{label}</div>
      {cell ? (
        <>
          <div className="flex items-baseline gap-1">
            <span className="font-mono text-2xl font-light tabular-nums text-text">{cell.percent}</span>
            <span className="text-base font-medium text-accent">%</span>
          </div>
          <BatteryBar percent={cell.percent} className="mt-1.5" />
          <div className="mt-1 h-4 truncate text-xs font-medium text-accent">{key ? t(key) : null}</div>
        </>
      ) : (
        <div className="font-mono text-2xl font-light text-text-faint">--</div>
      )}
    </div>
  );
}

/** The two modes as equal options in a segmented look. Not clickable: the current one is highlighted, neutral tone. */
export function SoundMode({ ncAsm }: { ncAsm: NcAsmView }) {
  const t = useT();
  const options = [
    { value: "noiseCancelling", label: t("headset.sound.nc") },
    { value: "ambient", label: t("headset.sound.ambient") },
  ] as const;
  const ambient = ncAsm.mode === "ambient";

  return (
    <div className="flex flex-col gap-3">
      <div
        role="group"
        aria-label={t("headset.sound.title")}
        title={t("headset.note")}
        className="inline-grid auto-cols-fr grid-flow-col self-start rounded-control border border-border bg-surface-2 p-0.5"
      >
        {options.map((option) => {
          const selected = option.value === ncAsm.mode;
          return (
            <span
              key={option.value}
              aria-current={selected ? "true" : undefined}
              aria-disabled="true"
              className={cx(
                "inline-flex h-7 cursor-default items-center justify-center rounded-[6px] px-3 text-sm font-medium whitespace-nowrap",
                selected ? "bg-accent text-accent-fg shadow-float" : "text-text-muted",
              )}
            >
              {option.label}
            </span>
          );
        })}
      </div>
      {ambient ? (
        <div className="flex flex-wrap items-baseline gap-x-6 gap-y-1 text-sm">
          <span className="text-text">{t(ncAsm.ambientMode === "voice" ? "headset.ambient.voice" : "headset.ambient.normal")}</span>
          <span className="text-text-muted">
            {t("headset.ambient.level")}{" "}
            <span className="font-mono tabular-nums text-text">
              {ncAsm.level}/{ncAsm.levelMax}
            </span>
          </span>
        </div>
      ) : null}
    </div>
  );
}

export function presetLabel(eq: EqView, t: ReturnType<typeof useT>): string {
  const key = presetKey(eq.presetName);
  return key ? t(key) : t("headset.eq.presetUnknown", { n: eq.preset });
}

export function Equalizer({ eq }: { eq: EqView }) {
  const t = useT();
  const half = eqHalfRange(eq);
  return (
    <div className="flex flex-col gap-4">
      <div className="text-base font-medium text-text">{presetLabel(eq, t)}</div>
      <ul aria-label={t("headset.eq.bands")} className="grid grid-cols-6 gap-2">
        {eq.bands.map((step, i) => {
          const db = bandDb(step, eq.stepCount);
          const pct = (Math.abs(db) / half) * 50;
          return (
            <li key={i} className="flex flex-col items-center gap-1.5">
              <span className="font-mono text-xs tabular-nums text-text">{formatDb(db)}</span>
              <div aria-hidden="true" className="relative h-20 w-2 rounded-pill bg-surface-2">
                <div className="absolute inset-x-0 top-1/2 h-px bg-border" />
                <div
                  className="absolute inset-x-0 rounded-pill bg-accent"
                  style={db >= 0 ? { bottom: "50%", height: `${pct}%` } : { top: "50%", height: `${pct}%` }}
                />
              </div>
              <span className="text-xs text-text-faint">{t("headset.eq.band", { n: i + 1 })}</span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function SettingRow({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-center justify-between gap-4 py-2.5 text-base">
      <dt className="text-text-muted">{label}</dt>
      <dd className={cx("text-text", mono && "font-mono tabular-nums")}>{value}</dd>
    </div>
  );
}
