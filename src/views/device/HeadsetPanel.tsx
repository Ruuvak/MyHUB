import { BatteryFull, Faders, Gear, Waveform } from "@phosphor-icons/react";
import { useEffect, useId, useState } from "react";
import { BatteryBar } from "../../components/device/BatteryBar";
import { ReadOnlyNote } from "../../components/device/ReadOnlyNote";
import { useT } from "../../i18n/useT";
import { cx } from "../../lib/cx";
import { bandDb, cellChargingKey, formatDb, presetKey } from "../../lib/headset";
import { refreshDevice, setEqBands, setEqPreset, setHeadsetToggle, setNcAsm } from "../../lib/ipc";
import type { DeviceDetail, EqView, HeadsetCell, NcAsmArgs, NcAsmView } from "../../lib/types";
import { Button, Panel, SegmentedControl, Switch } from "../../ui";
import { useHeadsetWrite, type HeadsetWrite, type HeadsetWriteScope } from "./useHeadsetWrite";

/** How long the "Reconnecting" state may last without a connected detail before the device is re-read. */
const RECONNECT_TIMEOUT_MS = 20_000;

/** Headset view: battery per cell, sound mode, equalizer and settings. Controls write only when detail.readOnly is false. */
export function HeadsetPanel({ detail, onDetail }: { detail: DeviceDetail; onDetail: (next: DeviceDetail) => void }) {
  const t = useT();
  const write = useHeadsetWrite(detail.id, onDetail);
  const [reconnecting, setReconnecting] = useState(false);
  const connected = detail.connection === "connected";

  // The reconnect ends when a connected detail arrives (the call result or device-detail-changed), or after 20 s with a re-read.
  useEffect(() => {
    if (!reconnecting || write.pending) return;
    if (connected) {
      setReconnecting(false);
      return;
    }
    const timer = window.setTimeout(() => {
      setReconnecting(false);
      refreshDevice(detail.id).then(
        (next) => {
          if (next) onDetail(next);
        },
        () => {},
      );
    }, RECONNECT_TIMEOUT_MS);
    return () => window.clearTimeout(timer);
  }, [reconnecting, write.pending, connected, detail.id, onDetail]);

  const headset = detail.headset;
  if (!headset) return null;
  const disabled = detail.readOnly || write.pending || reconnecting;

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
            {headset.ncAsm ? (
              <SoundMode
                ncAsm={headset.ncAsm}
                disabled={disabled}
                onSet={(args) => write.run("sound", () => setNcAsm({ id: detail.id, ...args }))}
              />
            ) : (
              <p className="text-base text-text-muted">--</p>
            )}
            <WriteError write={write} scope="sound" />
          </div>
        </Panel>

        <Panel title={t("headset.eq.title")} icon={<Faders size={18} weight="regular" aria-hidden="true" />}>
          <div className="px-4 pt-2 pb-4">
            {headset.eq ? (
              <Equalizer
                eq={headset.eq}
                disabled={disabled}
                onPreset={(preset) => write.run("eq", () => setEqPreset(detail.id, preset))}
                onSaveBands={(bands) => write.run("eq", () => setEqBands(detail.id, bands))}
              />
            ) : (
              <p className="text-base text-text-muted">--</p>
            )}
            <WriteError write={write} scope="eq" />
          </div>
        </Panel>

        <Panel title={t("headset.settings.title")} icon={<Gear size={18} weight="regular" aria-hidden="true" />}>
          <div className="flex flex-col px-4 pb-2">
            <ToggleRow
              label={t("headset.sidetone")}
              value={headset.sidetone}
              disabled={disabled}
              onChange={(on) => void write.run("settings", () => setHeadsetToggle(detail.id, "sidetone", on))}
            />
            <MultipointRow
              value={headset.multipoint}
              disabled={disabled}
              reconnecting={reconnecting}
              onConfirm={(on) => {
                setReconnecting(true);
                void write.run("settings", () => setHeadsetToggle(detail.id, "multipoint", on, true)).then((ok) => {
                  if (!ok) setReconnecting(false);
                });
              }}
            />
            <WriteError write={write} scope="settings" className="pb-2" />
            <dl className="flex flex-col divide-y divide-border border-t border-border">
              <SettingRow label={t("headset.codec")} value={headset.codec ?? "--"} />
              <SettingRow label={t("headset.firmware")} value={headset.firmware ?? "--"} mono />
            </dl>
          </div>
        </Panel>
      </div>

      {detail.readOnly && connected ? (
        <ReadOnlyNote icon="info" text={t("headset.note.writesOff", { setting: t("settings.allowWrites") })} />
      ) : null}
    </div>
  );
}

/** Inline error of the last failed write, shown next to the control group it belongs to. */
function WriteError({ write, scope, className }: { write: HeadsetWrite; scope: HeadsetWriteScope; className?: string }) {
  if (write.error?.scope !== scope) return null;
  return (
    <p role="alert" className={cx("mt-2 text-sm text-danger", className)}>
      {write.error.text}
    </p>
  );
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

/** Ambient level to send when switching modes: the current level kept inside the ambient range. */
function ambientLevel(ncAsm: NcAsmView): number {
  return Math.min(Math.max(ncAsm.level, ncAsm.levelMin), ncAsm.levelMax);
}

/** The two modes as equal options in a segmented control; no warning colours. Switching to noise cancelling sends level 0. */
export function SoundModeSwitch({
  ncAsm,
  disabled,
  onSet,
}: {
  ncAsm: NcAsmView;
  disabled: boolean;
  onSet: (args: NcAsmArgs) => Promise<boolean>;
}) {
  const t = useT();
  return (
    <SegmentedControl
      label={t("headset.sound.title")}
      value={ncAsm.mode}
      disabled={disabled}
      options={[
        { value: "noiseCancelling", label: t("headset.sound.nc") },
        { value: "ambient", label: t("headset.sound.ambient") },
      ]}
      onChange={(mode) => {
        if (disabled || mode === ncAsm.mode) return;
        void onSet(
          mode === "noiseCancelling"
            ? { mode, ambientMode: ncAsm.ambientMode, level: 0 }
            : { mode, ambientMode: ncAsm.ambientMode, level: ambientLevel(ncAsm) },
        );
      }}
    />
  );
}

/** Mode switch plus, in ambient mode, the sub-mode and a level slider that sends one command on release. */
export function SoundMode({
  ncAsm,
  disabled,
  onSet,
}: {
  ncAsm: NcAsmView;
  disabled: boolean;
  onSet: (args: NcAsmArgs) => Promise<boolean>;
}) {
  const t = useT();
  const [draft, setDraft] = useState<number | null>(null);
  const ambient = ncAsm.mode === "ambient";
  const shown = draft ?? ncAsm.level;

  function commit() {
    if (draft === null) return;
    if (draft === ncAsm.level || disabled) {
      setDraft(null);
      return;
    }
    void onSet({ mode: "ambient", ambientMode: ncAsm.ambientMode, level: draft }).then(() => setDraft(null));
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="self-start">
        <SoundModeSwitch ncAsm={ncAsm} disabled={disabled} onSet={onSet} />
      </div>
      {ambient ? (
        <>
          <div className="self-start">
            <SegmentedControl
              label={t("headset.ambient.mode")}
              value={ncAsm.ambientMode}
              disabled={disabled}
              options={[
                { value: "normal", label: t("headset.ambient.normal") },
                { value: "voice", label: t("headset.ambient.voice") },
              ]}
              onChange={(ambientMode) => {
                if (disabled || ambientMode === ncAsm.ambientMode) return;
                void onSet({ mode: "ambient", ambientMode, level: ambientLevel(ncAsm) });
              }}
            />
          </div>
          <label className="flex items-center gap-3 text-sm text-text-muted">
            <span className="shrink-0">{t("headset.ambient.level")}</span>
            <input
              type="range"
              min={ncAsm.levelMin}
              max={ncAsm.levelMax}
              step={1}
              value={shown}
              disabled={disabled}
              onChange={(event) => setDraft(Number(event.currentTarget.value))}
              onPointerUp={commit}
              onKeyUp={(event) => {
                if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"].includes(event.key)) commit();
              }}
              className="h-5 min-w-0 flex-1 accent-accent disabled:opacity-50"
            />
            <span className="w-12 shrink-0 text-right font-mono tabular-nums text-text">
              {shown}/{ncAsm.levelMax}
            </span>
          </label>
        </>
      ) : null}
    </div>
  );
}

export function presetLabel(eq: EqView, t: ReturnType<typeof useT>): string {
  const key = presetKey(eq.presetName);
  return key ? t(key) : t("headset.eq.presetUnknown", { n: eq.preset });
}

/** Preset picker plus six vertical band sliders staged locally; nothing is written until "Zapisz". */
export function Equalizer({
  eq,
  disabled,
  onPreset,
  onSaveBands,
}: {
  eq: EqView;
  disabled: boolean;
  onPreset: (preset: number) => Promise<boolean>;
  onSaveBands: (bands: number[]) => Promise<boolean>;
}) {
  const t = useT();
  const [draft, setDraft] = useState<number[] | null>(null);
  const bandsKey = eq.bands.join(",");
  const bands = draft ?? eq.bands;
  const dirty = draft !== null && draft.join(",") !== bandsKey;

  // Once the headset reports exactly the staged values (after a save), the staging ends.
  useEffect(() => {
    setDraft((d) => (d !== null && d.join(",") === bandsKey ? null : d));
  }, [bandsKey]);

  function setBand(index: number, step: number) {
    setDraft((d) => {
      const next = [...(d ?? eq.bands)];
      next[index] = step;
      return next;
    });
  }

  return (
    <div className="flex flex-col gap-4">
      <select
        aria-label={t("headset.eq.preset")}
        value={eq.preset}
        disabled={disabled}
        onChange={(event) => {
          const preset = Number(event.currentTarget.value);
          if (preset !== eq.preset) void onPreset(preset);
        }}
        className="h-9 self-start rounded-control border border-border bg-surface-2 px-3 text-base text-text disabled:opacity-50"
      >
        {eq.presets.some((p) => p.id === eq.preset) ? null : <option value={eq.preset}>{presetLabel(eq, t)}</option>}
        {eq.presets.map((p) => {
          const key = presetKey(p.name);
          return (
            <option key={p.id} value={p.id}>
              {key ? t(key) : t("headset.eq.presetUnknown", { n: p.id })}
            </option>
          );
        })}
      </select>
      <ul aria-label={t("headset.eq.bands")} className="grid grid-cols-6 gap-2">
        {bands.map((step, i) => {
          const db = bandDb(step, eq.stepCount);
          return (
            <li key={i} className="flex flex-col items-center gap-1.5">
              <span className="font-mono text-xs tabular-nums text-text">{formatDb(db)}</span>
              <div className="relative h-24 w-6">
                <input
                  type="range"
                  aria-label={t("headset.eq.band", { n: i + 1 })}
                  aria-valuetext={`${formatDb(db)} dB`}
                  min={0}
                  max={eq.stepCount - 1}
                  step={1}
                  value={step}
                  disabled={disabled}
                  onChange={(event) => setBand(i, Number(event.currentTarget.value))}
                  className="absolute top-1/2 left-1/2 h-5 w-24 -translate-x-1/2 -translate-y-1/2 -rotate-90 accent-accent disabled:opacity-50"
                />
              </div>
              <span className="text-xs text-text-faint">{t("headset.eq.band", { n: i + 1 })}</span>
            </li>
          );
        })}
      </ul>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="primary"
          size="sm"
          className="disabled:cursor-default disabled:border disabled:border-border disabled:bg-surface-2 disabled:text-text-faint disabled:opacity-60"
          disabled={disabled || !dirty}
          onClick={() => {
            if (draft) void onSaveBands(draft);
          }}
        >
          {t("headset.eq.save")}
        </Button>
        <Button variant="secondary" size="sm" disabled={disabled || !dirty} className="disabled:cursor-default disabled:border disabled:border-border disabled:bg-surface-2 disabled:text-text-faint disabled:opacity-60" onClick={() => setDraft(null)}>
          {t("headset.eq.revert")}
        </Button>
      </div>
      <p className="text-sm text-text-muted">{t("headset.eq.hint")}</p>
    </div>
  );
}

function ToggleRow({
  label,
  value,
  disabled,
  onChange,
}: {
  label: string;
  value: boolean | null;
  disabled: boolean;
  onChange: (on: boolean) => void;
}) {
  const id = useId();
  return (
    <div className="flex items-center justify-between gap-4 py-2.5 text-base">
      <span id={id} className="text-text-muted">
        {label}
      </span>
      <Switch checked={value === true} labelledBy={id} disabled={disabled || value === null} onChange={onChange} />
    </div>
  );
}

/** Multipoint switch: a change first asks for confirmation because the headset reconnects. */
function MultipointRow({
  value,
  disabled,
  reconnecting,
  onConfirm,
}: {
  value: boolean | null;
  disabled: boolean;
  reconnecting: boolean;
  onConfirm: (on: boolean) => void;
}) {
  const t = useT();
  const id = useId();
  const [target, setTarget] = useState<boolean | null>(null);
  // A confirm that was open is dropped when controls lock (write started, disconnect, changes turned off).
  const open = target !== null && !disabled;

  return (
    <div className="flex flex-col">
      <div className="flex items-center justify-between gap-4 py-2.5 text-base">
        <span id={id} className="text-text-muted">
          {t("headset.multipoint")}
        </span>
        <div className="flex items-center gap-3">
          {reconnecting ? <span className="text-sm text-text-muted">{t("headset.reconnecting")}</span> : null}
          <Switch
            checked={value === true}
            labelledBy={id}
            disabled={disabled || value === null}
            onChange={(on) => setTarget(on)}
          />
        </div>
      </div>
      {open ? (
        <div role="alertdialog" aria-labelledby={`${id}-text`} className="mb-2 flex flex-col gap-3 rounded-control border border-border bg-surface-2 p-3">
          <p id={`${id}-text`} className="text-sm text-text">
            {t("headset.multipoint.confirm")}
          </p>
          <div className="flex gap-2">
            <Button
              variant="primary"
              size="sm"
              onClick={() => {
                const on = target;
                setTarget(null);
                if (on !== null) onConfirm(on);
              }}
            >
              {t("headset.multipoint.change")}
            </Button>
            <Button variant="secondary" size="sm" autoFocus onClick={() => setTarget(null)}>
              {t("headset.multipoint.cancel")}
            </Button>
          </div>
        </div>
      ) : null}
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
