import { useId } from "react";
import { cx } from "../lib/cx";
import { Switch } from "./Switch";

/** Label and hint on the left, switch on the right. */
export function SwitchRow({
  label,
  hint,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  hint?: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}) {
  const id = useId();
  const labelId = `${id}-label`;
  const hintId = `${id}-hint`;
  return (
    <div className="flex items-center justify-between gap-4 py-3">
      <div className="min-w-0">
        <div id={labelId} className={cx("text-base font-medium text-text", disabled && "opacity-60")}>
          {label}
        </div>
        {hint ? (
          <div id={hintId} className="text-sm text-text-muted">
            {hint}
          </div>
        ) : null}
      </div>
      <Switch checked={checked} onChange={onChange} labelledBy={labelId} describedBy={hint ? hintId : undefined} disabled={disabled} />
    </div>
  );
}
