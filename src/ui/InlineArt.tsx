import { cx } from "../lib/cx";
import badgeSvg from "../assets/design/badge-read-only.svg?raw";
import lockupSvg from "../assets/design/logo-lockup.svg?raw";
import symbolSvg from "../assets/design/logo-symbol.svg?raw";

interface InlineArtProps {
  /** Trusted SVG markup from src/assets/design (build-time, local). Inline so the CSS variables apply. */
  svg: string;
  /** Box in px. The SVG scales to fit, keeping its viewBox ratio. */
  width: number;
  height: number;
  /** Accessible name. Without it the art is decorative and hidden from assistive tech. */
  label?: string;
  className?: string;
}

/** Inline SVG artwork in a fixed box. */
export function InlineArt({ svg, width, height, label, className }: InlineArtProps) {
  return (
    <span
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      className={cx("inline-block shrink-0 leading-none [&>svg]:block [&>svg]:h-full [&>svg]:w-full", className)}
      style={{ width, height }}
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}

type Sized = { size?: number; label?: string; className?: string };

/** Read-only shield badge (square). */
export function ReadOnlyBadge({ size = 18, label, className }: Sized) {
  return <InlineArt svg={badgeSvg} width={size} height={size} label={label} className={className} />;
}

/** Logo symbol (square). */
export function LogoSymbol({ size = 28, label, className }: Sized) {
  return <InlineArt svg={symbolSvg} width={size} height={size} label={label} className={className} />;
}

/** Logo lockup, symbol plus wordmark (viewBox 264x64); `size` is the height. */
export function LogoLockup({ size = 28, label, className }: Sized) {
  return <InlineArt svg={lockupSvg} width={Math.round((size * 264) / 64)} height={size} label={label} className={className} />;
}
