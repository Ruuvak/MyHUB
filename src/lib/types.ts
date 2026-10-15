/** Shared types for the backend contract (mirrors the Tauri commands in src-tauri). */

export type ThemeSetting = "system" | "light" | "dark";
export type LanguageSetting = "auto" | "pl" | "en";

export interface Settings {
  theme: ThemeSetting;
  language: LanguageSetting;
  closeToTray: boolean;
  startMinimized: boolean;
  /** Start with the system, hidden in the tray. Default true. */
  launchAtLogin: boolean;
  /** Master switch for volatile DPI and report rate changes. Off by default. */
  allowDeviceWrites: boolean;
  /** After start and reconnect, return to the last mode and DPI settings. Needs allowDeviceWrites. Default true. */
  restoreLastMode: boolean;
}

/** Last values the user applied to a device, stored on the PC. */
export interface DevicePrefs {
  dpi: number | null;
  reportRateHz: number | null;
  /** Last mode the user chose. */
  mode: "onboard" | "host";
}

export type DeviceKind = "mouse" | "keyboard" | "headset" | "gamepad";

export interface DeviceSummary {
  id: string;
  name: string;
  kind: DeviceKind;
  connection: "connected" | "disconnected";
  batteryPercent: number | null;
}

export interface AppInfo {
  name: string;
  version: string;
}

export type BindingKind = "mouse" | "keyboard" | "consumer" | "special" | "unassigned" | "unknown";

export type SpecialAction =
  | "wheelLeft"
  | "wheelRight"
  | "dpiUp"
  | "dpiDown"
  | "dpiShift"
  | "profileCycle"
  | "gShift";

export interface BindingInfo {
  /** 0-based slot index; the label in the UI is "G{slot+1}". */
  slot: number;
  kind: BindingKind;
  /** Bitmask: 0x01 left, 0x02 right, 0x04 middle, 0x08 back, 0x10 forward. */
  mouseButtons: number | null;
  /** HID modifier bitmask (0x01 LCtrl, 0x02 LShift, 0x04 LAlt, 0x08 LGui, 0x10 RCtrl, 0x20 RShift, 0x40 RAlt, 0x80 RGui). */
  modifiers: number | null;
  /** Keyboard: HID keyboard usage. Consumer: consumer usage. */
  usage: number | null;
  special: SpecialAction | null;
  /** Raw special code, also set for unknown specials. */
  specialCode: number | null;
  /** The 4 raw bytes as hex, e.g. "90070000". */
  rawHex: string;
}

export interface ProfileInfo {
  /** "user-1".."user-5", "rom-1", "rom-2". */
  id: string;
  number: number;
  rom: boolean;
  enabled: boolean;
  name: string | null;
  reportRateHz: number | null;
  dpiLevels: number[];
  defaultDpiIndex: number;
  shiftDpiIndex: number;
  buttons: BindingInfo[];
  gshift: BindingInfo[];
  /** ROM sectors store CRC 0, so crcOk is false there but the data is still shown. */
  crcOk: boolean;
}

export interface OnboardInfo {
  mode: "host" | "onboard" | null;
  activeProfile: number | null;
  buttonCount: number;
  /** User profiles first, then ROM profiles. */
  profiles: ProfileInfo[];
  /** Active profile number (3..5) when it is enabled, the mode is onboard and the sector is writable; else null. */
  writableProfile: number | null;
}

export type BatteryLevel = "full" | "good" | "low" | "critical";
export type ChargingState = "discharging" | "charging" | "almostFull" | "full" | "slowCharging" | "error" | "unknown";

export interface BatteryInfo {
  /** As reported by the mouse; can be 0 while only `level` is meaningful. */
  percent: number;
  level: BatteryLevel | null;
  charging: ChargingState;
  /** Byte 3 of 0x1004 fn1 != 0 (inferred). */
  externalPower: boolean;
}

/** Fields the mouse stores in the active onboard profile. All optional; omitted ones stay as they are. */
export interface SaveProfileArgs {
  id: string;
  dpiLevels?: number[];
  defaultDpiIndex?: number;
  shiftDpiIndex?: number;
  reportRateHz?: number;
}

export interface SaveProfileResult {
  detail: DeviceDetail;
  outcome: "written" | "unchanged";
  backupPath: string | null;
  /** False when the mouse had no other profile to switch to; the new values then apply after a profile change. */
  reloaded: boolean;
}

export interface DeviceDetail extends DeviceSummary {
  vendor: string;
  transport: string;
  protocol: string;
  readOnly: boolean;
  /** Unix ms of the last successful read. */
  updatedAtMs: number;
  /** UI illustration key, "g502x" for this mouse. */
  illustration: string | null;
  battery: BatteryInfo | null;
  dpi: { min: number; max: number; step: number; current: number } | null;
  reportRate: { optionsHz: number[]; currentHz: number } | null;
  onboard: OnboardInfo | null;
}
