import { useCallback, useEffect, useRef, useState } from "react";
import { getDeviceDetail, listDevices, onDeviceDetailChanged, onDevicesChanged, refreshDevice } from "../lib/ipc";
import type { DeviceDetail, DeviceSummary } from "../lib/types";

/** Runs an async subscribe function and unsubscribes on cleanup, even if cleanup comes first. */
function useSubscription(subscribe: () => Promise<() => void>, deps: unknown[]) {
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    subscribe().then(
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
  }, deps);
}

/**
 * Loads the device list on mount, again whenever the window regains focus, and on every
 * "devices-changed" event. Null while loading.
 */
export function useDevices(): DeviceSummary[] | null {
  const [devices, setDevices] = useState<DeviceSummary[] | null>(null);

  useEffect(() => {
    let active = true;
    const load = () => {
      listDevices().then(
        (list) => {
          if (active) setDevices(list);
        },
        () => {
          if (active) setDevices((current) => current ?? []);
        },
      );
    };
    load();
    window.addEventListener("focus", load);
    return () => {
      active = false;
      window.removeEventListener("focus", load);
    };
  }, []);

  useSubscription(() => onDevicesChanged((list) => setDevices(list)), []);

  return devices;
}

export interface DeviceDetailState {
  /** Undefined while the first load runs, null when the backend has no detail for this id. */
  detail: DeviceDetail | null | undefined;
  /** True while a manual refresh is pending. */
  refreshing: boolean;
  refresh: () => void;
  /** Replaces the detail with a fresh one returned by a write command. */
  applyDetail: (next: DeviceDetail) => void;
}

/** Detail for one device: loads on id change and window focus, follows "device-detail-changed". */
export function useDeviceDetail(id: string | null): DeviceDetailState {
  const [detail, setDetail] = useState<DeviceDetail | null | undefined>(undefined);
  const [refreshing, setRefreshing] = useState(false);
  const currentId = useRef(id);
  currentId.current = id;

  useEffect(() => {
    setDetail(undefined);
    setRefreshing(false);
    if (id === null) return;
    let active = true;
    const load = () => {
      getDeviceDetail(id).then(
        (value) => {
          if (active) setDetail(value);
        },
        () => {
          if (active) setDetail((current) => current ?? null);
        },
      );
    };
    load();
    window.addEventListener("focus", load);
    return () => {
      active = false;
      window.removeEventListener("focus", load);
    };
  }, [id]);

  useSubscription(
    () =>
      onDeviceDetailChanged((next) => {
        if (next.id === currentId.current) setDetail(next);
      }),
    [],
  );

  const refresh = useCallback(() => {
    const target = currentId.current;
    if (target === null || refreshing) return;
    setRefreshing(true);
    refreshDevice(target).then(
      (next) => {
        if (currentId.current !== target) return;
        if (next) setDetail(next);
        setRefreshing(false);
      },
      () => {
        if (currentId.current === target) setRefreshing(false);
      },
    );
  }, [refreshing]);

  const applyDetail = useCallback((next: DeviceDetail) => {
    if (next.id === currentId.current) setDetail(next);
  }, []);

  return { detail, refreshing, refresh, applyDetail };
}

/** Re-renders every `intervalMs` and returns the current time in ms, for "updated ago" text. */
export function useNow(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(timer);
  }, [intervalMs]);
  return now;
}
