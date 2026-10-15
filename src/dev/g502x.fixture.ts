/**
 * Dev preview only, values from a real read-only snapshot.
 *
 * Source: the user's Logitech G502 X LIGHTSPEED, read on 2026-10-15. This file is loaded only through a
 * dynamic import inside the `!isTauri()` branch of lib/ipc.ts, when the URL has `?demo=g502x` or
 * `?demo=g502x-off`. The production app never reaches it.
 */
import type { BatteryInfo, BindingInfo, ChargingState, DeviceDetail, DeviceSummary, ProfileInfo, SpecialAction } from "../lib/types";

const BUTTON_COUNT = 11;

const SPECIAL_CODES: Record<SpecialAction, number> = {
  wheelLeft: 0x01,
  wheelRight: 0x02,
  dpiUp: 0x03,
  dpiDown: 0x04,
  dpiShift: 0x07,
  profileCycle: 0x0a,
  gShift: 0x0b,
};

const hex = (bytes: number[]) => bytes.map((b) => b.toString(16).padStart(2, "0")).join("");

const empty = { mouseButtons: null, modifiers: null, usage: null, special: null, specialCode: null } as const;

/** `80 01 hi lo`: mouse button bitmask, big endian. */
function mouse(slot: number, mask: number): BindingInfo {
  return { slot, kind: "mouse", ...empty, mouseButtons: mask, rawHex: hex([0x80, 0x01, mask >> 8, mask & 0xff]) };
}

/** `80 02 mod usage`: HID modifier bitmask and keyboard usage. */
function keyboard(slot: number, modifiers: number, usage: number): BindingInfo {
  return { slot, kind: "keyboard", ...empty, modifiers, usage, rawHex: hex([0x80, 0x02, modifiers, usage]) };
}

/** `90 nn 00 00`: special function. */
function special(slot: number, action: SpecialAction): BindingInfo {
  const code = SPECIAL_CODES[action];
  return { slot, kind: "special", ...empty, special: action, specialCode: code, rawHex: hex([0x90, code, 0, 0]) };
}

/** `FF FF FF FF`. */
function unassigned(slot: number): BindingInfo {
  return { slot, kind: "unassigned", ...empty, rawHex: "ffffffff" };
}

/** Default layout of profiles 1 and 3..5: G1..G11 in slot order. */
function baseButtons(): BindingInfo[] {
  return [
    mouse(0, 0x01),
    mouse(1, 0x02),
    mouse(2, 0x04),
    mouse(3, 0x08),
    special(4, "dpiShift"),
    mouse(5, 0x10),
    special(6, "wheelLeft"),
    special(7, "wheelRight"),
    special(8, "profileCycle"),
    special(9, "dpiUp"),
    special(10, "dpiDown"),
  ];
}

function profile2Buttons(): BindingInfo[] {
  return baseButtons().map((b) => (b.slot === 4 ? special(4, "gShift") : b));
}

function profile2Gshift(): BindingInfo[] {
  return [
    mouse(0, 0x01),
    mouse(1, 0x02),
    keyboard(2, 0x01, 0x17),
    mouse(3, 0x08),
    unassigned(4),
    mouse(5, 0x10),
    keyboard(6, 0x03, 0x2b),
    keyboard(7, 0x01, 0x2b),
    keyboard(8, 0x01, 0x27),
    keyboard(9, 0x01, 0x1d),
    keyboard(10, 0x01, 0x1b),
  ];
}

function allUnassigned(): BindingInfo[] {
  return Array.from({ length: BUTTON_COUNT }, (_, slot) => unassigned(slot));
}

const levelsA = [600, 1200, 1600, 2400, 3200];
const levelsB = [800, 1200, 1600, 2400, 3200];

function buildProfiles(): ProfileInfo[] {
  const user1: ProfileInfo = {
    id: "user-1",
    number: 1,
    rom: false,
    enabled: true,
    name: "PROFILE_NAME_DEFAULT",
    reportRateHz: 1000,
    dpiLevels: levelsA,
    defaultDpiIndex: 2,
    shiftDpiIndex: 0,
    buttons: baseButtons(),
    gshift: baseButtons(),
    crcOk: true,
  };
  const user2: ProfileInfo = {
    id: "user-2",
    number: 2,
    rom: false,
    enabled: true,
    name: null,
    reportRateHz: 500,
    dpiLevels: levelsB,
    defaultDpiIndex: 2,
    shiftDpiIndex: 0,
    buttons: profile2Buttons(),
    gshift: profile2Gshift(),
    crcOk: true,
  };
  const disabled = (number: number): ProfileInfo => ({
    id: `user-${number}`,
    number,
    rom: false,
    enabled: false,
    name: null,
    reportRateHz: 1000,
    dpiLevels: levelsB,
    defaultDpiIndex: 2,
    shiftDpiIndex: 0,
    buttons: baseButtons(),
    gshift: allUnassigned(),
    crcOk: true,
  });
  const rom1: ProfileInfo = { ...disabled(3), id: "rom-1", number: 1, rom: true, enabled: true, crcOk: false };
  const rom2: ProfileInfo = {
    ...user2,
    id: "rom-2",
    number: 2,
    rom: true,
    crcOk: false,
    buttons: profile2Buttons(),
    gshift: profile2Gshift(),
  };
  // Profile 3 is the MyHUB profile: a clone of profile 2 with DPI 400/800/1200/1600/3200 (real device, stage 5A).
  const user3: ProfileInfo = {
    ...user2,
    id: "user-3",
    number: 3,
    dpiLevels: [400, 800, 1200, 1600, 3200],
    defaultDpiIndex: 2,
  };
  return [user1, user2, user3, disabled(4), disabled(5), rom1, rom2];
}

/** Dev preview only: `&charging=charging|almostFull|full|slowCharging|error` previews the cable states. */
function previewBattery(): BatteryInfo {
  const raw = new URLSearchParams(window.location.search).get("charging");
  const states: ChargingState[] = ["charging", "almostFull", "full", "slowCharging", "error"];
  const charging = states.find((s) => s === raw) ?? (raw !== null ? "charging" : "discharging");
  return { percent: 55, level: "good", charging, externalPower: charging !== "discharging" };
}

function buildDetail(connected: boolean): DeviceDetail {
  return {
    id: "logitech-g502x",
    name: "G502 X LIGHTSPEED",
    kind: "mouse",
    connection: connected ? "connected" : "disconnected",
    batteryPercent: 55,
    vendor: "Logitech",
    transport: "LIGHTSPEED",
    protocol: "HID++ 2.0",
    readOnly: true,
    updatedAtMs: Date.now(),
    illustration: "g502x",
    battery: previewBattery(),
    dpi: { min: 100, max: 25600, step: 50, current: 1600 },
    reportRate: { optionsHz: [1000, 500, 250, 125], currentHz: 500 },
    onboard: { mode: "onboard", activeProfile: 3, buttonCount: BUTTON_COUNT, profiles: buildProfiles(), writableProfile: 3 },
  };
}

function toSummary(detail: DeviceDetail): DeviceSummary {
  return {
    id: detail.id,
    name: detail.name,
    kind: detail.kind,
    connection: detail.connection,
    batteryPercent: detail.batteryPercent,
  };
}

/** Returns the fixture device for a `?demo=` value, or null for any other value. */
export function demoDetail(demo: string | null): DeviceDetail | null {
  if (demo === "g502x") return buildDetail(true);
  if (demo === "g502x-off") return buildDetail(false);
  return null;
}

export function demoSummaries(demo: string | null): DeviceSummary[] {
  const detail = demoDetail(demo);
  return detail ? [toSummary(detail)] : [];
}
