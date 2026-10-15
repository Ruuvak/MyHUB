import { GameController, Headset, Keyboard, Mouse, type Icon } from "@phosphor-icons/react";
import type { DeviceKind } from "../lib/types";

const icons: Record<DeviceKind, Icon> = {
  mouse: Mouse,
  keyboard: Keyboard,
  headset: Headset,
  gamepad: GameController,
};

/** Phosphor icon for a device kind, 18px by default. */
export function DeviceIcon({ kind, size = 18 }: { kind: DeviceKind; size?: number }) {
  const Component = icons[kind];
  return <Component size={size} weight="regular" aria-hidden="true" />;
}
