import { useId, useState } from "react";
import { DeviceIllustration } from "../../components/device/DeviceIllustration";
import { resolveIllustration } from "../../components/illustrations";
import { useT } from "../../i18n/useT";
import { bindingLabel, slotLabel } from "../../lib/bindings";
import { cx } from "../../lib/cx";
import { customProfileName, profileTitle } from "../../lib/device";
import type { DeviceDetail, ProfileInfo, SpecialAction } from "../../lib/types";
import type { DeviceUi } from "../../state/deviceUi";
import { Badge, Panel, SegmentedControl, Switch } from "../../ui";
import {
  DpiIcon,
  DpiShiftIcon,
  GShiftIcon,
  ProfileCycleIcon,
  TiltLeftIcon,
  TiltRightIcon,
  type CustomIconProps,
} from "../../ui/icons";

/** Custom icon per special action; DPI up and down share the DPI glyph. */
const SPECIAL_ICONS: Record<SpecialAction, (props: CustomIconProps) => React.JSX.Element> = {
  dpiShift: DpiShiftIcon,
  gShift: GShiftIcon,
  wheelLeft: TiltLeftIcon,
  wheelRight: TiltRightIcon,
  profileCycle: ProfileCycleIcon,
  dpiUp: DpiIcon,
  dpiDown: DpiIcon,
};

interface ButtonsTabProps {
  detail: DeviceDetail;
  ui: DeviceUi;
  onUiChange: (patch: Partial<DeviceUi>) => void;
}

/** Resolves which profile the Buttons tab shows: the chosen one, else the active one, else the first enabled. */
export function resolveProfile(detail: DeviceDetail, profileId: string | null): ProfileInfo | null {
  const onboard = detail.onboard;
  if (!onboard) return null;
  const { profiles, activeProfile } = onboard;
  return (
    profiles.find((p) => p.id === profileId) ??
    profiles.find((p) => !p.rom && p.number === activeProfile) ??
    profiles.find((p) => p.enabled) ??
    profiles[0] ??
    null
  );
}

/** Buttons tab: profile and layer pickers, the numbered mouse map and the slot list. Read-only. */
export function ButtonsTab({ detail, ui, onUiChange }: ButtonsTabProps) {
  const t = useT();
  const switchLabelId = useId();
  const [hoverSlot, setHoverSlot] = useState<number | null>(null);
  const onboard = detail.onboard;
  const profile = resolveProfile(detail, ui.profileId);

  if (!onboard || !profile) {
    return <p className="text-base text-text-muted">{t("buttons.none")}</p>;
  }

  const profileOptions = onboard.profiles
    .filter((p) => ui.showDisabled || p.enabled || p.id === profile.id)
    .map((p) => ({ value: p.id, label: p.rom ? t("profile.factoryShort", { n: p.number }) : String(p.number) }));

  const layerOptions = [
    { value: "base" as const, label: t("buttons.layer.base") },
    { value: "gshift" as const, label: t("buttons.layer.gshift") },
  ];

  const bindings = (ui.layer === "gshift" ? profile.gshift : profile.buttons)
    .filter((b) => b.slot < onboard.buttonCount)
    .sort((a, b) => a.slot - b.slot);
  const showMap = resolveIllustration(detail.illustration, "top") !== null;
  const customName = customProfileName(profile);
  const active = !profile.rom && profile.number === onboard.activeProfile;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-x-5 gap-y-3">
        <div className="flex items-center gap-2">
          <span className="text-sm text-text-muted">{t("buttons.profile")}</span>
          <SegmentedControl
            label={t("buttons.profile")}
            value={profile.id}
            options={profileOptions}
            onChange={(profileId) => onUiChange({ profileId })}
          />
        </div>
        <div className="flex items-center gap-2">
          <Switch
            checked={ui.showDisabled}
            onChange={(showDisabled) => onUiChange({ showDisabled })}
            labelledBy={switchLabelId}
          />
          <span id={switchLabelId} className="text-sm text-text-muted">
            {t("buttons.showDisabled")}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-sm text-text-muted">{t("buttons.layer")}</span>
          <SegmentedControl
            label={t("buttons.layer")}
            value={ui.layer}
            options={layerOptions}
            onChange={(layer) => onUiChange({ layer })}
          />
        </div>
      </div>

      <Panel>
        <div className="flex flex-wrap items-center gap-2 px-4 pt-4">
          <h2 className="text-base font-semibold text-text">{profileTitle(profile, t)}</h2>
          {customName ? <span className="text-sm text-text-muted">{customName}</span> : null}
          {active ? <Badge tone="accent">{t("profile.active")}</Badge> : null}
          {!profile.enabled ? <Badge>{t("profile.disabled")}</Badge> : null}
        </div>
        <div className="flex flex-wrap items-start gap-x-8 gap-y-4 p-4">
          {showMap ? (
            <div className="mx-auto shrink-0 sm:mx-0">
              <DeviceIllustration
                illustration={detail.illustration}
                kind={detail.kind}
                view="top"
                size={340}
                showMarkers
                highlightSlot={hoverSlot}
                onSlotHover={setHoverSlot}
                label={t("buttons.map")}
                markerLabel={(slot) => {
                  const binding = bindings.find((b) => b.slot === slot);
                  return binding ? `${slotLabel(slot)}: ${bindingLabel(binding, t)}` : slotLabel(slot);
                }}
              />
            </div>
          ) : null}
          <ul aria-label={t("buttons.list")} className="flex min-w-[220px] flex-1 flex-col gap-0.5">
            {bindings.map((binding) => {
              const highlighted = hoverSlot === binding.slot;
              return (
                <li
                  key={binding.slot}
                  tabIndex={0}
                  onMouseEnter={() => setHoverSlot(binding.slot)}
                  onMouseLeave={() => setHoverSlot(null)}
                  onFocus={() => setHoverSlot(binding.slot)}
                  onBlur={() => setHoverSlot(null)}
                  className={cx(
                    "t-colors flex items-center gap-3 rounded-control px-3 py-1.5",
                    highlighted && "bg-surface-2",
                  )}
                >
                  <span className="w-8 shrink-0 font-mono text-sm tabular-nums text-text-muted">
                    {slotLabel(binding.slot)}
                  </span>
                  <span
                    className={cx(
                      "min-w-0 truncate text-base",
                      binding.kind === "unassigned" ? "text-text-faint" : "text-text",
                      binding.kind === "unknown" && "font-mono",
                    )}
                  >
                    {bindingLabel(binding, t)}
                  </span>
                  {binding.kind === "special" && binding.special ? <SpecialIcon special={binding.special} /> : null}
                </li>
              );
            })}
          </ul>
        </div>
      </Panel>
    </div>
  );
}

function SpecialIcon({ special }: { special: SpecialAction }) {
  const Icon = SPECIAL_ICONS[special];
  return <Icon size={16} className="shrink-0 text-text-faint" />;
}
