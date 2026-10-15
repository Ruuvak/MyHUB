import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { resolveLanguage } from "../lib/language";
import { getSettings, onSettingsChanged, setSettings } from "../lib/ipc";
import { applyTheme, watchSystemTheme } from "../lib/theme";
import type { Settings } from "../lib/types";

interface SettingsContextValue {
  /** Null until the first load finishes. */
  settings: Settings | null;
  loadFailed: boolean;
  saveFailed: boolean;
  /** Applies a partial change immediately and persists it through the backend. */
  update: (patch: Partial<Settings>) => void;
  reload: () => void;
}

const SettingsContext = createContext<SettingsContextValue | null>(null);

/** Holds the settings, persists changes, and keeps data-theme and lang on <html> in sync. */
export function SettingsProvider({ children }: { children: ReactNode }) {
  const [settings, setLocal] = useState<Settings | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saveFailed, setSaveFailed] = useState(false);
  const latest = useRef<Settings | null>(null);

  const commit = useCallback((next: Settings) => {
    latest.current = next;
    setLocal(next);
  }, []);

  const reload = useCallback(() => {
    setLoadFailed(false);
    getSettings().then(commit, () => setLoadFailed(true));
  }, [commit]);

  useEffect(() => {
    reload();
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    onSettingsChanged((next) => commit(next)).then(
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
  }, [reload, commit]);

  const update = useCallback(
    (patch: Partial<Settings>) => {
      const previous = latest.current;
      if (!previous) return;
      const next = { ...previous, ...patch };
      setSaveFailed(false);
      commit(next);
      setSettings(next).then(
        (saved) => {
          if (latest.current === next) commit(saved);
        },
        () => {
          if (latest.current === next) commit(previous);
          setSaveFailed(true);
        },
      );
    },
    [commit],
  );

  const theme = settings?.theme ?? "system";
  useEffect(() => {
    applyTheme(theme);
    if (theme !== "system") return;
    return watchSystemTheme(() => applyTheme("system"));
  }, [theme]);

  const language = settings?.language ?? "auto";
  useEffect(() => {
    document.documentElement.lang = resolveLanguage(language);
  }, [language]);

  const value = useMemo(
    () => ({ settings, loadFailed, saveFailed, update, reload }),
    [settings, loadFailed, saveFailed, update, reload],
  );
  return <SettingsContext.Provider value={value}>{children}</SettingsContext.Provider>;
}

/** Reads the settings context. Must be used under SettingsProvider. */
export function useSettings(): SettingsContextValue {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error("useSettings must be used inside SettingsProvider");
  return ctx;
}
