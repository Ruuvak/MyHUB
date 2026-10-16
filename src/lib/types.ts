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

export type CellCharging = "notCharging" | "charging" | "charged" | "unknown";

export interface HeadsetCell {
  percent: number;
  charging: CellCharging;
}

export interface HeadsetBattery {
  left: HeadsetCell | null;
  right: HeadsetCell | null;
  case: HeadsetCell | null;
}

export interface NcAsmView {
  mode: "noiseCancelling" | "ambient";
  ambientMode: "normal" | "voice";
  level: number;
  /** Range of the current ambient mode. */
  levelMin: number;
  levelMax: number;
}

/** Arguments of set_nc_asm besides the device id. NC is sent with level 0. */
export interface NcAsmArgs {
  mode: "noiseCancelling" | "ambient";
  ambientMode: "normal" | "voice";
  level: number;
}

export type HeadsetToggle = "sidetone" | "multipoint";

export interface EqPresetView {
  id: number;
  /** Stable lowercase key (off, bright, ..., custom, user1, user2); null for an unlisted id. */
  name: string | null;
}

export interface EqBandInfo {
  kind: "hz" | "clearBass" | "special" | "unknown";
  /** Centre frequency, set for kind "hz". */
  hz: number | null;
}

export interface EqView {
  preset: number;
  presetName: string | null;
  presets: EqPresetView[];
  /** Raw band steps, 0..stepCount-1; dB = step - (stepCount - 1) / 2. */
  bands: number[];
  stepCount: number;
  /** What each band is, same order and length as `bands`; null when not read. Never assume where Clear Bass sits: find kind "clearBass". */
  bandInfo: EqBandInfo[] | null;
}

/** Headset data (Sony WF-C710N), read only for now. */
export interface HeadsetView {
  battery: HeadsetBattery | null;
  ncAsm: NcAsmView | null;
  eq: EqView | null;
  multipoint: boolean | null;
  sidetone: boolean | null;
  /** "SBC" | "AAC" | "LDAC" | "aptX" | "aptX HD" | "LC3", null while unsettled or unknown. */
  codec: string | null;
  firmware: string | null;
}

/** Key work mode of a Hall effect profile. */
export type HeMode = "global" | "static" | "rapidTrigger";

export interface HeProfileView {
  index: number;
  /** null when the profile has no name. */
  name: string | null;
  mode: HeMode;
  actuationMm: number;
  pressMm: number;
  releaseMm: number;
  /** Keys whose own mode is not "global". */
  perKeyOverrides: number;
  dksKeys: number;
  snapPairs: number;
  analogKeys: number;
}

export interface HeView {
  /** 0-based index of the active profile. */
  currentProfile: number;
  profiles: HeProfileView[];
  /** Four [x, y] points of the response curve. */
  curve: [number, number][] | null;
}

export interface LightingView {
  brightness: number;
  effect: number;
  speed: number;
  hue: number;
  saturation: number;
  effectMax: number;
  perKey: boolean;
  mix: boolean;
  ledCount: number;
}

export interface TimersView {
  backlightOffS: number;
  sleepS: number;
  heScanIdleS: number;
}

export interface KeymapKeyView {
  row: number;
  col: number;
  code: number;
  /** QMK style name, e.g. "KC_A" or "MO(3)". */
  label: string;
}

/** One layer; matrix slots without a physical key are left out. */
export interface KeymapLayerView {
  index: number;
  keys: KeymapKeyView[];
}

/** Keyboard data (Keychron K2 HE), read only for now. */
export interface KeyboardView {
  firmware: string;
  viaProtocol: number;
  layerCount: number;
  defaultLayer: number | null;
  he: HeView | null;
  lighting: LightingView | null;
  timers: TimersView | null;
  keymap: KeymapLayerView[] | null;
  /** True while another configurator talks to the keyboard and the app sends nothing. */
  otherConfigurator: boolean;
  /** "usb": the cable; "receiver": the 2.4 GHz Keychron Link receiver (settings are the cached ones). */
  connectionPath: "usb" | "receiver";
  /** True only over USB and while no other configurator is active. */
  configurable: boolean;
  /** Unix ms of the USB read the shown settings come from; null over USB or without known settings. */
  cachedAtMs: number | null;
  /** Firmware of the 2.4 GHz receiver, when read. */
  receiverFirmware: string | null;
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
  /** Set for headsets, null for the mouse. */
  headset: HeadsetView | null;
  /** Set for keyboards, null for the other devices. */
  keyboard: KeyboardView | null;
}
