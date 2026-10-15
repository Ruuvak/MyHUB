// Copies the runtime SVGs from assets/design/ (untouched source of truth) to src/assets/design/.
// Strips the C2PA <metadata> block and its namespace, collapses inter-tag whitespace and trims.
// Also turns the hero renders in assets/design/renders/ (2048x1536 RGBA PNG, untouched) into WebP with
// alpha at 1024x768 (@2x) and 512x384 (@1x) in src/assets/design/renders/. Full frames are kept, no
// trimming, so every device keeps the scale and centring the artist gave it.
// Idempotent: the output depends only on the input. Run with `bun run assets:import`.
import { mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import sharp from "sharp";

const root = resolve(import.meta.dir, "..");
const from = join(root, "assets", "design");
const to = join(root, "src", "assets", "design");

const FILES = ["mouse-top", "badge-read-only", "logo-symbol", "logo-lockup"];

/** Source PNG (in assets/design/renders) to runtime name. */
const RENDERS: Record<string, string> = {
  "01-gaming-mouse": "mouse",
  "02-tkl-keyboard": "keyboard",
  "03-gaming-headset": "headset",
  "04-wireless-gamepad": "gamepad",
  "05-usb-receiver": "receiver",
  "06-earbuds-case": "earbuds",
};
const WEBP_QUALITY = 82;
const SIZES = [
  { suffix: "@1x", width: 512, height: 384 },
  { suffix: "@2x", width: 1024, height: 768 },
];

export function clean(svg: string): string {
  return (
    svg
      .replace(/<metadata>[\s\S]*?<\/metadata>/g, "")
      .replace(/\s+xmlns:c2pa="[^"]*"/g, "")
      .replace(/\r\n/g, "\n")
      .replace(/>\s+</g, "><")
      .trim() + "\n"
  );
}

mkdirSync(to, { recursive: true });
for (const name of FILES) {
  const out = clean(readFileSync(join(from, `${name}.svg`), "utf8"));
  if (/c2pa|<script|<image|href=/i.test(out)) throw new Error(`${name}.svg still has forbidden content`);
  writeFileSync(join(to, `${name}.svg`), out);
  console.log(`${name}.svg  ${out.length} bytes`);
}

const rendersTo = join(to, "renders");
mkdirSync(rendersTo, { recursive: true });
for (const [source, name] of Object.entries(RENDERS)) {
  for (const { suffix, width, height } of SIZES) {
    const file = join(rendersTo, `${name}${suffix}.webp`);
    await sharp(join(from, "renders", `${source}.png`))
      .resize(width, height, { fit: "fill", kernel: "lanczos3" })
      .webp({ quality: WEBP_QUALITY, alphaQuality: 90 })
      .toFile(file);
    console.log(`renders/${name}${suffix}.webp  ${statSync(file).size} bytes`);
  }
}
