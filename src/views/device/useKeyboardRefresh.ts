import { useEffect, useRef } from "react";
import { refreshDevice } from "../../lib/ipc";
import type { DeviceDetail } from "../../lib/types";

/** Shortest gap between two refreshes caused by the window regaining focus. */
const FOCUS_REFRESH_GAP_MS = 5_000;

/**
 * Re-reads the keyboard once when it is shown (the keyboard does not push profile changes) and again
 * when the window regains focus, at most every 5 s. A null answer (backend busy) is ignored.
 */
export function useKeyboardRefresh(id: string, onDetail: (next: DeviceDetail) => void) {
  const onDetailRef = useRef(onDetail);
  onDetailRef.current = onDetail;

  useEffect(() => {
    let active = true;
    let last = 0;
    const run = () => {
      last = Date.now();
      refreshDevice(id).then(
        (next) => {
          if (active && next) onDetailRef.current(next);
        },
        () => {},
      );
    };
    const onFocus = () => {
      if (Date.now() - last >= FOCUS_REFRESH_GAP_MS) run();
    };
    run();
    window.addEventListener("focus", onFocus);
    return () => {
      active = false;
      window.removeEventListener("focus", onFocus);
    };
  }, [id]);
}
