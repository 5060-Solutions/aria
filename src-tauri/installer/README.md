# Installer Assets

This directory contains branding assets for the Aria installers.

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
