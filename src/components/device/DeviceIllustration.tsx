import { useId } from "react";
import { cx } from "../../lib/cx";
import type { DeviceKind } from "../../lib/types";
import { DeviceIcon } from "../DeviceIcon";
import { resolveIllustration, type IllustrationDef, type IllustrationView } from "../illustrations";

interface DeviceIllustrationProps {
  /** UI illustration key from the device detail ("g502x"). Unknown or null falls back to the kind icon. */
  illustration: string | null | undefined;
  kind: DeviceKind;
  /** "hero" sits on the accent glow, "top" is the bare artwork (used for the button map). */
  view?: IllustrationView;
  /** Artwork height in px. */
  size: number;
  /** Slot index (0..10) to highlight, found in the artwork by its `slot-g{n}` id. */
  highlightSlot?: number | null;
  /** Numbered markers at the slot anchors. */
  showMarkers?: boolean;
  /** Called with the slot on marker hover or focus, and with null when it ends. */
  onSlotHover?: (slot: number | null) => void;
  /** Accessible name of the whole map when markers are shown. */
  label?: string;
  /** Accessible name per marker, e.g. "G1: Left click". */
  markerLabel?: (slot: number) => string;
  dimmed?: boolean;
  /** Switched off or asleep: the hero render is shown faded and slightly desaturated, without the glow. */
  asleep?: boolean;
  /** Narrower glow, for left-aligned rows. */
  compact?: boolean;
  className?: string;
}

/**
 * One entry point for device artwork. The artwork itself (raster render for the hero, inline SVG for the button map) comes from the registry in illustrations/index.ts.
 * Highlighting is a style rule on `#slot-g{n}` inside this component's root, so any artwork that carries
 * those ids works. Markers are drawn here from the artwork's anchor data.
 */
export function DeviceIllustration({
  illustration,
  kind,
  view = "hero",
  size,
  highlightSlot = null,
  showMarkers = false,
  onSlotHover,
  label,
  markerLabel,
  dimmed,
  asleep,
  compact,
  className,
}: DeviceIllustrationProps) {
  const rootId = `illo-${useId().replace(/[^a-zA-Z0-9]/g, "")}`;
  const sleeping = asleep === true && view === "hero";
  const def = resolveIllustration(illustration, view, kind);
  const artWidth = def ? Math.round((size * def.viewBox.width) / def.viewBox.height) : size;

  const art = def?.raster ? (
    <img
      src={def.raster.src1x}
      srcSet={`${def.raster.src1x} 1x, ${def.raster.src2x} 2x`}
      alt=""
      aria-hidden="true"
      width={artWidth}
      height={size}
      decoding="async"
      draggable={false}
      className={cx("block shrink-0 select-none", sleeping && "[filter:grayscale(.6)] opacity-55")}
    />
  ) : def?.svg ? (
    <div data-illo={rootId} className="relative shrink-0" style={{ width: artWidth, height: size }}>
      {/* Decorative: the art carries no text. Inline so the --illu-* variables and the slot ids work. */}
      <div
        aria-hidden="true"
        className="h-full w-full [&>svg]:block [&>svg]:h-full [&>svg]:w-full"
        dangerouslySetInnerHTML={{ __html: def.svg }}
      />
      {highlightSlot !== null ? (
        // Targets the artwork's own part by id. Scoped to this root so several instances do not interfere.
        <style>{`[data-illo="${rootId}"] [id="slot-g${highlightSlot + 1}"]{fill:color-mix(in srgb,var(--accent) 20%,transparent);stroke:var(--accent);stroke-opacity:1;stroke-width:2px}`}</style>
      ) : null}
      {showMarkers ? (
        <Markers
          def={def}
          highlightSlot={highlightSlot}
          onSlotHover={onSlotHover}
          label={label}
          markerLabel={markerLabel}
        />
      ) : null}
    </div>
  ) : (
    <span className="text-text-muted">
      <DeviceIcon kind={kind} size={Math.round(size / 2)} />
    </span>
  );

  if (view === "top") return <div className={cx(dimmed && "opacity-50", className)}>{art}</div>;

  const glowHeight = Math.round(size * 1.04);
  return (
    <div
      className={cx("relative flex shrink-0 items-center justify-center", className)}
      style={{ width: Math.max(artWidth, Math.round(glowHeight * (compact ? 1.1 : 1.5))), height: glowHeight }}
    >
      {sleeping ? null : <div aria-hidden="true" className="device-glow absolute inset-0" />}
      <div className={cx("relative", dimmed && !sleeping && "opacity-50")}>{art}</div>
    </div>
  );
}

function Markers({
  def,
  highlightSlot,
  onSlotHover,
  label,
  markerLabel,
}: {
  def: IllustrationDef;
  highlightSlot: number | null;
  onSlotHover?: (slot: number | null) => void;
  label?: string;
  markerLabel?: (slot: number) => string;
}) {
  const interactive = onSlotHover !== undefined;
  return (
    <svg
      viewBox={`0 0 ${def.viewBox.width} ${def.viewBox.height}`}
      className="pointer-events-none absolute inset-0 h-full w-full"
      role="group"
      aria-label={label}
      focusable="false"
    >
      {Object.entries(def.slots).map(([key, point]) => {
        const slot = Number(key);
        const active = highlightSlot === slot;
        return (
          <g
            key={slot}
            className="g502-marker pointer-events-auto"
            transform={`translate(${point.x} ${point.y})`}
            tabIndex={interactive ? 0 : undefined}
            role={interactive ? "img" : undefined}
            aria-label={interactive ? markerLabel?.(slot) : undefined}
            aria-hidden={interactive ? undefined : true}
            onMouseEnter={interactive ? () => onSlotHover(slot) : undefined}
            onMouseLeave={interactive ? () => onSlotHover(null) : undefined}
            onFocus={interactive ? () => onSlotHover(slot) : undefined}
            onBlur={interactive ? () => onSlotHover(null) : undefined}
          >
            <circle className="marker-ring" r="10" fill="none" stroke="var(--accent)" strokeWidth="2" />
            <circle
              r="7.5"
              className="svg-fade"
              fill={active ? "var(--accent)" : "var(--surface)"}
              stroke={active ? "var(--accent)" : "var(--text-faint)"}
              strokeWidth="1.25"
            />
            <text
              textAnchor="middle"
              dominantBaseline="central"
              fontSize="8.5"
              fontWeight="600"
              fill={active ? "var(--accent-fg)" : "var(--text-muted)"}
              style={{ fontFamily: "var(--font-mono)", fontVariantNumeric: "tabular-nums" }}
            >
              {slot + 1}
            </text>
          </g>
        );
      })}
    </svg>
  );
}
