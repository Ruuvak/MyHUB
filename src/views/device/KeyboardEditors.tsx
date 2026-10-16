import { useId, useState } from "react";
import { useT } from "../../i18n/useT";
import { setKeyboardLighting, setKeyboardTravel, type KeyboardLightingArgs } from "../../lib/ipc";
import { brightnessPercent, effectKey, formatMm, hsvSwatch, profileDisplayName } from "../../lib/keyboard";
import type { DeviceDetail, HeProfileView, LightingView } from "../../lib/types";
import { Button, SegmentedControl } from "../../ui";
import { useKeyboardWrite } from "./useKeyboardWrite";

type Write = ReturnType<typeof useKeyboardWrite>;

/** Travel values in 0.1 mm steps, so the sliders and the comparison never see floating point noise. */
interface TravelDraft {
  mode: "static" | "rapidTrigger";
  actuation: number;
  press: number;
  release: number;
}

function travelOf(profile: HeProfileView): TravelDraft {
  return {
    // A profile whose global word points at the global setting shows as Static until the user picks a mode.
    mode: profile.mode === "rapidTrigger" ? "rapidTrigger" : "static",
    actuation: Math.round(profile.actuationMm * 10),
    press: Math.round(profile.pressMm * 10),
    release: Math.round(profile.releaseMm * 10),
  };
}

function sameTravel(a: TravelDraft, b: TravelDraft): boolean {
  if (a.mode !== b.mode || a.actuation !== b.actuation) return false;
  return a.mode === "static" || (a.press === b.press && a.release === b.release);
}

/** The slider row shared by both editors: label, range input and the value on the right. */
function SliderRow({
  label,
  value,
  min,
  max,
  step = 1,
  display,
  disabled,
  onChange,
  strip,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  display: string;
  disabled: boolean;
  onChange: (value: number) => void;
  /** A CSS gradient shown as a thin strip under the slider (hue and saturation). */
  strip?: string;
}) {
  return (
    <label className="flex flex-col gap-1 text-sm text-text-muted">
      <span className="flex items-center gap-3">
        <span className="shrink-0">{label}</span>
        <span className="ml-auto font-mono tabular-nums text-text">{display}</span>
      </span>
      <input
        type="range"
        aria-label={label}
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(Number(event.currentTarget.value))}
        className="h-5 w-full accent-accent disabled:opacity-50"
      />
      {strip ? <span aria-hidden="true" className="h-1.5 w-full rounded-pill border border-border" style={{ background: strip }} /> : null}
    </label>
  );
}

function SaveBar({ dirty, write, onSave, onReset }: { dirty: boolean; write: Write; onSave: () => void; onReset: () => void }) {
  const t = useT();
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Button variant="primary" size="sm" disabled={!dirty || write.pending} aria-busy={write.pending || undefined} onClick={onSave}>
        {write.pending ? t("keyboard.edit.saving") : t("keyboard.edit.save")}
      </Button>
      <Button size="sm" disabled={!dirty || write.pending} onClick={onReset}>
        {t("keyboard.edit.reset")}
      </Button>
    </div>
  );
}

/**
 * Editor of the actuation point and Rapid Trigger of the ACTIVE HE profile. Edits stay local until "Save to keyboard";
 * nothing is sent while a slider moves. Mount it with the profile index as key so a profile switch drops the draft.
 */
export function HeEditor({ detail, profile, write }: { detail: DeviceDetail; profile: HeProfileView; write: Write }) {
  const t = useT();
  const base = travelOf(profile);
  const [staged, setStaged] = useState<TravelDraft | null>(null);
  const draft = staged ?? base;
  const dirty = staged !== null && !sameTravel(staged, base);
  const rapid = draft.mode === "rapidTrigger";
  const edit = (patch: Partial<TravelDraft>) => setStaged({ ...draft, ...patch });
  const mm = (tenths: number) => formatMm(tenths / 10);

  const save = () => {
    void write
      .run(() =>
        setKeyboardTravel(detail.id, {
          profile: profile.index,
          mode: draft.mode,
          actuationMm: draft.actuation / 10,
          pressMm: draft.press / 10,
          releaseMm: draft.release / 10,
        }),
      )
      .then((ok) => {
        if (ok) setStaged(null);
      });
  };

  return (
    <div className="mt-1 flex flex-col gap-3 rounded-control border border-border bg-surface-2 px-3 py-3">
      <div className="text-base font-medium text-text">{t("keyboard.he.editTitle", { name: profileDisplayName(profile, t) })}</div>
      <p className="text-sm text-text-muted">{t("keyboard.he.editHint")}</p>
      <div className="flex">
        <SegmentedControl
          label={t("keyboard.he.mode")}
          value={draft.mode}
          disabled={write.pending}
          options={[
            { value: "static", label: t("keyboard.he.mode.static") },
            { value: "rapidTrigger", label: t("keyboard.he.mode.rapidTrigger") },
          ]}
          onChange={(mode) => edit({ mode })}
        />
      </div>
      <SliderRow
        label={t("keyboard.he.actuation")}
        value={draft.actuation}
        min={2}
        max={38}
        display={mm(draft.actuation)}
        disabled={write.pending}
        onChange={(actuation) => edit({ actuation })}
      />
      {rapid ? (
        <>
          <SliderRow
            label={t("keyboard.he.press")}
            value={draft.press}
            min={1}
            max={38}
            display={mm(draft.press)}
            disabled={write.pending}
            onChange={(press) => edit({ press })}
          />
          <SliderRow
            label={t("keyboard.he.release")}
            value={draft.release}
            min={1}
            max={38}
            display={mm(draft.release)}
            disabled={write.pending}
            onChange={(release) => edit({ release })}
          />
        </>
      ) : null}
      <SaveBar dirty={dirty} write={write} onSave={save} onReset={() => setStaged(null)} />
    </div>
  );
}

interface LightDraft {
  effect: number;
  brightness: number;
  speed: number;
  hue: number;
  saturation: number;
}

function lightOf(l: LightingView): LightDraft {
  return { effect: l.effect, brightness: l.brightness, speed: l.speed, hue: l.hue, saturation: l.saturation };
}

const HUE_STRIP = `linear-gradient(to right, ${Array.from({ length: 13 }, (_, i) => `hsl(${i * 30} 100% 50%)`).join(", ")})`;

/** Only the fields that differ from what the keyboard shows; hue and saturation go together. */
function lightChange(draft: LightDraft, base: LightDraft): KeyboardLightingArgs {
  const change: KeyboardLightingArgs = {};
  if (draft.effect !== base.effect) change.effect = draft.effect;
  // The slider works in percent: the same percent is the same brightness.
  if (brightnessPercent(draft.brightness) !== brightnessPercent(base.brightness)) change.brightness = draft.brightness;
  if (draft.speed !== base.speed) change.speed = draft.speed;
  if (draft.hue !== base.hue || draft.saturation !== base.saturation) {
    change.hue = draft.hue;
    change.saturation = draft.saturation;
  }
  return change;
}

/** Editor of the global lighting: effect, brightness (in percent), speed and colour. Staged, saved on demand. */
export function LightingEditor({ detail, lighting, write }: { detail: DeviceDetail; lighting: LightingView; write: Write }) {
  const t = useT();
  const selectId = useId();
  const base = lightOf(lighting);
  const [staged, setStaged] = useState<LightDraft | null>(null);
  const draft = staged ?? base;
  const change = lightChange(draft, base);
  const dirty = staged !== null && Object.keys(change).length > 0;
  const edit = (patch: Partial<LightDraft>) => setStaged({ ...draft, ...patch });
  const effects = Array.from({ length: lighting.effectMax + 1 }, (_, i) => i);

  const save = () => {
    void write.run(() => setKeyboardLighting(detail.id, change)).then((ok) => {
      if (ok) setStaged(null);
    });
  };

  return (
    <div className="flex flex-col gap-3 px-4 pb-4">
      <div className="flex flex-wrap items-center gap-3">
        <label htmlFor={selectId} className="text-sm text-text-muted">
          {t("keyboard.light.effect")}
        </label>
        <select
          id={selectId}
          value={draft.effect}
          disabled={write.pending}
          onChange={(event) => edit({ effect: Number(event.currentTarget.value) })}
          className="h-9 rounded-control border border-border bg-surface-2 px-3 text-base text-text disabled:opacity-50"
        >
          {effects.includes(draft.effect) ? null : <option value={draft.effect}>{t("keyboard.light.effectN", { n: draft.effect })}</option>}
          {effects.map((effect) => {
            const key = effectKey(effect);
            return (
              <option key={effect} value={effect}>
                {key ? t(key) : t("keyboard.light.effectN", { n: effect })}
              </option>
            );
          })}
        </select>
      </div>
      <SliderRow
        label={t("keyboard.light.brightness")}
        value={brightnessPercent(draft.brightness)}
        min={0}
        max={100}
        display={`${brightnessPercent(draft.brightness)}%`}
        disabled={write.pending}
        onChange={(percent) => edit({ brightness: Math.round((percent / 100) * 255) })}
      />
      <SliderRow
        label={t("keyboard.light.speed")}
        value={draft.speed}
        min={0}
        max={255}
        display={String(draft.speed)}
        disabled={write.pending}
        onChange={(speed) => edit({ speed })}
      />
      <SliderRow
        label={t("keyboard.light.hue")}
        value={draft.hue}
        min={0}
        max={255}
        display={String(draft.hue)}
        disabled={write.pending}
        onChange={(hue) => edit({ hue })}
        strip={HUE_STRIP}
      />
      <SliderRow
        label={t("keyboard.light.saturation")}
        value={draft.saturation}
        min={0}
        max={255}
        display={String(draft.saturation)}
        disabled={write.pending}
        onChange={(saturation) => edit({ saturation })}
        strip={`linear-gradient(to right, ${hsvSwatch(draft.hue, 0)}, ${hsvSwatch(draft.hue, 255)})`}
      />
      <p className="text-xs text-text-muted">{t("keyboard.light.editHint")}</p>
      <SaveBar dirty={dirty} write={write} onSave={save} onReset={() => setStaged(null)} />
      {write.error ? (
        <p role="alert" className="text-sm text-danger">
          {write.error}
        </p>
      ) : null}
    </div>
  );
}
