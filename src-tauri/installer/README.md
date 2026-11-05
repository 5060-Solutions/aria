# Installer Assets

This directory contains branding assets for the Aria installers.

## macOS (DMG)

`dmg/` holds the background of the window that opens when the DMG is mounted:
Aria on the left, an arrow, and the Applications folder on the right.

- `background.svg` - the source, 660x440 (a 660x400 view plus slack), with the layout notes
- `background.png` / `background@2x.png` - 1x and 2x renders
- `background.tiff` - both renders in one HiDPI TIFF; this is the file
  `bundle.macOS.dmg.background` in `tauri.conf.json` points at, so Retina
  displays get the 2x image

Rebuild the renders after editing the SVG with
`node scripts/generate-dmg-background.mjs` (the TIFF needs macOS). The window
size and icon positions in `tauri.conf.json` must match the SVG's layout.

Tauri applies the background and positions with an AppleScript that drives
Finder. It skips that script when `CI=true`; the release workflow sets
`TAURI_BUNDLER_DMG_IGNORE_CI=true` to run it anyway, and falls back to a plain
DMG if Finder cannot be scripted on the runner. Building locally, the terminal
needs permission to control Finder (System Settings > Privacy & Security >
Automation).

## Windows (WiX)

- `banner.bmp` - 493x58 pixels, displayed at the top of the installer wizard
- `dialog.bmp` - 493x312 pixels, displayed on the first and last installer pages

## Windows (NSIS)

- `header.bmp` - 150x57 pixels, header image for installer pages
- `sidebar.bmp` - 164x314 pixels, sidebar image for welcome/finish pages

## Creating Assets

For best results, use the Aria logo with the purple gradient background.
Export at the exact dimensions specified above.

### Recommended Design

- Use the Aria waveform logo centered
- Background: Linear gradient from #6366f1 to #a855f7
- Text: "Aria" in white, Inter/SF Pro font
- Tagline (optional): "Your voice, beautifully connected."

### Tools

- Figma, Sketch, or Adobe Illustrator for design
- ImageMagick for batch conversion: `convert input.png -resize 493x58! banner.bmp`
