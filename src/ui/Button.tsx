import type { ButtonHTMLAttributes } from "react";
import { cx } from "../lib/cx";

export type ButtonVariant = "primary" | "secondary" | "ghost";
export type ButtonSize = "sm" | "md";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
}

const variants: Record<ButtonVariant, string> = {
  primary: "bg-accent text-accent-fg hover-fine:opacity-90",
  secondary: "bg-surface-2 text-text border border-border hover-fine:border-text-faint",
  ghost: "bg-transparent text-text-muted hover-fine:bg-surface-2 hover-fine:text-text",
};

const sizes: Record<ButtonSize, string> = {
  sm: "h-8 px-3 text-sm gap-1.5",
  md: "h-9 px-4 text-base gap-2",
};

/** Text button with primary, secondary and ghost variants; presses in with a small scale. */
export function Button({ variant = "secondary", size = "md", className, type = "button", ...rest }: ButtonProps) {
  return (
    <button
      type={type}
      className={cx(
        "pressable inline-flex items-center justify-center rounded-control font-medium whitespace-nowrap disabled:opacity-50",
        variants[variant],
        sizes[size],
        className,
      )}
      {...rest}
    />
  );
}
