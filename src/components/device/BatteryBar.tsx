import { cx } from "../../lib/cx";
import { batteryTone } from "../../lib/device";

const fills = { accent: "bg-accent", warning: "bg-warning", danger: "bg-danger" } as const;

/** Thin bar: surface-2 track, fill in accent (warning at 20% or less, danger at 10% or less). Scales with transform. */
export function BatteryBar({ percent, className }: { percent: number; className?: string }) {
  const clamped = Math.min(100, Math.max(0, percent));
  return (
    <div aria-hidden="true" className={cx("h-1.5 w-full overflow-hidden rounded-pill bg-surface-2", className)}>
      <div
        className={cx("t-slide h-full w-full origin-left rounded-pill", fills[batteryTone(clamped)])}
        style={{ transform: `scaleX(${clamped / 100})` }}
      />
    </div>
  );
}
