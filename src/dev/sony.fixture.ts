/**
 * Dev preview only: a Sony WF-C710N shaped like the backend answers (see ipc-contract.md, "Sony headset").
 * Loaded through src/dev/demo.ts when the URL has `?demo=sony`, `?demo=sony-off` or `?demo=all`.
 * No MAC address, no serial number.
 */
import type { DeviceDetail } from "../lib/types";

// Preset ids as the real headset reports them (EQ capability reply).
const PRESETS: [number, string][] = [
  [0x00, "off"], [0x10, "bright"], [0x11, "excited"], [0x12, "mellow"], [0x13, "relaxed"], [0x14, "vocal"],
  [0x15, "treble"], [0x16, "bass"], [0x17, "speech"], [0xa0, "custom"], [0xa1, "user1"], [0xa2, "user2"],
];

export function buildSonyDetail(connected: boolean): DeviceDetail {
  const params = new URLSearchParams(window.location.search);
  const ambient = params.get("sound") === "ambient";
  const voice = params.get("ambient") === "voice";
  return {
    id: "sony-wf-c710n",
    name: "WF-C710N",
    kind: "headset",
    connection: connected ? "connected" : "disconnected",
    batteryPercent: 100,
    vendor: "Sony",
    transport: "Bluetooth",
    protocol: "Sony MDR",
    readOnly: true,
    updatedAtMs: Date.now(),
    illustration: "wf-c710n",
    battery: { percent: 100, level: null, charging: "discharging", externalPower: false },
    dpi: null,
    reportRate: null,
    onboard: null,
    headset: {
      battery: {
        left: { percent: 100, charging: "notCharging" },
        right: { percent: 100, charging: "notCharging" },
        case: { percent: 70, charging: "notCharging" },
      },
      ncAsm: { mode: ambient ? "ambient" : "noiseCancelling", ambientMode: voice ? "voice" : "normal", level: 20, levelMin: 1, levelMax: 20 },
      eq: {
        preset: 160,
        presetName: "custom",
        presets: PRESETS.map(([id, name]) => ({ id, name })),
        bands: [16, 14, 11, 10, 12, 15],
        stepCount: 21,
        bandInfo: [
          { kind: "clearBass", hz: null },
          { kind: "hz", hz: 400 },
          { kind: "hz", hz: 1000 },
          { kind: "hz", hz: 2500 },
          { kind: "hz", hz: 6300 },
          { kind: "hz", hz: 16000 },
        ],
      },
      multipoint: true,
      sidetone: true,
      codec: "AAC",
      firmware: "1.0.5",
    },
  };
}
