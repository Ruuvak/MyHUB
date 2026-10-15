import { WarningCircle } from "@phosphor-icons/react";
import type { ReactNode } from "react";
import { useT } from "../i18n/useT";
import type { LanguageSetting, ThemeSetting } from "../lib/types";
import { useSettings } from "../state/settings";
import { Button, Panel, SegmentedControl, Skeleton, SwitchRow } from "../ui";

/** Settings: theme, language and window behavior. Changes apply immediately through setSettings. */
export function SettingsView() {
  const t = useT();
  const { settings, loadFailed, saveFailed, update, reload } = useSettings();

  const themeOptions: Array<{ value: ThemeSetting; label: string }> = [
    { value: "system", label: t("settings.theme.system") },
    { value: "light", label: t("settings.theme.light") },
    { value: "dark", label: t("settings.theme.dark") },
  ];
  const languageOptions: Array<{ value: LanguageSetting; label: string }> = [
    { value: "auto", label: t("settings.language.auto") },
    { value: "pl", label: t("settings.language.pl") },
    { value: "en", label: t("settings.language.en") },
  ];

  return (
    <div className="mx-auto flex w-full max-w-xl flex-col gap-6">
      <h1 className="text-xl font-semibold text-text">{t("settings.title")}</h1>

      {saveFailed ? <ErrorLine>{t("settings.saveError")}</ErrorLine> : null}

      {loadFailed ? (
        <div className="flex flex-col items-start gap-3">
          <ErrorLine>{t("settings.loadError")}</ErrorLine>
          <Button size="sm" onClick={reload}>
            {t("settings.retry")}
          </Button>
        </div>
      ) : settings === null ? (
        <SettingsSkeleton />
      ) : (
        <>
          <Panel title={t("settings.appearance")}>
            <div className="divide-y divide-border px-4">
              <ControlRow label={t("settings.theme")}>
                <SegmentedControl
                  label={t("settings.theme")}
                  value={settings.theme}
                  options={themeOptions}
                  onChange={(theme) => update({ theme })}
                />
              </ControlRow>
              <ControlRow label={t("settings.language")}>
                <SegmentedControl
                  label={t("settings.language")}
                  value={settings.language}
                  options={languageOptions}
                  onChange={(language) => update({ language })}
                />
              </ControlRow>
            </div>
          </Panel>

          <Panel title={t("settings.behavior")}>
            <div className="divide-y divide-border px-4">
              <SwitchRow
                label={t("settings.closeToTray")}
                hint={t("settings.closeToTray.hint")}
                checked={settings.closeToTray}
                onChange={(closeToTray) => update({ closeToTray })}
              />
              <SwitchRow
                label={t("settings.launchAtLogin")}
                hint={t("settings.launchAtLogin.hint")}
                checked={settings.launchAtLogin}
                onChange={(launchAtLogin) => update({ launchAtLogin })}
              />
              <SwitchRow
                label={t("settings.startMinimized")}
                hint={t("settings.startMinimized.hint")}
                checked={settings.startMinimized}
                onChange={(startMinimized) => update({ startMinimized })}
              />
            </div>
          </Panel>

          <Panel title={t("settings.device")}>
            <div className="divide-y divide-border px-4">
              <SwitchRow
                label={t("settings.allowWrites")}
                hint={t("settings.allowWrites.hint")}
                checked={settings.allowDeviceWrites}
                onChange={(allowDeviceWrites) => update({ allowDeviceWrites })}
              />
              <SwitchRow
                label={t("settings.restoreLastMode")}
                hint={t("settings.restoreLastMode.hint")}
                checked={settings.restoreLastMode}
                disabled={!settings.allowDeviceWrites}
                onChange={(restoreLastMode) => update({ restoreLastMode })}
              />
            </div>
          </Panel>
        </>
      )}
    </div>
  );
}

function ErrorLine({ children }: { children: ReactNode }) {
  return (
    <p role="alert" className="flex items-center gap-2 text-base text-danger">
      <WarningCircle size={18} weight="regular" aria-hidden="true" className="shrink-0" />
      {children}
    </p>
  );
}

function ControlRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 py-3">
      <span className="text-base font-medium text-text">{label}</span>
      {children}
    </div>
  );
}

/** Placeholder with the same panel and row layout as the loaded settings. */
function SettingsSkeleton() {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-6">
      <div className="rounded-panel border border-border bg-surface px-4 py-4">
        <Skeleton className="h-5 w-24" />
        <div className="mt-3 flex items-center justify-between gap-4 py-3">
          <Skeleton className="h-5 w-20" />
          <Skeleton className="h-8 w-60" />
        </div>
        <div className="flex items-center justify-between gap-4 border-t border-border py-3">
          <Skeleton className="h-5 w-20" />
          <Skeleton className="h-8 w-60" />
        </div>
      </div>
      {[28, 36].map((titleWidth) => (
        <div key={titleWidth} className="rounded-panel border border-border bg-surface px-4 py-4">
          <Skeleton className={titleWidth === 28 ? "h-5 w-28" : "h-5 w-36"} />
          <div className="mt-3 flex items-center justify-between gap-4 py-3">
            <div className="flex flex-col gap-2">
              <Skeleton className="h-5 w-40" />
              <Skeleton className="h-4 w-64" />
            </div>
            <Skeleton className="h-5 w-9" />
          </div>
          <div className="flex items-center justify-between gap-4 border-t border-border py-3">
            <div className="flex flex-col gap-2">
              <Skeleton className="h-5 w-44" />
              <Skeleton className="h-4 w-56" />
            </div>
            <Skeleton className="h-5 w-9" />
          </div>
        </div>
      ))}
    </div>
  );
}
