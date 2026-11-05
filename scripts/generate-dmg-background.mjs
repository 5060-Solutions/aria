// Renders the macOS installer (DMG) background from its SVG source.
//
//   node scripts/generate-dmg-background.mjs
//
// Writes, next to src-tauri/installer/dmg/background.svg:
//   background.png     660x440, for reference and non-Retina previews
//   background@2x.png  1320x880
//   background.tiff    both of the above in one file (macOS only, via
//                      tiffutil), which is what tauri.conf.json points at:
//                      Finder picks the 2x page on a Retina display, where a
//                      plain 1x PNG would be drawn blurry.
//
// The text is set in Figtree when it is installed and Helvetica Neue
// otherwise; the app bundles Figtree only as woff2, which resvg cannot read.
import { Resvg } from "@resvg/resvg-js";
import { execFileSync } from "child_process";
import { readFileSync, writeFileSync } from "fs";
import { dirname, join } from "path";
import { fileURLToPath } from "url";

const dir = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "installer", "dmg");
const svg = readFileSync(join(dir, "background.svg"), "utf8");

const WIDTH = 660;

for (const [name, scale] of [
  ["background.png", 1],
  ["background@2x.png", 2],
]) {
  const png = new Resvg(svg, {
    fitTo: { mode: "width", value: WIDTH * scale },
    font: { loadSystemFonts: true, defaultFontFamily: "Helvetica Neue" },
  })
    .render()
    .asPng();
  writeFileSync(join(dir, name), png);
  console.log(`  ✓ ${name}`);
}

if (process.platform === "darwin") {
  execFileSync(
    "tiffutil",
    [
      "-cathidpicheck",
      join(dir, "background.png"),
      join(dir, "background@2x.png"),
      "-out",
      join(dir, "background.tiff"),
    ],
    { stdio: "inherit" }
  );
  console.log("  ✓ background.tiff");
} else {
  console.log("  - background.tiff not rebuilt: tiffutil is macOS only");
}
