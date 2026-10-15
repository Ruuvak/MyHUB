import { CustomIcon, type CustomIconProps } from "./CustomIcon";

/** Custom icons from assets/design/icon-*.svg, one component each. Paths are copied from the source files. */

export type { CustomIconProps };

export const DpiIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <path d="M40 128H216M72 96L40 128L72 160M184 96L216 128L184 160M128 100V156M96 114V142M160 114V142" />
  </CustomIcon>
);

export const DpiShiftIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <circle cx="128" cy="128" r="56" />
    <path d="M128 24V64M128 192V232M24 128H64M192 128H232" />
    <circle cx="128" cy="128" r="8" />
  </CustomIcon>
);

export const GShiftIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <path d="M128 96L216 136L128 176L40 136Z" />
    <path d="M40 176L128 216L216 176" />
    <path d="M96 56L128 24L160 56" />
  </CustomIcon>
);

export const ReportRateIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <path d="M24 208H232M48 208V132M96 208V92M144 208V132M192 208V172" />
    <circle cx="48" cy="120" r="12" />
    <circle cx="96" cy="80" r="12" />
    <circle cx="144" cy="120" r="12" />
    <circle cx="192" cy="160" r="12" />
  </CustomIcon>
);

export const HostModeIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <rect x="24" y="48" width="88" height="72" rx="10" />
    <rect x="144" y="48" width="88" height="72" rx="10" />
    <path d="M64 176H192M88 152L64 176L88 200M168 152L192 176L168 200" />
  </CustomIcon>
);

export const MemoryProfileIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <rect x="64" y="64" width="128" height="128" rx="16" />
    <path d="M104 24V64M152 24V64M104 192V232M152 192V232M24 104H64M24 152H64M192 104H232M192 152H232M108 108H148V156L128 142L108 156Z" />
  </CustomIcon>
);

export const ProfileCycleIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <path d="M49 114A80 80 0 0 1 207 114M167 114H207V74M207 142A80 80 0 0 1 49 142M89 142H49V182" />
    <circle cx="128" cy="128" r="20" />
  </CustomIcon>
);

export const TiltLeftIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <rect x="96" y="24" width="64" height="112" rx="32" />
    <path d="M128 56V88M216 192H56M96 152L56 192L96 232" />
  </CustomIcon>
);

export const TiltRightIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <rect x="96" y="24" width="64" height="112" rx="32" />
    <path d="M128 56V88M40 192H200M160 152L200 192L160 232" />
  </CustomIcon>
);

export const WirelessReceiverIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <rect x="32" y="116" width="36" height="24" rx="4" />
    <rect x="68" y="100" width="100" height="56" rx="16" />
    <path d="M202 100A44 44 0 0 1 202 156M223 82A72 72 0 0 1 223 174" />
  </CustomIcon>
);

const BatteryShell = () => (
  <>
    <rect x="24" y="72" width="184" height="112" rx="24" />
    <path d="M232 112V144" />
  </>
);

export const BatteryFullIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <BatteryShell />
    <path d="M72 104V152" />
    <path d="M116 104V152" />
    <path d="M160 104V152" />
  </CustomIcon>
);

export const BatteryHalfIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <BatteryShell />
    <path d="M72 104V152" />
    <path d="M116 104V152" />
  </CustomIcon>
);

export const BatteryLowIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <BatteryShell />
    <path d="M72 104V152" />
  </CustomIcon>
);

export const BatteryChargingIcon = (props: CustomIconProps) => (
  <CustomIcon {...props}>
    <BatteryShell />
    <path d="M136 92L104 132H152L120 172" />
  </CustomIcon>
);

/** Battery glyph by level: full above 60, half 21 to 60, low at 20 or less. */
export function BatteryLevelIcon({ percent, charging, ...props }: CustomIconProps & { percent: number; charging?: boolean }) {
  if (charging) return <BatteryChargingIcon {...props} />;
  if (percent > 60) return <BatteryFullIcon {...props} />;
  if (percent > 20) return <BatteryHalfIcon {...props} />;
  return <BatteryLowIcon {...props} />;
}
