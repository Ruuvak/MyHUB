import { Clock, Keyboard, Lightbulb, Sliders } from "@phosphor-icons/react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { ReadOnlyNote } from "../../components/device/ReadOnlyNote";
import { useUpdatedAgo } from "../../components/device/useUpdatedAgo";
import { useT } from "../../i18n/useT";
import { errorCode, keyboardErrorMessage } from "../../lib/deviceErrors";
import { cx } from "../../lib/cx";
import { refreshDevice, setKeyboardProfile } from "../../lib/ipc";
import {
  brightnessPercent,
  effectKey,
  formatDuration,
  formatMm,
  heModeKey,
  hsvSwatch,
  initialLayer,
  keyTitle,
  layerNameKey,
  profileDisplayName,
  shortKeyLabel,
} from "../../lib/keyboard";
import type { DeviceDetail, HeProfileView, KeyboardView, KeymapLayerView, LightingView } from "../../lib/types";
import { Badge, Panel, SegmentedControl } from "../../ui";
import { useKeyboardRefresh } from "./useKeyboardRefresh";

const MATRIX_ROWS = 6;
const MATRIX_COLS = 16;

/** Keyboard view (Keychron K2 HE): info, HE profiles (selectable over USB with writes on), lighting, timers and the keymap. Re-reads the keyboard once when shown. */
export function KeyboardPanel({ detail, onDetail }: { detail: DeviceDetail; onDetail: (next: DeviceDetail) => void }) {
  const t = useT();
  useKeyboardRefresh(detail.id, onDetail);
  const keyboard = detail.keyboard;
  if (!keyboard) return null;
  const connected = detail.connection === "connected";
  const receiver = keyboard.connectionPath === "receiver";

  return (
    <div className="flex flex-col gap-4">
      {receiver ? <ReceiverNotes keyboard={keyboard} /> : null}
      <div className={cx("flex flex-col gap-4", !connected && "opacity-60")}>
        <Panel title={t("keyboard.info.title")} icon={<Keyboard size={18} weight="regular" aria-hidden="true" />}>
          <div className="px-4 pb-4">
            <dl className="flex flex-col divide-y divide-border">
              <InfoRow label={t("keyboard.firmware")} value={keyboard.firmware || "--"} mono />
              <InfoRow label={t("keyboard.protocol")} value={detail.protocol} mono />
              <InfoRow label={t("keyboard.transport")} value={detail.transport} />
              {keyboard.receiverFirmware ? (
                <InfoRow label={t("keyboard.receiverFirmware")} value={keyboard.receiverFirmware} mono />
              ) : null}
            </dl>
          </div>
        </Panel>

        {keyboard.he ? <HeCard detail={detail} keyboard={keyboard} onDetail={onDetail} /> : null}
        {keyboard.lighting ? <LightingCard lighting={keyboard.lighting} /> : null}
        {keyboard.timers ? <TimersCard timers={keyboard.timers} /> : null}
        {keyboard.keymap && keyboard.keymap.length > 0 ? <KeymapCard keyboard={keyboard} keymap={keyboard.keymap} /> : null}
      </div>

      <div className="flex flex-col gap-2">
        {keyboard.otherConfigurator ? <ReadOnlyNote icon="info" text={t("keyboard.note.other")} /> : null}
        {receiver ? null : <ReadOnlyNote icon="info" text={t("keyboard.note.usb")} />}
      </div>
    </div>
  );
}

/** Over the 2.4 GHz receiver: how it is connected and where the shown settings come from. */
export function ReceiverNotes({ keyboard }: { keyboard: KeyboardView }) {
  const t = useT();
  return (
    <div className="flex flex-col gap-2">
      <ReadOnlyNote icon="info" text={t("keyboard.note.receiver")} />
      <CacheLine cachedAtMs={keyboard.cachedAtMs} hasSettings={keyboard.he !== null} />
    </div>
  );
}

function CacheLine({ cachedAtMs, hasSettings }: { cachedAtMs: number | null; hasSettings: boolean }) {
  const t = useT();
  const when = useUpdatedAgo(cachedAtMs, "since");
  const text = hasSettings && when ? t("keyboard.cache.since", { when }) : t("keyboard.cache.none");
  return <p className="text-xs text-text-muted">{text}</p>;
}

function InfoRow({ label, value, mono }: { label: string; value: ReactNode; mono?: boolean }) {
  return (
    <div className="flex items-center justify-between gap-4 py-2.5 text-base">
      <dt className="shrink-0 text-text-muted">{label}</dt>
      <dd className={cx("min-w-0 truncate text-right text-text", mono && "font-mono tabular-nums")}>{value}</dd>
    </div>
  );
}

function HeCard({ detail, keyboard, onDetail }: { detail: DeviceDetail; keyboard: KeyboardView; onDetail: (next: DeviceDetail) => void }) {
  const t = useT();
  const he = keyboard.he;
  const [pending, setPending] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  if (!he) return null;
  const selectable = !detail.readOnly && keyboard.configurable && !keyboard.otherConfigurator;
  // Writes are off but the keyboard could be configured: say what to turn on.
  const needsWrites = detail.readOnly && keyboard.configurable;

  const select = (index: number) => {
    if (pending !== null || index === he.currentProfile) return;
    setPending(index);
    setError(null);
    setKeyboardProfile(detail.id, index).then(
      (next) => {
        if (!alive.current) return;
        setPending(null);
        onDetail(next);
      },
      (e: unknown) => {
        if (!alive.current) return;
        setPending(null);
        const { key, vars } = keyboardErrorMessage(e);
        setError(t(key, vars));
        // The state is unknown after a timeout: show what the keyboard has now.
        if (errorCode(e) === "deviceError:timeout") {
          refreshDevice(detail.id).then(
            (next) => {
              if (next && alive.current) onDetail(next);
            },
            () => {},
          );
        }
      },
    );
  };

  return (
    <Panel title={t("keyboard.he.title")} icon={<Sliders size={18} weight="regular" aria-hidden="true" />}>
      <ul className="flex flex-col gap-2 px-4 pt-2 pb-4">
        {he.profiles.map((profile) => (
          <li key={profile.index}>
            <ProfileRow
              profile={profile}
              active={profile.index === he.currentProfile}
              onSelect={selectable ? () => select(profile.index) : undefined}
              pending={pending === profile.index}
              locked={pending !== null}
            />
          </li>
        ))}
        {error ? (
          <li role="alert" className="text-sm text-danger">
            {error}
          </li>
        ) : null}
        {needsWrites ? (
          <li>
            <ReadOnlyNote icon="info" text={t("keyboard.he.needsWrites")} />
          </li>
        ) : null}
      </ul>
    </Panel>
  );
}

function ProfileRow({
  profile,
  active,
  onSelect,
  pending = false,
  locked = false,
}: {
  profile: HeProfileView;
  active: boolean;
  /** Set when the card is a button (writes allowed, keyboard configurable). */
  onSelect?: () => void;
  pending?: boolean;
  /** A switch is running: every card is disabled. */
  locked?: boolean;
}) {
  const t = useT();
  const counts: Array<{ label: string; value: number }> = [
    { label: t("keyboard.he.perKey"), value: profile.perKeyOverrides },
    { label: t("keyboard.he.dks"), value: profile.dksKeys },
    { label: t("keyboard.he.snap"), value: profile.snapPairs },
    { label: t("keyboard.he.analog"), value: profile.analogKeys },
  ].filter((c) => c.value > 0);
  const rapid = profile.mode === "rapidTrigger";
  const name = profileDisplayName(profile, t);

  const body = (
    <>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="min-w-0 truncate text-base font-medium text-text">{name}</span>
        {active ? <Badge tone="accent">{t("keyboard.he.active")}</Badge> : null}
        {pending ? <Badge>{t("keyboard.he.switching")}</Badge> : null}
        <span className="ml-auto text-sm text-text-muted">{t(heModeKey(profile.mode))}</span>
      </div>
      <div className="mt-1 flex flex-wrap gap-x-4 gap-y-0.5 text-sm text-text-muted">
        <Metric label={t("keyboard.he.actuation")} value={formatMm(profile.actuationMm)} />
        {rapid ? <Metric label={t("keyboard.he.press")} value={formatMm(profile.pressMm)} /> : null}
        {rapid ? <Metric label={t("keyboard.he.release")} value={formatMm(profile.releaseMm)} /> : null}
      </div>
      {counts.length > 0 ? (
        <div className="mt-1.5 flex flex-wrap gap-1.5">
          {counts.map((c) => (
            <Badge key={c.label}>
              {c.label} {c.value}
            </Badge>
          ))}
        </div>
      ) : null}
    </>
  );
  const frame = cx("rounded-control border px-3 py-2.5", active ? "border-accent/60 bg-accent/10" : "border-border bg-surface-2");

  if (!onSelect) {
    return (
      <div aria-current={active ? "true" : undefined} className={frame}>
        {body}
      </div>
    );
  }
  return (
    <button
      type="button"
      aria-current={active ? "true" : undefined}
      aria-busy={pending || undefined}
      aria-label={active ? undefined : t("keyboard.he.select", { name })}
      disabled={locked || pending}
      onClick={onSelect}
      className={cx(
        frame,
        "t-colors block w-full text-left disabled:opacity-60",
        !active && !locked && "hover-fine:border-accent/40",
        pending && "opacity-80",
      )}
    >
      {body}
    </button>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <span>
      {label} <span className="font-mono tabular-nums text-text">{value}</span>
    </span>
  );
}

export function EffectName({ effect }: { effect: number }) {
  const t = useT();
  const key = effectKey(effect);
  return <>{key ? t(key) : t("keyboard.light.effectN", { n: effect })}</>;
}

function LightingCard({ lighting }: { lighting: LightingView }) {
  const t = useT();
  return (
    <Panel title={t("keyboard.light.title")} icon={<Lightbulb size={18} weight="regular" aria-hidden="true" />}>
      <div className="px-4 pb-4">
        <dl className="flex flex-col divide-y divide-border">
          <InfoRow label={t("keyboard.light.effect")} value={<EffectName effect={lighting.effect} />} />
          <InfoRow label={t("keyboard.light.brightness")} value={`${brightnessPercent(lighting.brightness)}%`} mono />
          <InfoRow label={t("keyboard.light.speed")} value={String(lighting.speed)} mono />
          <InfoRow
            label={t("keyboard.light.color")}
            value={<ColorSwatch hue={lighting.hue} saturation={lighting.saturation} />}
          />
        </dl>
        <div className="mt-2 flex flex-wrap gap-1.5">
          {lighting.perKey ? <Badge>{t("keyboard.light.perKey")}</Badge> : null}
          {lighting.mix ? <Badge>{t("keyboard.light.mix")}</Badge> : null}
          {lighting.ledCount > 0 ? <Badge>{t("keyboard.light.leds", { n: lighting.ledCount })}</Badge> : null}
        </div>
      </div>
    </Panel>
  );
}

function TimersCard({ timers }: { timers: NonNullable<KeyboardView["timers"]> }) {
  const t = useT();
  return (
    <Panel title={t("keyboard.timers.title")} icon={<Clock size={18} weight="regular" aria-hidden="true" />}>
      <div className="px-4 pb-4">
        <dl className="flex flex-col divide-y divide-border">
          <InfoRow label={t("keyboard.timers.backlightOff")} value={formatDuration(timers.backlightOffS)} mono />
          <InfoRow label={t("keyboard.timers.sleep")} value={formatDuration(timers.sleepS)} mono />
          <InfoRow label={t("keyboard.timers.heScanIdle")} value={formatDuration(timers.heScanIdleS)} mono />
        </dl>
      </div>
    </Panel>
  );
}

function KeymapCard({ keyboard, keymap }: { keyboard: KeyboardView; keymap: KeymapLayerView[] }) {
  const t = useT();
  const [layer, setLayer] = useState(() => initialLayer(keyboard));
  const count = Math.max(keyboard.layerCount, keymap.length);
  const options = Array.from({ length: count }, (_, i) => {
    const nameKey = layerNameKey(i, keyboard.layerCount);
    return { value: String(i), label: nameKey ? t(nameKey) : t("keyboard.keymap.layerN", { n: i }) };
  });
  const data = keymap.find((l) => l.index === layer);
  const cells = new Map((data?.keys ?? []).map((k) => [k.row * MATRIX_COLS + k.col, k]));

  return (
    <Panel title={t("keyboard.keymap.title")} icon={<Keyboard size={18} weight="regular" aria-hidden="true" />}>
      <div className="flex flex-col gap-3 px-4 pt-2 pb-4">
        <div className="flex overflow-x-auto">
          <SegmentedControl label={t("keyboard.keymap.layer")} value={String(layer)} options={options} onChange={(v) => setLayer(Number(v))} />
        </div>
        {data ? (
          <div className="overflow-x-auto pb-1">
            <div
              role="group"
              aria-label={t("keyboard.keymap.grid")}
              className="grid min-w-[600px] gap-[3px]"
              style={{ gridTemplateColumns: `repeat(${MATRIX_COLS}, minmax(0, 1fr))` }}
            >
              {Array.from({ length: MATRIX_ROWS * MATRIX_COLS }, (_, i) => {
                const key = cells.get(i);
                if (!key) return <span key={i} aria-hidden="true" className="h-9" />;
                return (
                  <KeyCap key={i} label={shortKeyLabel(key.label)} title={keyTitle(key)} />
                );
              })}
            </div>
          </div>
        ) : (
          <p className="text-sm text-text-muted">{t("keyboard.keymap.empty")}</p>
        )}
      </div>
    </Panel>
  );
}

function KeyCap({ label, title }: { label: string; title: string }) {
  return (
    <span
      title={title}
      className="flex h-9 min-w-0 items-center justify-center overflow-hidden rounded-[6px] border border-border bg-surface-2 px-0.5 font-mono text-[11px] leading-none text-text"
    >
      <span className="truncate">{label}</span>
    </span>
  );
}

/** Colour swatch of the lighting colour, with its HSV values as the tooltip. */
export function ColorSwatch({ hue, saturation, size = 16 }: { hue: number; saturation: number; size?: number }) {
  return (
    <span
      aria-hidden="true"
      title={`H ${hue} S ${saturation}`}
      className="inline-block shrink-0 rounded-pill border border-border"
      style={{ width: size, height: size, background: hsvSwatch(hue, saturation) }}
    />
  );
}

