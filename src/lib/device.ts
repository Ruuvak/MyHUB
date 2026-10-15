import type { MessageKey } from "../i18n/en";
import type { Translate } from "./bindings";
import type { BatteryInfo, DeviceDetail, DeviceSummary, ProfileInfo } from "./types";

/** The device firmware writes this token when a profile has no custom name. */
const DEFAULT_NAME_TOKEN = "PROFILE_NAME_DEFAULT";

/** Custom profile name, or null when the profile has none (or only the firmware default token). */
export function customProfileName(profile: ProfileInfo): string | null {
  const name = profile.name?.trim();
  return name && name !== DEFAULT_NAME_TOKEN ? name : null;
}

/** "Profil N" for user profiles, "Fabryczny N" for ROM profiles. */
export function profileTitle(profile: ProfileInfo, t: Translate): string {
  return profile.rom ? `${t("profile.factory")} ${profile.number}` : t("device.profile", { n: profile.number });
}

/** The user profile that is currently active on the device, if known. */
export function activeUserProfile(detail: DeviceDetail): ProfileInfo | null {
  const onboard = detail.onboard;
  if (!onboard || onboard.activeProfile === null) return null;
  return onboard.profiles.find((p) => !p.rom && p.number === onboard.activeProfile) ?? null;
}

/** Clamps a DPI value into [min, max] and rounds it to the nearest step counted from min. */
export function snapDpi(value: number, min: number, max: number, step: number): number {
  const clamped = Math.min(max, Math.max(min, value));
  const snapped = min + Math.round((clamped - min) / step) * step;
  return Math.min(max, snapped);
}

/** Battery percent for display: the detail value, else the summary value, else null. */
export function batteryOf(detail: DeviceDetail | null | undefined, summary: DeviceSummary | null): number | null {
  const battery = detail?.battery;
  // The mouse can report 0% with only a level; then the level word is shown instead of a percent.
  if (battery && battery.percent <= 0 && battery.level) return null;
  return battery?.percent ?? detail?.batteryPercent ?? summary?.batteryPercent ?? null;
}

/** Fill tone for battery bars: warning at 20% or less, danger at 10% or less. */
export function batteryTone(percent: number): "accent" | "warning" | "danger" {
  if (percent <= 10) return "danger";
  if (percent <= 20) return "warning";
  return "accent";
}

/** True while the cable is in play: external power or any charging state other than discharging/unknown. */
export function isOnPower(battery: BatteryInfo | null | undefined): boolean {
  if (!battery) return false;
  return battery.externalPower || (battery.charging !== "discharging" && battery.charging !== "unknown");
}

/** Message key of the charging word for the battery, or null when the mouse runs on its battery. */
export function chargingKey(battery: BatteryInfo | null | undefined): MessageKey | null {
  if (!battery || !isOnPower(battery)) return null;
  switch (battery.charging) {
    case "charging":
      return "battery.charging";
    case "almostFull":
      return "battery.almostFull";
    case "full":
      return "battery.full";
    case "slowCharging":
      return "battery.slowCharging";
    case "error":
      return "battery.error";
    default:
      return "battery.onPower";
  }
}

const levelKeys = {
  full: "battery.level.full",
  good: "battery.level.good",
  low: "battery.level.low",
  critical: "battery.level.critical",
} as const;

/** Level word used in place of the percent when the mouse reports 0%. */
export function levelKey(battery: BatteryInfo | null | undefined): MessageKey | null {
  return battery && battery.percent <= 0 && battery.level ? levelKeys[battery.level] : null;
}
