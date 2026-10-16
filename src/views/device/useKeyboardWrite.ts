import { useCallback, useEffect, useRef, useState } from "react";
import { useT } from "../../i18n/useT";
import { errorCode, keyboardErrorMessage } from "../../lib/deviceErrors";
import { refreshDevice } from "../../lib/ipc";
import type { DeviceDetail } from "../../lib/types";

/**
 * Runs one keyboard write at a time for a card: tracks the pending state and the inline error text, hands the new detail
 * to the parent, and re-reads the keyboard after a timeout (the state is unknown then). `run` resolves to true on success.
 */
export function useKeyboardWrite(id: string, onDetail: (next: DeviceDetail) => void) {
  const t = useT();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const run = useCallback(
    async (call: () => Promise<DeviceDetail>): Promise<boolean> => {
      setPending(true);
      setError(null);
      try {
        const next = await call();
        if (!alive.current) return false;
        setPending(false);
        onDetail(next);
        return true;
      } catch (e: unknown) {
        if (!alive.current) return false;
        setPending(false);
        const { key, vars } = keyboardErrorMessage(e);
        setError(t(key, vars));
        if (errorCode(e) === "deviceError:timeout") {
          refreshDevice(id).then(
            (next) => {
              if (next && alive.current) onDetail(next);
            },
            () => {},
          );
        }
        return false;
      }
    },
    [id, onDetail, t],
  );

  return { pending, error, run };
}
