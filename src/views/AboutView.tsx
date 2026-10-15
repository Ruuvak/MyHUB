import { useEffect, useState } from "react";
import { useT } from "../i18n/useT";
import { appInfo } from "../lib/ipc";
import type { AppInfo } from "../lib/types";
import { Panel, Skeleton } from "../ui";

/** About: app name, version, what the app does and the safety note. */
export function AboutView() {
  const t = useT();
  const [info, setInfo] = useState<AppInfo | null>(null);

  useEffect(() => {
    let active = true;
    appInfo().then(
      (value) => {
        if (active) setInfo(value);
      },
      () => {
        if (active) setInfo({ name: "MyHUB", version: "?" });
      },
    );
    return () => {
      active = false;
    };
  }, []);

  return (
    <div className="mx-auto flex w-full max-w-xl flex-col gap-6">
      <h1 className="text-xl font-semibold text-text">{t("about.title")}</h1>
      <Panel>
        <div className="flex flex-col gap-4 p-4">
          <div className="flex items-baseline justify-between gap-4">
            <span className="text-lg font-semibold text-text">{info?.name ?? "MyHUB"}</span>
            <span className="flex items-baseline gap-2 text-sm text-text-muted">
              {t("about.version")}
              {info ? (
                <span className="font-mono tabular-nums text-text">{info.version}</span>
              ) : (
                <Skeleton className="h-4 w-12" />
              )}
            </span>
          </div>
          <p className="text-base text-text-muted">{t("about.description")}</p>
          <p className="border-t border-border pt-4 text-base text-text">{t("about.safety")}</p>
        </div>
      </Panel>
    </div>
  );
}
