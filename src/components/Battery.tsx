import { useT } from "../i18n/useT";
import { cx } from "../lib/cx";

/** Battery percentage in mono. Turns warning at 20% or less and danger at 10% or less. */
export function Battery({ percent }: { percent: number }) {
  const t = useT();
  const tone = percent <= 10 ? "text-danger" : percent <= 20 ? "text-warning" : "text-text-muted";
  return (
    <span className={cx("font-mono text-sm tabular-nums", tone)}>
      <span aria-hidden="true">{percent}%</span>
      <span className="sr-only">{t("devices.battery", { n: percent })}</span>
    </span>
  );
}
