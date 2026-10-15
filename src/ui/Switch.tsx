import { cx } from "../lib/cx";

interface SwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** Id of the element that names this switch. */
  labelledBy: string;
  /** Id of the element that describes this switch. */
  describedBy?: string;
  disabled?: boolean;
}

/** On/off switch (role="switch"). Pill track, thumb slides with transform. */
export function Switch({ checked, onChange, labelledBy, describedBy, disabled }: SwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cx(
        "t-colors relative inline-flex h-5 w-9 shrink-0 items-center rounded-pill border disabled:opacity-50",
        checked ? "border-accent bg-accent" : "border-border bg-surface-2",
      )}
    >
      <span
        aria-hidden="true"
        className={cx(
          "t-slide absolute top-0.5 left-0.5 size-3.5 rounded-pill",
          checked ? "bg-accent-fg" : "bg-text-muted",
        )}
        style={{ transform: `translateX(${checked ? 16 : 0}px)` }}
      />
    </button>
  );
}
