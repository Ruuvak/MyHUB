import { useCallback, useEffect, useRef, useState } from "react";
import { useT } from "../../i18n/useT";
import { errorCode, headsetErrorMessage } from "../../lib/deviceErrors";
import { refreshDevice } from "../../lib/ipc";
import type { DeviceDetail } from "../../lib/types";

export type HeadsetWriteScope = "sound" | "eq" | "settings";

export interface HeadsetWrite {
  /** True while a write runs; every control of the device is disabled meanwhile (one write at a time). */
  pending: boolean;
  /** Last failure and the control group it belongs to. Cleared when the next write starts. */
  error: { scope: HeadsetWriteScope; text: string } | null;
  /** Runs one write. Resolves true when the headset accepted it, false on a rejection (the error is set). */
  run: (scope: HeadsetWriteScope, call: () => Promise<DeviceDetail>) => Promise<boolean>;
}

/**
 * One write at a time for a headset: pending flag, inline error with its scope, translated codes.
 * After "deviceError:timeout" the state is unknown, so the device is re-read.
 */
export function useHeadsetWrite(deviceId: string, onDetail: (next: DeviceDetail) => void): HeadsetWrite {
  const t = useT();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<HeadsetWrite["error"]>(null);
  const alive = useRef(true);
  const busy = useRef(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const run = useCallback(
    (scope: HeadsetWriteScope, call: () => Promise<DeviceDetail>): Promise<boolean> => {
      if (busy.current) return Promise.resolve(false);
      busy.current = true;
      setPending(true);
      setError(null);
      return call().then(
        (fresh) => {
          busy.current = false;
          if (!alive.current) return true;
          onDetail(fresh);
          setPending(false);
          return true;
        },
        (e: unknown) => {
          busy.current = false;
          if (!alive.current) return false;
          setPending(false);
          const { key, vars } = headsetErrorMessage(e);
          setError({ scope, text: t(key, vars) });
          if (errorCode(e) === "deviceError:timeout") {
            refreshDevice(deviceId).then(
              (next) => {
                if (next && alive.current) onDetail(next);
              },
              () => {},
            );
          }
          return false;
        },
      );
    },
    [deviceId, onDetail, t],
  );

  return { pending, error, run };
}
