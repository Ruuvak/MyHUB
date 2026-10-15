import type { DeviceTab } from "../lib/deepLink";

export type ButtonLayer = "base" | "gshift";

/** Per-session UI choices for the device page. Lives in App state, so it survives view switches. */
export interface DeviceUi {
  tab: DeviceTab;
  /** Profile shown in the Buttons tab ("user-2", "rom-1"), or null for the active one. */
  profileId: string | null;
  layer: ButtonLayer;
  showDisabled: boolean;
}

export const initialDeviceUi: DeviceUi = {
  tab: "performance",
  profileId: null,
  layer: "base",
  showDisabled: false,
};
