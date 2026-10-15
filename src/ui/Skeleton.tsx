import { cx } from "../lib/cx";

/** Loading placeholder block. Size it with className; pulses unless reduced motion is on. */
export function Skeleton({ className }: { className?: string }) {
  return <div aria-hidden="true" className={cx("animate-skeleton rounded-control bg-surface-2", className)} />;
}
