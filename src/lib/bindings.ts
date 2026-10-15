import type { MessageKey } from "../i18n/en";
import type { BindingInfo, SpecialAction } from "./types";

/** Human labels for onboard button bindings. All tables are data, so a new code is one new row. */

export type Translate = (key: MessageKey, vars?: Record<string, string | number>) => string;

/** Mouse button bitmask, lowest bit first. */
const MOUSE_BITS: ReadonlyArray<readonly [mask: number, key: MessageKey]> = [
  [0x01, "binding.mouse.left"],
  [0x02, "binding.mouse.right"],
  [0x04, "binding.mouse.middle"],
  [0x08, "binding.mouse.back"],
  [0x10, "binding.mouse.forward"],
];

/** HID modifier bits; left and right variants are not told apart. Order is the display order. */
const MODIFIERS: ReadonlyArray<readonly [mask: number, name: string]> = [
  [0x11, "Ctrl"],
  [0x22, "Shift"],
  [0x44, "Alt"],
  [0x88, "Win"],
];

const SPECIAL_KEYS: Record<SpecialAction, MessageKey> = {
  dpiShift: "binding.special.dpiShift",
  gShift: "binding.special.gShift",
  dpiUp: "binding.special.dpiUp",
  dpiDown: "binding.special.dpiDown",
  wheelLeft: "binding.special.wheelLeft",
  wheelRight: "binding.special.wheelRight",
  profileCycle: "binding.special.profileCycle",
};

/** HID consumer usages with a localized name. */
const CONSUMER_KEYS: Record<number, MessageKey> = {
  0xe9: "binding.media.volumeUp",
  0xea: "binding.media.volumeDown",
  0xe2: "binding.media.mute",
  0xcd: "binding.media.playPause",
  0xb5: "binding.media.next",
  0xb6: "binding.media.previous",
  0xb7: "binding.media.stop",
};

/** HID keyboard usage page (0x07): usage to key name. Names are the same in every language. */
const KEY_NAMES: Record<number, string> = buildKeyNames();

function buildKeyNames(): Record<number, string> {
  const names: Record<number, string> = {};
  // Letters 0x04..0x1D.
  for (let i = 0; i < 26; i++) names[0x04 + i] = String.fromCharCode(65 + i);
  // Digits: 0x1E..0x26 are 1..9, 0x27 is 0.
  for (let i = 0; i < 9; i++) names[0x1e + i] = String(i + 1);
  names[0x27] = "0";
  // F1..F12 at 0x3A, F13..F24 at 0x68.
  for (let i = 0; i < 12; i++) names[0x3a + i] = `F${i + 1}`;
  for (let i = 0; i < 12; i++) names[0x68 + i] = `F${i + 13}`;
  // Keypad digits: 0x59..0x61 are 1..9, 0x62 is 0.
  for (let i = 0; i < 9; i++) names[0x59 + i] = `Num ${i + 1}`;
  names[0x62] = "Num 0";
  const rest: Array<[number, string]> = [
    [0x28, "Enter"],
    [0x29, "Esc"],
    [0x2a, "Backspace"],
    [0x2b, "Tab"],
    [0x2c, "Space"],
    [0x2d, "-"],
    [0x2e, "="],
    [0x2f, "["],
    [0x30, "]"],
    [0x31, "\\"],
    [0x32, "#"],
    [0x33, ";"],
    [0x34, "'"],
    [0x35, "`"],
    [0x36, ","],
    [0x37, "."],
    [0x38, "/"],
    [0x39, "Caps Lock"],
    [0x46, "Print Screen"],
    [0x47, "Scroll Lock"],
    [0x48, "Pause"],
    [0x49, "Insert"],
    [0x4a, "Home"],
    [0x4b, "PgUp"],
    [0x4c, "Delete"],
    [0x4d, "End"],
    [0x4e, "PgDn"],
    [0x4f, "Right"],
    [0x50, "Left"],
    [0x51, "Down"],
    [0x52, "Up"],
    [0x53, "Num Lock"],
    [0x54, "Num /"],
    [0x55, "Num *"],
    [0x56, "Num -"],
    [0x57, "Num +"],
    [0x58, "Num Enter"],
    [0x63, "Num ."],
    [0x64, "\\"],
    [0x65, "Menu"],
  ];
  for (const [usage, name] of rest) names[usage] = name;
  return names;
}

const hexByte = (n: number, digits: number) => n.toString(16).toUpperCase().padStart(digits, "0");

function mouseLabel(mask: number, t: Translate): string {
  const parts: string[] = [];
  let known = 0;
  for (const [bit, key] of MOUSE_BITS) {
    if (mask & bit) parts.push(t(key));
    known |= bit;
  }
  const unknown = mask & ~known;
  if (unknown) parts.push(t("binding.mouse.unknown", { code: hexByte(unknown, 2) }));
  return parts.join(" + ");
}

function keyboardLabel(modifiers: number, usage: number, t: Translate): string {
  const parts: string[] = [];
  for (const [mask, name] of MODIFIERS) {
    if (modifiers & mask) parts.push(name);
  }
  if (usage !== 0) parts.push(KEY_NAMES[usage] ?? t("binding.key.unknown", { code: hexByte(usage, 2) }));
  return parts.join(" + ");
}

function consumerLabel(usage: number, t: Translate): string {
  const key = CONSUMER_KEYS[usage];
  return key ? t(key) : t("binding.media.unknown", { code: hexByte(usage, 4) });
}

/** Human label for one binding. Falls back to the raw hex when the data cannot be interpreted. */
export function bindingLabel(binding: BindingInfo, t: Translate): string {
  switch (binding.kind) {
    case "unassigned":
      return t("binding.unassigned");
    case "mouse": {
      const label = binding.mouseButtons ? mouseLabel(binding.mouseButtons, t) : "";
      return label || binding.rawHex;
    }
    case "keyboard": {
      const label = keyboardLabel(binding.modifiers ?? 0, binding.usage ?? 0, t);
      return label || binding.rawHex;
    }
    case "consumer":
      return binding.usage === null ? binding.rawHex : consumerLabel(binding.usage, t);
    case "special": {
      if (binding.special) return t(SPECIAL_KEYS[binding.special]);
      if (binding.specialCode !== null) return t("binding.special.unknown", { code: hexByte(binding.specialCode, 2) });
      return binding.rawHex;
    }
    default:
      return binding.rawHex;
  }
}

/** Slot label shown next to a binding: "G1".."G11". */
export function slotLabel(slot: number): string {
  return `G${slot + 1}`;
}
