/**
 * Dev preview only: a Keychron K2 HE shaped like the backend answers (see ipc-contract.md, "Keychron K2 HE").
 * Values from the read-only probe of 2026-10-16 (.claude/crew/keychron-notes.md); the keymap rows are the
 * golden read in crates/keychron-proto/tests/fixtures/k2he_keymap.txt. Loaded through src/dev/demo.ts when
 * the URL has `?demo=k2he`, `?demo=k2he-busy` or `?demo=all`. No serial number.
 */
import type { DeviceDetail, KeymapKeyView, KeymapLayerView } from "../lib/types";

// Layers 0 Mac, 1 Mac Fn, 2 Win, 3 Win Fn: 6 rows x 16 big-endian keycodes.
const ROWS: string[][] = [
  [
    "0029 00be 00bd 7e04 7e05 7828 7827 00ac 00ae 00ab 00a8 00aa 00a9 7e08 004c 7821",
    "0035 001e 001f 0020 0021 0022 0023 0024 0025 0026 0027 002d 002e 002a 004b 0000",
    "002b 0014 001a 0008 0015 0017 001c 0018 000c 0012 0013 002f 0030 0031 004e 0000",
    "0039 0004 0016 0007 0009 000a 000b 000d 000e 000f 0033 0034 0028 004a 0000 0000",
    "00e1 0000 001d 001b 0006 0019 0005 0011 0010 0036 0037 0038 00e5 0052 004d 0000",
    "00e0 7e00 7e02 0000 0000 0000 002c 0000 0000 7e03 5221 00e4 0050 0051 004f 0000",
  ],
  [
    "0001 003a 003b 003c 003d 003e 003f 0040 0041 0042 0043 0044 0045 0001 0001 7820",
    "0001 7e0b 7e0c 7e0d 7e0e 0001 0001 0001 0001 0001 0001 0001 0001 0001 0001 0000",
    "7820 7821 7827 7823 7825 7829 0001 0001 0001 0001 0001 0001 0001 0001 0001 0000",
    "0001 7822 7828 7824 7826 782a 0001 0001 0001 0001 0001 0001 0001 0001 0000 0000",
    "0001 0000 0001 0001 0001 0001 7e0f 7013 0001 0001 0001 0001 0001 0001 0001 0000",
    "0001 0001 0001 0000 0000 0000 0001 0000 0000 0001 0001 0001 0001 0001 0001 0000",
  ],
  [
    "0029 003a 003b 003c 003d 003e 003f 0040 0041 0042 0043 0044 0045 0046 004c 7821",
    "0035 001e 001f 0020 0021 0022 0023 0024 0025 0026 0027 002d 002e 002a 004b 0000",
    "002b 0014 001a 0008 0015 0017 001c 0018 000c 0012 0013 002f 0030 0031 004e 0000",
    "0039 0004 0016 0007 0009 000a 000b 000d 000e 000f 0033 0034 0028 004a 0000 0000",
    "00e1 0000 001d 001b 0006 0019 0005 0011 0010 0036 0037 0038 00e5 0052 004d 0000",
    "00e0 00e3 00e2 0000 0000 0000 002c 0000 0000 00e6 5223 00e4 0050 0051 004f 0000",
  ],
  [
    "0001 00be 00bd 7e06 7e07 7828 7827 00ac 00ae 00ab 00a8 00aa 00a9 0001 0001 7820",
    "0001 7e0b 7e0c 7e0d 7e0e 0001 0001 0001 0001 0001 0001 0001 0001 0001 0001 0000",
    "7820 7821 7827 7823 7825 7829 0001 0001 0001 0001 0001 0001 0001 0001 0001 0000",
    "0001 7822 7828 7824 7826 782a 0001 0001 0001 0001 0001 0001 0001 0001 0000 0000",
    "0001 0000 0001 0001 0001 0001 7e0f 7013 0001 0001 0001 0001 0001 0001 0001 0000",
    "0001 0001 0001 0000 0000 0000 0001 0000 0000 0001 0001 0001 0001 0001 0001 0000",
  ],
];

const LETTERS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const BASIC: Record<number, string> = {
  0x28: "KC_ENT", 0x29: "KC_ESC", 0x2a: "KC_BSPC", 0x2b: "KC_TAB", 0x2c: "KC_SPC", 0x2d: "KC_MINS", 0x2e: "KC_EQL",
  0x2f: "KC_LBRC", 0x30: "KC_RBRC", 0x31: "KC_BSLS", 0x33: "KC_SCLN", 0x34: "KC_QUOT", 0x35: "KC_GRV",
  0x36: "KC_COMM", 0x37: "KC_DOT", 0x38: "KC_SLSH", 0x39: "KC_CAPS", 0x46: "KC_PSCR", 0x4a: "KC_HOME",
  0x4b: "KC_PGUP", 0x4c: "KC_DEL", 0x4d: "KC_END", 0x4e: "KC_PGDN", 0x4f: "KC_RGHT", 0x50: "KC_LEFT",
  0x51: "KC_DOWN", 0x52: "KC_UP", 0xa8: "KC_MUTE", 0xa9: "KC_VOLU", 0xaa: "KC_VOLD", 0xab: "KC_MNXT",
  0xac: "KC_MPRV", 0xae: "KC_MPLY", 0xbd: "KC_BRIU", 0xbe: "KC_BRID", 0xe0: "KC_LCTL", 0xe1: "KC_LSFT",
  0xe2: "KC_LALT", 0xe3: "KC_LGUI", 0xe4: "KC_RCTL", 0xe5: "KC_RSFT", 0xe6: "KC_RALT",
};
const RGB = [
  "RGB_TOG", "RGB_MOD", "RGB_RMOD", "RGB_HUI", "RGB_HUD", "RGB_SAI", "RGB_SAD", "RGB_VAI", "RGB_VAD", "RGB_SPI",
  "RGB_SPD", "RGB_M_P", "RGB_M_B", "RGB_M_R", "RGB_M_SW", "RGB_M_SN", "RGB_M_K", "RGB_M_X", "RGB_M_G", "RGB_M_T",
];
const CUSTOM = [
  "Left Option", "Right Option", "Left Cmd", "Right Cmd", "Mission Control", "Launch Pad", "Task View", "File Explorer",
  "Screenshot", "Cortana", "Siri", "Bluetooth Host 1", "Bluetooth Host 2", "Bluetooth Host 3", "2.4G", "Battery Level",
  "Profile 1", "Profile 2", "Profile 3",
];

/** The QMK style name the backend's Keycode Display gives, for the codes in this fixture. */
function label(code: number): string {
  if (code === 0) return "KC_NO";
  if (code === 1) return "KC_TRNS";
  if (code >= 0x04 && code <= 0x1d) return `KC_${LETTERS[code - 0x04]}`;
  if (code >= 0x1e && code <= 0x27) return `KC_${(code - 0x1e + 1) % 10}`;
  if (code >= 0x3a && code <= 0x45) return `KC_F${code - 0x3a + 1}`;
  if (code >= 0x5200 && code <= 0x52df) {
    const n = code & 0x1f;
    return ((code >> 5) & 7) === 1 ? `MO(${n})` : `0x${code.toString(16)}`;
  }
  if (code === 0x7013) return "NK_TOGG";
  if (code >= 0x7820 && code <= 0x7833) return RGB[code - 0x7820] ?? `0x${code.toString(16)}`;
  if (code >= 0x7e00 && code <= 0x7eff) return CUSTOM[code - 0x7e00] ?? `CUSTOM(${code - 0x7e00})`;
  return BASIC[code] ?? `0x${code.toString(16).padStart(4, "0")}`;
}

function buildKeymap(): KeymapLayerView[] {
  return ROWS.map((rows, index) => {
    const keys: KeymapKeyView[] = [];
    rows.forEach((line, row) => {
      line.split(" ").forEach((hex, col) => {
        const code = parseInt(hex, 16);
        // Slots that read 0x0000 on every layer have no key; the others keep KC_NO.
        const present = ROWS.some((layerRows) => layerRows[row]?.split(" ")[col] !== "0000");
        if (present) keys.push({ row, col, code, label: label(code) });
      });
    });
    return { index, keys };
  });
}

export function buildK2heDetail(busy: boolean): DeviceDetail {
  return {
    id: "keychron-k2he",
    name: "K2 HE",
    kind: "keyboard",
    connection: "connected",
    batteryPercent: null,
    vendor: "Keychron",
    transport: "USB",
    protocol: "VIA 12 / Keychron HE 4",
    readOnly: true,
    updatedAtMs: Date.now(),
    illustration: "k2he",
    battery: null,
    dpi: null,
    reportRate: null,
    onboard: null,
    headset: null,
    keyboard: {
      firmware: "v1.2.1 2025-06-17-10:09:52",
      viaProtocol: 12,
      layerCount: 4,
      defaultLayer: 2,
      he: {
        currentProfile: 0,
        profiles: [
          { index: 0, name: null, mode: "static", actuationMm: 2.0, pressMm: 0, releaseMm: 0, perKeyOverrides: 0, dksKeys: 0, snapPairs: 0, analogKeys: 0 },
          { index: 1, name: null, mode: "rapidTrigger", actuationMm: 2.0, pressMm: 0.4, releaseMm: 0.4, perKeyOverrides: 0, dksKeys: 0, snapPairs: 0, analogKeys: 0 },
          { index: 2, name: null, mode: "rapidTrigger", actuationMm: 2.0, pressMm: 0.4, releaseMm: 0.4, perKeyOverrides: 0, dksKeys: 0, snapPairs: 0, analogKeys: 25 },
        ],
        curve: null,
      },
      lighting: { brightness: 255, effect: 6, speed: 127, hue: 0, saturation: 255, effectMax: 24, perKey: true, mix: true, ledCount: 84 },
      timers: { backlightOffS: 600, sleepS: 7200, heScanIdleS: 1800 },
      keymap: buildKeymap(),
      otherConfigurator: busy,
    },
  };
}
