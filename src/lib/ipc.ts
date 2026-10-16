import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type {
  AppInfo,
  DeviceDetail,
  DevicePrefs,
  DeviceSummary,
  HeadsetToggle,
  HeadsetView,
  NcAsmArgs,
  SaveProfileArgs,
  SaveProfileResult,
  Settings,
} from "./types";

/** Typed wrapper over the Tauri commands and events. Outside Tauri it falls back to a dev preview. */

// DEV PREVIEW ONLY: used when the UI runs in a plain browser (no Tauri). Settings live in memory
// and are lost on reload. Devices come from a fixture only when the URL has ?demo=g502x,
// ?demo=g502x-off, ?demo=sony, ?demo=sony-off, ?demo=k2he, ?demo=k2he-busy or ?demo=all (all devices), otherwise the list is empty. Never reached inside the real app.
const previewDefaults: Settings = {
  theme: "system",
  language: "auto",
  closeToTray: true,
  startMinimized: false,
  launchAtLogin: true,
  allowDeviceWrites: false,
  restoreLastMode: true,
};
let previewSettings: Settings = { ...previewDefaults };

/** Dev preview only: the ?demo= query value, or null. */
function previewDemo(): string | null {
  if (!import.meta.env.DEV) return null;
  return new URLSearchParams(window.location.search).get("demo");
}

/** Dev preview only: loads the fixture module, which is kept out of the production code path. */
function loadFixture() {
  // Constant-folded away in production builds, so the fixture chunk is not emitted at all.
  if (!import.meta.env.DEV) throw new Error("Preview fixtures are available in dev builds only");
  return import("../dev/demo");
}

const wait = (ms: number) => new Promise<void>((resolve) => window.setTimeout(resolve, ms));

export function appInfo(): Promise<AppInfo> {
  if (!isTauri()) return Promise.resolve({ name: "MyHUB", version: "dev" });
  return invoke<AppInfo>("app_info");
}

export function getSettings(): Promise<Settings> {
  if (!isTauri()) return Promise.resolve({ ...previewSettings });
  return invoke<Settings>("get_settings");
}

export function setSettings(settings: Settings): Promise<Settings> {
  if (!isTauri()) {
    previewSettings = { ...settings };
    return Promise.resolve({ ...previewSettings });
  }
  return invoke<Settings>("set_settings", { settings });
}

/** Label of the window this frontend runs in: "main" or "flyout". */
export function windowLabel(): string {
  if (!isTauri()) {
    // Dev preview only: ?view=flyout renders the flyout card in a browser tab.
    return new URLSearchParams(window.location.search).get("view") === "flyout" ? "flyout" : "main";
  }
  return getCurrentWindow().label;
}

export async function listDevices(): Promise<DeviceSummary[]> {
  if (!isTauri()) {
    if (!previewDemo()) return [];
    return (await loadFixture()).demoSummaries(previewDemo());
  }
  return invoke<DeviceSummary[]>("list_devices");
}

// Dev preview only: values "applied" through setDpi and setReportRate, kept in memory like a real
// device would keep them until power off. Reset on reload.
const previewApplied: { dpi?: number; reportRateHz?: number; host?: boolean } = {};
let previewPrefs: DevicePrefs = { dpi: null, reportRateHz: null, mode: "onboard" };
// Dev preview only: what saveProfileToMouse "wrote" into the writable profile.
let previewSaved: Omit<SaveProfileArgs, "id"> = {};

/** Dev preview only: the fixture detail with the saved profile values merged into the writable profile. */
function withPreviewSaved(detail: DeviceDetail): DeviceDetail {
  const onboard = detail.onboard;
  if (!onboard) return detail;
  const writable = previewApplied.host ? null : onboard.writableProfile;
  const profiles = onboard.profiles.map((p) =>
    p.rom || p.number !== writable
      ? p
      : {
          ...p,
          dpiLevels: previewSaved.dpiLevels ?? p.dpiLevels,
          defaultDpiIndex: previewSaved.defaultDpiIndex ?? p.defaultDpiIndex,
          shiftDpiIndex: previewSaved.shiftDpiIndex ?? p.shiftDpiIndex,
          reportRateHz: previewSaved.reportRateHz ?? p.reportRateHz,
        },
  );
  // Dev only: `&dpiIndex=N` simulates a DPI index event from the mouse buttons (onboard mode).
  const raw = new URLSearchParams(window.location.search).get("dpiIndex");
  const active = profiles.find((p) => !p.rom && p.number === onboard.activeProfile);
  const level = raw !== null && !previewApplied.host ? active?.dpiLevels[Number(raw)] : undefined;
  const dpi = detail.dpi && level !== undefined ? { ...detail.dpi, current: level } : detail.dpi;
  return { ...detail, dpi, onboard: { ...onboard, profiles, writableProfile: writable } };
}

// Dev preview only: headset values "written" through the setNcAsm, setEq* and setHeadsetToggle wrappers.
let previewHeadset: Partial<HeadsetView> = {};

// Dev preview only: HE profile "selected" through setKeyboardProfile (0-based).
let previewKeyboardProfile: number | null = null;

/** Dev preview only: the fixture with applied values and a readOnly flag derived like the backend does. */
async function previewDetail(id: string): Promise<DeviceDetail | null> {
  if (!previewDemo()) return null;
  const detail = (await loadFixture()).demoDetails(previewDemo()).find((d) => d.id === id);
  if (!detail) return null;
  const connected = detail.connection === "connected";
  return {
    ...detail,
    headset: detail.headset && { ...detail.headset, ...previewHeadset },
    keyboard:
      detail.keyboard?.he && previewKeyboardProfile !== null
        ? { ...detail.keyboard, he: { ...detail.keyboard.he, currentProfile: previewKeyboardProfile } }
        : detail.keyboard,
    // Dev only: `&writes=1` makes a headset or keyboard fixture writable without turning the setting on.
    readOnly: !((previewSettings.allowDeviceWrites || ((detail.headset || detail.keyboard) && new URLSearchParams(window.location.search).get("writes") === "1")) && connected),
    dpi: detail.dpi && { ...detail.dpi, current: previewApplied.dpi ?? detail.dpi.current },
    reportRate: detail.reportRate && { ...detail.reportRate, currentHz: previewApplied.reportRateHz ?? detail.reportRate.currentHz },
    onboard: detail.onboard && previewApplied.host ? { ...detail.onboard, mode: "host" } : detail.onboard,
    updatedAtMs: Date.now(),
  };
}

async function previewDetailSaved(id: string): Promise<DeviceDetail | null> {
  const detail = await previewDetail(id);
  return detail && withPreviewSaved(detail);
}

export async function getDeviceDetail(id: string): Promise<DeviceDetail | null> {
  if (!isTauri()) return previewDetailSaved(id);
  return invoke<DeviceDetail | null>("get_device_detail", { id });
}

/** Re-reads the device now (read tier only). */
export async function refreshDevice(id: string): Promise<DeviceDetail | null> {
  if (!isTauri()) {
    if (!previewDemo()) return null;
    await wait(700);
    return previewDetailSaved(id);
  }
  return invoke<DeviceDetail | null>("refresh_device", { id });
}

/**
 * Sets the DPI at runtime (volatile, lost when the mouse powers off). Rejects with a contract error
 * string: "writesDisabled", "notSupported", "notHostMode", "disconnected", "outOfRange", "verifyFailed", "deviceError:<code>".
 */
export async function setDpi(id: string, dpi: number): Promise<DeviceDetail> {
  if (!isTauri()) {
    await wait(450);
    const detail = await previewDetail(id);
    if (!detail) throw "disconnected";
    if (detail.readOnly) throw detail.connection !== "connected" ? "disconnected" : "writesDisabled";
    if (!previewApplied.host) throw "notHostMode";
    if (!detail.dpi || dpi < detail.dpi.min || dpi > detail.dpi.max || (dpi - detail.dpi.min) % detail.dpi.step !== 0) throw "outOfRange";
    previewApplied.dpi = dpi;
    previewPrefs = { ...previewPrefs, dpi };
    return { ...detail, dpi: { ...detail.dpi, current: dpi }, onboard: detail.onboard };
  }
  return invoke<DeviceDetail>("set_dpi", { id, dpi });
}

/**
 * Switches the mouse to HOST mode on purpose (temporary mode). The mouse buttons, profiles and LEDs stop
 * working until leaveTemporaryMode. Errors: "writesDisabled", "disconnected", "deviceError:<code>".
 */
export async function enterTemporaryMode(id: string): Promise<DeviceDetail> {
  if (!isTauri()) {
    await wait(450);
    const detail = await previewDetailSaved(id);
    if (!detail) throw "disconnected";
    if (detail.readOnly) throw detail.connection !== "connected" ? "disconnected" : "writesDisabled";
    previewApplied.host = true;
    previewPrefs = { ...previewPrefs, mode: "host" };
    const next = await previewDetailSaved(id);
    if (!next) throw "disconnected";
    return next;
  }
  return invoke<DeviceDetail>("enter_temporary_mode", { id });
}

/** Returns the mouse to its onboard profile. Same error strings as enterTemporaryMode. */
export async function leaveTemporaryMode(id: string): Promise<DeviceDetail> {
  if (!isTauri()) {
    await wait(450);
    previewApplied.host = false;
    previewPrefs = { ...previewPrefs, mode: "onboard" };
    delete previewApplied.dpi;
    delete previewApplied.reportRateHz;
    const next = await previewDetailSaved(id);
    if (!next) throw "disconnected";
    return next;
  }
  return invoke<DeviceDetail>("leave_temporary_mode", { id });
}

/** Sets the report rate at runtime (volatile). Same error strings as setDpi. */
export async function setReportRate(id: string, hz: number): Promise<DeviceDetail> {
  if (!isTauri()) {
    await wait(450);
    const detail = await previewDetail(id);
    if (!detail) throw "disconnected";
    if (detail.readOnly) throw detail.connection !== "connected" ? "disconnected" : "writesDisabled";
    if (!previewApplied.host) throw "notHostMode";
    if (!detail.reportRate || !detail.reportRate.optionsHz.includes(hz)) throw "outOfRange";
    previewApplied.reportRateHz = hz;
    previewPrefs = { ...previewPrefs, reportRateHz: hz };
    return { ...detail, reportRate: { ...detail.reportRate, currentHz: hz }, onboard: detail.onboard };
  }
  return invoke<DeviceDetail>("set_report_rate", { id, hz });
}

/**
 * Writes the staged values into the active MyHUB profile in the mouse flash. Only call it from an explicit
 * save click, never from a slider or input change. Rejects with a contract error string: "writesDisabled",
 * "disconnected", "profileNotWritable", "notOnboardMode", "outOfRange", "backupFailed",
 * "flashWriteFailed:<restored|restoreFailed|notNeeded|unknown>", "savedReselectFailed:<0x000N|unknown>", "deviceError:<code>".
 */
export async function saveProfileToMouse(args: SaveProfileArgs): Promise<SaveProfileResult> {
  if (!isTauri()) {
    await wait(900);
    const detail = await previewDetailSaved(args.id);
    if (!detail) throw "disconnected";
    if (detail.readOnly) throw detail.connection !== "connected" ? "disconnected" : "writesDisabled";
    if (detail.onboard?.writableProfile == null) throw detail.onboard?.mode === "host" ? "notOnboardMode" : "profileNotWritable";
    const { id: _id, ...fields } = args;
    const before = JSON.stringify(previewSaved);
    const current = detail.onboard.profiles.find((p) => !p.rom && p.number === detail.onboard?.writableProfile);
    const same =
      (!fields.dpiLevels || fields.dpiLevels.join() === current?.dpiLevels.join()) &&
      (fields.defaultDpiIndex === undefined || fields.defaultDpiIndex === current?.defaultDpiIndex) &&
      (fields.shiftDpiIndex === undefined || fields.shiftDpiIndex === current?.shiftDpiIndex) &&
      (fields.reportRateHz === undefined || fields.reportRateHz === current?.reportRateHz);
    if (same) return { detail, outcome: "unchanged", backupPath: null, reloaded: true };
    previewSaved = { ...JSON.parse(before), ...fields };
    const next = await previewDetailSaved(args.id);
    if (!next) throw "disconnected";
    return { detail: next, outcome: "written", backupPath: "backups/logitech-g502x/20261015T120000Z_sector_0003.json", reloaded: !new URLSearchParams(window.location.search).has("noreload") };
  }
  return invoke<SaveProfileResult>("save_profile_to_mouse", { ...args });
}

/** Dev preview only: the headset detail for a write, or a thrown contract error string like the backend gives. */
async function previewHeadsetWrite(id: string): Promise<DeviceDetail & { headset: HeadsetView }> {
  await wait(450);
  const detail = await previewDetail(id);
  if (!detail?.headset) throw "notSupported";
  if (detail.readOnly) throw detail.connection !== "connected" ? "disconnected" : "writesDisabled";
  return { ...detail, headset: detail.headset };
}

/**
 * Sets the sound mode (noise cancelling or ambient). Rejects with a contract error string: "writesDisabled",
 * "disconnected", "outOfRange", "notSupported", "verifyFailed", "deviceError:timeout" (state unknown, re-read
 * with refreshDevice), "deviceError:<code>".
 */
export async function setNcAsm(args: NcAsmArgs & { id: string }): Promise<DeviceDetail> {
  if (!isTauri()) {
    const detail = await previewHeadsetWrite(args.id);
    const current = detail.headset.ncAsm;
    if (!current || args.level < 0 || args.level > current.levelMax) throw "outOfRange";
    previewHeadset = { ...previewHeadset, ncAsm: { ...current, mode: args.mode, ambientMode: args.ambientMode, level: args.level } };
    return (await previewDetail(args.id)) ?? detail;
  }
  return invoke<DeviceDetail>("set_nc_asm", { ...args });
}

/** Selects an equalizer preset by id. Same error strings as setNcAsm. */
export async function setEqPreset(id: string, preset: number): Promise<DeviceDetail> {
  if (!isTauri()) {
    const detail = await previewHeadsetWrite(id);
    const eq = detail.headset.eq;
    const entry = eq?.presets.find((p) => p.id === preset);
    if (!eq || !entry) throw "outOfRange";
    previewHeadset = { ...previewHeadset, eq: { ...eq, preset, presetName: entry.name } };
    return (await previewDetail(id)) ?? detail;
  }
  return invoke<DeviceDetail>("set_eq_preset", { id, preset });
}

/** Writes the six band steps (0..stepCount-1); the backend switches to the Custom preset first. Same error strings as setNcAsm. */
export async function setEqBands(id: string, bands: number[]): Promise<DeviceDetail> {
  if (!isTauri()) {
    const detail = await previewHeadsetWrite(id);
    const eq = detail.headset.eq;
    if (!eq || bands.length !== eq.bands.length || bands.some((b) => !Number.isInteger(b) || b < 0 || b >= eq.stepCount)) throw "outOfRange";
    const custom = eq.presets.find((p) => p.name === "custom");
    previewHeadset = { ...previewHeadset, eq: { ...eq, bands, preset: custom?.id ?? eq.preset, presetName: custom ? "custom" : eq.presetName } };
    return (await previewDetail(id)) ?? detail;
  }
  return invoke<DeviceDetail>("set_eq_bands", { id, bands });
}

/**
 * Switches sidetone or multipoint. Multipoint needs confirmDisconnect: true (otherwise "confirmRequired") and
 * the headset reconnects; the returned detail may be disconnected and the final state arrives through
 * onDeviceDetailChanged. Also rejects with "alertMissing" and "alertRefused", plus the setNcAsm strings.
 */
export async function setHeadsetToggle(id: string, setting: HeadsetToggle, on: boolean, confirmDisconnect?: boolean): Promise<DeviceDetail> {
  if (!isTauri()) {
    const detail = await previewHeadsetWrite(id);
    if (setting === "multipoint") {
      if (!confirmDisconnect) throw "confirmRequired";
      await wait(1500);
    }
    previewHeadset = { ...previewHeadset, [setting]: on };
    return (await previewDetail(id)) ?? detail;
  }
  return invoke<DeviceDetail>("set_headset_toggle", { id, setting, on, ...(confirmDisconnect ? { confirmDisconnect } : {}) });
}

/** Last DPI and report rate the user applied to this device, stored on the PC. */
export function getDevicePrefs(id: string): Promise<DevicePrefs | null> {
  if (!isTauri()) return Promise.resolve(previewDemo() ? { ...previewPrefs } : null);
  return invoke<DevicePrefs | null>("get_device_prefs", { id });
}

/** Shows and focuses the main window, optionally selecting a device there. */
export function openMainWindow(deviceId?: string): Promise<void> {
  if (!isTauri()) return Promise.resolve();
  return invoke<void>("open_main_window", { deviceId });
}

export function hideFlyout(): Promise<void> {
  if (!isTauri()) return Promise.resolve();
  return invoke<void>("hide_flyout");
}

/** Subscribes to a backend event. Resolves to an unsubscribe function; a no-op outside Tauri. */
async function subscribe<T>(event: string, cb: (payload: T) => void): Promise<() => void> {
  if (!isTauri()) return () => {};
  return listen<T>(event, (e) => cb(e.payload));
}

/** Subscribes to backend settings changes. */
export function onSettingsChanged(cb: (settings: Settings) => void): Promise<() => void> {
  return subscribe<Settings>("settings-changed", cb);
}

export function onDevicesChanged(cb: (devices: DeviceSummary[]) => void): Promise<() => void> {
  return subscribe<DeviceSummary[]>("devices-changed", cb);
}

export function onDeviceDetailChanged(cb: (detail: DeviceDetail) => void): Promise<() => void> {
  return subscribe<DeviceDetail>("device-detail-changed", cb);
}

/** Sent to the main window with the device id to select. */
export function onSelectDevice(cb: (id: string) => void): Promise<() => void> {
  return subscribe<string>("select-device", cb);
}

/**
 * Makes the HE profile with this 0-based index the active one on the keyboard (A9 11, never flash). Rejects with a
 * contract error string: "writesDisabled", "usbRequired" (keyboard on 2.4 GHz), "disconnected", "busy" (another
 * program talks to the keyboard), "outOfRange", "verifyFailed", "notSupported" (not the keyboard id),
 * "deviceError:timeout" (state unknown, the backend re-read it; refreshDevice shows the result),
 * "deviceError:<kind>".
 */
export async function setKeyboardProfile(id: string, profile: number): Promise<DeviceDetail> {
  if (!isTauri()) {
    await wait(450);
    const detail = await previewDetail(id);
    if (!detail?.keyboard) throw "notSupported";
    if (detail.readOnly) throw detail.connection !== "connected" ? "disconnected" : "writesDisabled";
    if (detail.keyboard.connectionPath === "receiver") throw "usbRequired";
    if (detail.keyboard.otherConfigurator) throw "busy";
    if (!Number.isInteger(profile) || profile < 0 || profile >= (detail.keyboard.he?.profiles.length ?? 0)) throw "outOfRange";
    previewKeyboardProfile = profile;
    return (await previewDetail(id)) ?? detail;
  }
  return invoke<DeviceDetail>("set_keyboard_profile", { id, profile });
}
