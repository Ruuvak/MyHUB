import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useT } from "../i18n/useT";
import { errorCode } from "../lib/deviceErrors";
import { checkForUpdate, installUpdate, onCheckUpdates, onUpdateProgress } from "../lib/ipc";
import type { UpdateInfo, UpdateProgress } from "../lib/types";
import { Button } from "../ui";

/** Delay before the silent check on start, so it never competes with the first device read. */
const STARTUP_CHECK_MS = 3000;

type State =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "available"; info: UpdateInfo }
  | { kind: "installing"; info: UpdateInfo; progress: UpdateProgress | null }
  | { kind: "latest" }
  | { kind: "error"; message: string };

/** Update notice above the main view: silent check on start, manual check from the tray. */
export function UpdateBanner() {
  const t = useT();
  const [state, setState] = useState<State>({ kind: "idle" });
  const busy = useRef(false);
  const installingRef = useRef(false);

  const check = useCallback(async (manual: boolean) => {
    if (busy.current || installingRef.current) return;
    busy.current = true;
    if (manual) setState({ kind: "checking" });
    try {
      const info = await checkForUpdate();
      if (info) setState({ kind: "available", info });
      else setState(manual ? { kind: "latest" } : { kind: "idle" });
    } catch (error) {
      if (manual) setState({ kind: "error", message: errorCode(error) });
      else {
        console.error("update check failed", error);
        setState({ kind: "idle" });
      }
    } finally {
      busy.current = false;
    }
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => void check(false), STARTUP_CHECK_MS);
    return () => window.clearTimeout(timer);
  }, [check]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    onCheckUpdates(() => void check(true)).then(
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
  }, [check]);

  const installing = state.kind === "installing";
  useEffect(() => {
    if (!installing) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    onUpdateProgress((progress) => {
      setState((current) => (current.kind === "installing" ? { ...current, progress } : current));
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
  }, [installing]);

  const install = (info: UpdateInfo) => {
    if (installingRef.current) return;
    installingRef.current = true;
    setState({ kind: "installing", info, progress: null });
    installUpdate().catch((error: unknown) => {
      installingRef.current = false;
      setState({ kind: "error", message: errorCode(error) });
    });
  };

  const dismiss = () => setState({ kind: "idle" });

  if (state.kind === "idle") return null;

  let text: string;
  let actions: ReactNode = null;
  switch (state.kind) {
    case "checking":
      text = t("update.checking");
      break;
    case "available": {
      const { info } = state;
      text = t("update.available", { version: info.version });
      actions = (
        <>
          <Button variant="primary" size="sm" onClick={() => install(info)}>
            {t("update.install")}
          </Button>
          <Button variant="ghost" size="sm" onClick={dismiss}>
            {t("update.later")}
          </Button>
        </>
      );
      break;
    }
    case "installing": {
      const { progress } = state;
      text =
        progress?.total && progress.total > 0
          ? t("update.downloadingPercent", {
              percent: Math.min(100, Math.floor((progress.downloaded / progress.total) * 100)),
            })
          : t("update.downloading");
      break;
    }
    case "latest":
      text = t("update.latest");
      actions = (
        <Button variant="ghost" size="sm" onClick={dismiss}>
          {t("update.dismiss")}
        </Button>
      );
      break;
    case "error":
      text = t("update.error", { message: state.message });
      actions = (
        <Button variant="ghost" size="sm" onClick={dismiss}>
          {t("update.dismiss")}
        </Button>
      );
      break;
  }

  return (
    <div
      role="status"
      className="flex items-center justify-between gap-4 border-b border-border bg-surface px-8 py-3 text-sm text-text"
    >
      <span className="min-w-0 break-words">{text}</span>
      <div className="flex shrink-0 items-center gap-2">{actions}</div>
    </div>
  );
}
