import { ArrowClockwise } from "@phosphor-icons/react";
import { useT } from "../../i18n/useT";
import { cx } from "../../lib/cx";
import { IconButton } from "../../ui";

/** Refresh icon button; the icon spins while the refresh is pending (not under reduced motion). */
export function RefreshButton({
  pending,
  onClick,
  disabled,
  size = "md",
}: {
  pending: boolean;
  onClick: () => void;
  disabled?: boolean;
  size?: "sm" | "md";
}) {
  const t = useT();
  return (
    <IconButton
      aria-label={t("action.refresh")}
      title={t("action.refresh")}
      size={size}
      aria-busy={pending}
      disabled={disabled || pending}
      onClick={onClick}
    >
      <ArrowClockwise size={18} weight="regular" aria-hidden="true" className={cx(pending && "animate-spin-slow")} />
    </IconButton>
  );
}
