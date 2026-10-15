import type { ButtonHTMLAttributes } from "react";
import { cx } from "../lib/cx";
import type { ButtonSize, ButtonVariant } from "./Button";

interface IconButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "aria-label"> {
  /** Required: an icon alone has no accessible name. */
  "aria-label": string;
  variant?: ButtonVariant;
  size?: ButtonSize;
}

const variants: Record<ButtonVariant, string> = {
  primary: "bg-accent text-accent-fg hover-fine:opacity-90",
  secondary: "bg-surface-2 text-text border border-border hover-fine:border-text-faint",
  ghost: "bg-transparent text-text-muted hover-fine:bg-surface-2 hover-fine:text-text",
};

const sizes: Record<ButtonSize, string> = {
  sm: "size-8",
  md: "size-9",
};

/** Square icon-only button. Pass a Phosphor icon as the child. */
export function IconButton({ variant = "ghost", size = "md", className, type = "button", ...rest }: IconButtonProps) {
  return (
    <button
      type={type}
      className={cx(
        "pressable inline-flex items-center justify-center rounded-control disabled:opacity-50",
        variants[variant],
        sizes[size],
        className,
      )}
      {...rest}
    />
  );
}
