/**
 * The flyout and the main window are separate webviews of the same origin. The flyout leaves the tab it
 * wants opened here before it asks the backend to show the main window; the main window reads it once
 * when the "select-device" event arrives. localStorage is a convenience only: if it is blocked, the main
 * window simply opens on its remembered tab.
 */

export type DeviceTab = "performance" | "buttons" | "profiles";

const KEY = "myhub.pendingTab";
const tabs: ReadonlyArray<DeviceTab> = ["performance", "buttons", "profiles"];

export function setPendingTab(tab: DeviceTab): void {
  try {
    window.localStorage.setItem(KEY, tab);
  } catch {
    // Storage unavailable: the main window keeps its current tab.
  }
}

/** Returns and clears the pending tab, if one was left by the flyout. */
export function takePendingTab(): DeviceTab | null {
  try {
    const value = window.localStorage.getItem(KEY);
    window.localStorage.removeItem(KEY);
    return tabs.find((tab) => tab === value) ?? null;
  } catch {
    return null;
  }
}
