import { useCallback, useEffect, useState } from "react";
import { Sidebar, type View } from "./components/Sidebar";
import { AccessBanner } from "./components/AccessBanner";
import { UpdateBanner } from "./components/UpdateBanner";
import { takePendingTab } from "./lib/deepLink";
import { onSelectDevice } from "./lib/ipc";
import { initialDeviceUi, type DeviceUi } from "./state/deviceUi";
import { useDevices } from "./state/devices";
import { AboutView } from "./views/AboutView";
import { DevicesView } from "./views/DevicesView";
import { SettingsView } from "./views/SettingsView";

/** App shell: sidebar plus the main view, switched by plain state (no router). */
export function App() {
  const [view, setView] = useState<View>("devices");
  const [selectedDeviceId, setSelectedDeviceId] = useState<string | null>(null);
  const [deviceUi, setDeviceUi] = useState<DeviceUi>(initialDeviceUi);
  const devices = useDevices();

  const updateDeviceUi = useCallback((patch: Partial<DeviceUi>) => {
    setDeviceUi((current) => ({ ...current, ...patch }));
  }, []);

  // The flyout asks for a device (and maybe a tab) through the backend's "select-device" event.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    onSelectDevice((id) => {
      setSelectedDeviceId(id);
      setView("devices");
      const tab = takePendingTab();
      if (tab) setDeviceUi((current) => ({ ...current, tab }));
    }).then(
      (fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // With exactly one device there is nothing to choose, so show it right away.
  useEffect(() => {
    if (selectedDeviceId === null && devices?.length === 1 && devices[0]) setSelectedDeviceId(devices[0].id);
  }, [devices, selectedDeviceId]);

  const selected = devices?.find((d) => d.id === selectedDeviceId) ?? null;

  return (
    <div className="grid h-full grid-cols-[240px_minmax(0,1fr)]">
      <Sidebar
        devices={devices}
        selectedDeviceId={selectedDeviceId}
        view={view}
        onSelectDevice={(id) => {
          setSelectedDeviceId(id);
          setView("devices");
        }}
        onNavigate={setView}
      />
      <div className="flex h-full min-h-0 min-w-0 flex-col">
        <UpdateBanner />
        <main className="min-h-0 min-w-0 flex-1 overflow-y-auto overflow-x-hidden p-8">
          <div key={view} className="animate-page h-full">
            {view === "devices" ? <AccessBanner /> : null}
            {view === "devices" ? (
              <DevicesView
                devices={devices}
                selected={selected}
                ui={deviceUi}
                onUiChange={updateDeviceUi}
                onOpenSettings={() => setView("settings")}
              />
            ) : null}
            {view === "settings" ? <SettingsView /> : null}
            {view === "about" ? <AboutView /> : null}
          </div>
        </main>
      </div>
    </div>
  );
}
