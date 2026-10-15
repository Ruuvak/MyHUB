import type { ReactNode, SVGProps } from "react";

export interface CustomIconProps extends Omit<SVGProps<SVGSVGElement>, "children" | "width" | "height"> {
  /** Width and height in px. Defaults to 1em, like the Phosphor icons. */
  size?: number | string;
}

/**
 * Base for the custom icons: 256 grid, 16 unit round strokes in currentColor, matching Phosphor "regular".
 * Hidden from assistive tech unless the caller passes aria-label or aria-hidden={false}.
 */
export function CustomIcon({
  size = "1em",
  children,
  ...rest
}: CustomIconProps & { children: ReactNode }) {
  const labelled = rest["aria-label"] !== undefined || rest["aria-labelledby"] !== undefined;
  return (
    <svg
      viewBox="0 0 256 256"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth={16}
      strokeLinecap="round"
      strokeLinejoin="round"
      focusable="false"
      aria-hidden={labelled ? undefined : true}
      role={labelled ? "img" : undefined}
      {...rest}
    >
      {children}
    </svg>
  );
}
