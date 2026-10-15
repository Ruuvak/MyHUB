import type { DeviceKind } from "../../lib/types";
import { G502X_SLOTS, G502X_VIEWBOX, type Point } from "./g502x.slots";
import mouseTop from "../../assets/design/mouse-top.svg?raw";
import earbuds1x from "../../assets/design/renders/earbuds@1x.webp";
import earbuds2x from "../../assets/design/renders/earbuds@2x.webp";
import gamepad1x from "../../assets/design/renders/gamepad@1x.webp";
import gamepad2x from "../../assets/design/renders/gamepad@2x.webp";
import headset1x from "../../assets/design/renders/headset@1x.webp";
import headset2x from "../../assets/design/renders/headset@2x.webp";
import keyboard1x from "../../assets/design/renders/keyboard@1x.webp";
import keyboard2x from "../../assets/design/renders/keyboard@2x.webp";
import mouse1x from "../../assets/design/renders/mouse@1x.webp";
import mouse2x from "../../assets/design/renders/mouse@2x.webp";
import receiver1x from "../../assets/design/renders/receiver@1x.webp";
import receiver2x from "../../assets/design/renders/receiver@2x.webp";

/**
 * Registry of device illustrations. Hero art is a raster render (WebP with alpha, 512x384 and 1024x768,
 * produced by scripts/import-design-assets.ts) shown through `<img srcset>`. The top view used for the
 * button map is our own static SVG from src/assets/design (imported as text, injected inline by
 * DeviceIllustration so the --illu-* variables apply); it carries the ids `slot-g1`..`slot-g11`, which
 * DeviceIllustration targets for highlighting, and the marker anchors are listed per view.
 */

export type IllustrationView = "top" | "hero";

export interface RasterArt {
  /** 512x384 file. */
  src1x: string;
  /** 1024x768 file. */
  src2x: string;
}

export interface IllustrationDef {
  /** SVG markup, trusted: a local build-time asset. Set for vector views. */
  svg?: string;
  /** Raster render. Set for hero views. */
  raster?: RasterArt;
  viewBox: { readonly width: number; readonly height: number };
  /** Marker anchors per slot index, in viewBox units. Empty when the view has no button map. */
  slots: Record<number, Point>;
}

const NO_SLOTS: Record<number, Point> = {};

/** Intrinsic size of the rendered frame (the @1x file is 512x384, the @2x file 1024x768). */
const RENDER_VIEWBOX = { width: 512, height: 384 } as const;

const renderDef = (src1x: string, src2x: string): IllustrationDef => ({
  raster: { src1x, src2x },
  viewBox: RENDER_VIEWBOX,
  slots: NO_SLOTS,
});

const generic = {
  mouse: renderDef(mouse1x, mouse2x),
  keyboard: renderDef(keyboard1x, keyboard2x),
  headset: renderDef(headset1x, headset2x),
  gamepad: renderDef(gamepad1x, gamepad2x),
} satisfies Record<DeviceKind, IllustrationDef>;

const registry: Record<string, Partial<Record<IllustrationView, IllustrationDef>>> = {
  g502x: {
    top: { svg: mouseTop, viewBox: G502X_VIEWBOX, slots: G502X_SLOTS },
    hero: generic.mouse,
  },
  receiver: { hero: renderDef(receiver1x, receiver2x) },
  // Not tied to a device yet.
  earbuds: { hero: renderDef(earbuds1x, earbuds2x) },
};

/**
 * The artwork for a UI illustration key. With `kind`, a hero view of an unknown key falls back to the
 * generic art of that device kind; the top (button map) view never falls back.
 */
export function resolveIllustration(
  key: string | null | undefined,
  view: IllustrationView,
  kind?: DeviceKind,
): IllustrationDef | null {
  const exact = key ? registry[key]?.[view] : undefined;
  if (exact) return exact;
  return view === "hero" && kind ? generic[kind] : null;
}
