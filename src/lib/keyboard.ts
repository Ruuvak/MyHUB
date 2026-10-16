import type { MessageKey } from "../i18n/en";
import type { HeMode, HeProfileView, KeyboardView, KeymapKeyView } from "./types";

const EFFECT_KEYS: readonly MessageKey[] = [
  "keyboard.effect.0",
  "keyboard.effect.1",
  "keyboard.effect.2",
  "keyboard.effect.3",
  "keyboard.effect.4",
  "keyboard.effect.5",
  "keyboard.effect.6",
  "keyboard.effect.7",
  "keyboard.effect.8",
  "keyboard.effect.9",
  "keyboard.effect.10",
  "keyboard.effect.11",
  "keyboard.effect.12",
  "keyboard.effect.13",
  "keyboard.effect.14",
  "keyboard.effect.15",
  "keyboard.effect.16",
  "keyboard.effect.17",
  "keyboard.effect.18",
  "keyboard.effect.19",
  "keyboard.effect.20",
  "keyboard.effect.21",
  "keyboard.effect.22",
  "keyboard.effect.23",
  "keyboard.effect.24",
];

/** Message key of a lighting effect number (0..24), or null for a number outside the table. */
export function effectKey(effect: number): MessageKey | null {
  return EFFECT_KEYS[effect] ?? null;
}

const DEFAULT_NAME_KEYS: readonly MessageKey[] = ["keyboard.he.default.0", "keyboard.he.default.1", "keyboard.he.default.2"];

/** Message key of the factory name of an HE profile (0-based index), or null beyond the three known ones. */
export function defaultProfileNameKey(index: number): MessageKey | null {
  return DEFAULT_NAME_KEYS[index] ?? null;
}

const MODE_KEYS: Record<HeMode, MessageKey> = {
  global: "keyboard.he.mode.global",
  static: "keyboard.he.mode.static",
  rapidTrigger: "keyboard.he.mode.rapidTrigger",
};

export function heModeKey(mode: HeMode): MessageKey {
  return MODE_KEYS[mode];
}

type Translate = (key: MessageKey, vars?: Record<string, string | number>) => string;

/** The profile's own name, else the factory name, else "Profile n". */
export function profileDisplayName(profile: HeProfileView, t: Translate): string {
  if (profile.name) return profile.name;
  const key = defaultProfileNameKey(profile.index);
  return key ? t(key) : t("keyboard.he.profileN", { n: profile.index + 1 });
}

/** The active HE profile, if the keyboard data has one. */
export function activeHeProfile(keyboard: KeyboardView): HeProfileView | null {
  const he = keyboard.he;
  if (!he) return null;
  return he.profiles.find((p) => p.index === he.currentProfile) ?? null;
}

export function formatMm(mm: number): string {
  return `${mm.toFixed(1)} mm`;
}

/** Brightness byte 0..255 as a percent. */
export function brightnessPercent(value: number): number {
  return Math.round((Math.min(Math.max(value, 0), 255) / 255) * 100);
}

/** "10 min", "2 h", "1 h 30 min", "45 s"; zero is "0 s". */
export function formatDuration(totalSeconds: number): string {
  const total = Math.max(0, Math.round(totalSeconds));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const parts: string[] = [];
  if (h > 0) parts.push(`${h} h`);
  if (m > 0) parts.push(`${m} min`);
  if (s > 0 || parts.length === 0) parts.push(`${s} s`);
  return parts.join(" ");
}

/** CSS colour for a VIA hue and saturation (0..255 each) at full value. */
export function hsvSwatch(hue: number, saturation: number): string {
  const h = Math.round((hue / 255) * 360);
  const s = Math.min(Math.max(saturation, 0), 255) / 255;
  const l = 1 - s / 2;
  const sl = l === 0 || l === 1 ? 0 : (1 - l) / Math.min(l, 1 - l);
  return `hsl(${h} ${Math.round(sl * 100)}% ${Math.round(l * 100)}%)`;
}

/** Short text for a key cap: no "KC_" prefix, transparent as a triangle, "no key" as empty. */
export function shortKeyLabel(label: string): string {
  if (label === "KC_TRNS") return "▽";
  if (label === "KC_NO") return "";
  return label.startsWith("KC_") ? label.slice(3) : label;
}

export function keyTitle(key: KeymapKeyView): string {
  return `${key.label} (0x${key.code.toString(16).toUpperCase().padStart(4, "0")})`;
}

/** Message key of the name of a layer when the keyboard has four (Mac, Mac Fn, Win, Win Fn); null otherwise. */
export function layerNameKey(layer: number, layerCount: number): MessageKey | null {
  if (layerCount !== 4) return null;
  return (["keyboard.keymap.mac", "keyboard.keymap.macFn", "keyboard.keymap.win", "keyboard.keymap.winFn"] as const)[layer] ?? null;
}

/** Layer to show first: the default layer when it exists in the data, else the first layer. */
export function initialLayer(keyboard: KeyboardView): number {
  const layers = keyboard.keymap ?? [];
  const wanted = keyboard.defaultLayer;
  if (wanted !== null && layers.some((l) => l.index === wanted)) return wanted;
  return layers[0]?.index ?? 0;
}
