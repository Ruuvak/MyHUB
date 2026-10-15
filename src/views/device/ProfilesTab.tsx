import { useT } from "../../i18n/useT";
import { cx } from "../../lib/cx";
import { customProfileName, profileTitle } from "../../lib/device";
import type { DeviceDetail, ProfileInfo } from "../../lib/types";
import type { DeviceUi } from "../../state/deviceUi";
import { Badge, Panel } from "../../ui";
import { resolveProfile } from "./ButtonsTab";

/** Profiles tab: user profiles, one divider, then the factory profiles. Selecting a row picks it for the Buttons tab. */
export function ProfilesTab({
  detail,
  ui,
  onUiChange,
}: {
  detail: DeviceDetail;
  ui: DeviceUi;
  onUiChange: (patch: Partial<DeviceUi>) => void;
}) {
  const t = useT();
  const onboard = detail.onboard;
  if (!onboard) return <p className="text-base text-text-muted">{t("buttons.none")}</p>;

  const selectedId = resolveProfile(detail, ui.profileId)?.id ?? null;
  const user = onboard.profiles.filter((p) => !p.rom);
  const rom = onboard.profiles.filter((p) => p.rom);

  const renderRow = (profile: ProfileInfo) => (
    <li key={profile.id}>
      <ProfileRow
        profile={profile}
        active={!profile.rom && profile.number === onboard.activeProfile}
        selected={profile.id === selectedId}
        onSelect={() => onUiChange({ profileId: profile.id })}
      />
    </li>
  );

  return (
    <Panel title={t("profiles.title")}>
      <p className="px-4 pb-2 text-sm text-text-muted">{t("profiles.hint")}</p>
      <div className="px-2 pb-2">
        <ul aria-label={t("profiles.user")} className="flex flex-col gap-0.5">
          {user.map(renderRow)}
        </ul>
        {rom.length > 0 ? (
          <>
            <div role="separator" className="mx-3 my-2 h-px bg-border" />
            <ul aria-label={t("profiles.rom")} className="flex flex-col gap-0.5">
              {rom.map(renderRow)}
            </ul>
          </>
        ) : null}
      </div>
    </Panel>
  );
}

function ProfileRow({
  profile,
  active,
  selected,
  onSelect,
}: {
  profile: ProfileInfo;
  active: boolean;
  selected: boolean;
  onSelect: () => void;
}) {
  const t = useT();
  const customName = customProfileName(profile);
  const defaultDpi = profile.dpiLevels[profile.defaultDpiIndex];
  return (
    <button
      type="button"
      aria-pressed={selected}
      onClick={onSelect}
      className={cx(
        "t-colors grid w-full grid-cols-[8px_20px_minmax(0,1fr)_auto_auto] items-center gap-x-3 rounded-control px-3 py-2.5 text-left",
        selected ? "bg-surface-2" : "hover-fine:bg-surface-2",
        profile.rom || !profile.enabled ? "text-text-muted" : "text-text",
      )}
    >
      <span aria-hidden="true" className={cx("size-2 rounded-pill", active ? "bg-accent" : "bg-transparent")} />
      <span className="font-mono text-sm tabular-nums text-text-muted">{profile.number}</span>
      <span className="flex min-w-0 flex-col gap-1">
        <span className="truncate text-base font-medium">
          {customName ?? profileTitle(profile, t)}
        </span>
        <span className="flex flex-wrap items-center gap-1.5">
          <Badge>{profile.enabled ? t("profile.enabled") : t("profile.disabled")}</Badge>
          {active ? <Badge tone="accent">{t("profile.active")}</Badge> : null}
        </span>
      </span>
      <span className="w-[72px] text-right font-mono text-sm tabular-nums">
        {profile.reportRateHz !== null ? `${profile.reportRateHz} Hz` : "--"}
      </span>
      <span className="w-[88px] text-right font-mono text-sm tabular-nums">
        {defaultDpi !== undefined ? `${defaultDpi} DPI` : "--"}
      </span>
    </button>
  );
}
