import type { MessageKey } from "../i18n/en";
import type { Language } from "./language";
import type { CellCharging, EqView, HeadsetCell, HeadsetView } from "./types";

/** Lower of the left and right bud percent, or null when neither is reported. */
export function lowestBud(headset: HeadsetView): number | null {
  const values = [headset.battery?.left, headset.battery?.right]
    .filter((c): c is HeadsetCell => c != null)
    .map((c) => c.percent);
  return values.length > 0 ? Math.min(...values) : null;
}

/** Message key for a charging cell, or null when it runs on its battery or the state is unknown. */
export function cellChargingKey(charging: CellCharging): MessageKey | null {
  if (charging === "charging") return "battery.charging";
  if (charging === "charged") return "battery.full";
  return null;
}

const presetKeys = {
  off: "headset.eq.preset.off",
  bright: "headset.eq.preset.bright",
  excited: "headset.eq.preset.excited",
  mellow: "headset.eq.preset.mellow",
  relaxed: "headset.eq.preset.relaxed",
  vocal: "headset.eq.preset.vocal",
  treble: "headset.eq.preset.treble",
  bass: "headset.eq.preset.bass",
  speech: "headset.eq.preset.speech",
  custom: "headset.eq.preset.custom",
  user1: "headset.eq.preset.user1",
  user2: "headset.eq.preset.user2",
} as const satisfies Record<string, MessageKey>;

/** Message key for a preset name key, or null when the name is null or not in the list. */
export function presetKey(name: string | null | undefined): MessageKey | null {
  if (!name) return null;
  return (presetKeys as Record<string, MessageKey | undefined>)[name] ?? null;
}

/** Band gain in dB: step 10 of 21 steps is 0 dB. */
export function bandDb(step: number, stepCount: number): number {
  return step - (stepCount - 1) / 2;
}

/** Signed dB text, e.g. "+4", "0", "-3". */
export function formatDb(db: number): string {
  return db > 0 ? `+${db}` : String(db);
}

export function eqHalfRange(eq: EqView): number {
  return Math.max(1, (eq.stepCount - 1) / 2);
}

/** Frequency text for a band label: "400 Hz", "1 kHz", "2,5 kHz" (pl) or "2.5 kHz" (en). */
export function formatHz(hz: number, language: Language): string {
  const locale = language === "pl" ? "pl-PL" : "en-US";
  const number = (n: number) => new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(n);
  return hz >= 1000 ? `${number(hz / 1000)} kHz` : `${number(hz)} Hz`;
}

/** Bands split like the Sony app: frequency sliders plus the Clear Bass slider; null without band info or frequency bands. */
export function splitBands(eq: EqView): { freq: { index: number; hz: number }[]; clearBass: number | null } | null {
  const info = eq.bandInfo;
  if (!info || info.length !== eq.bands.length) return null;
  const freq: { index: number; hz: number }[] = [];
  info.forEach((b, index) => {
    if (b.kind === "hz" && b.hz !== null) freq.push({ index, hz: b.hz });
  });
  if (freq.length === 0) return null;
  const clear = info.findIndex((b) => b.kind === "clearBass");
  return { freq, clearBass: clear >= 0 ? clear : null };
}
