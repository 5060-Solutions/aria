import { Resvg } from '@resvg/resvg-js';
import { readFileSync, writeFileSync, mkdirSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const rootDir = join(__dirname, '..');
const iconsDir = join(rootDir, 'src-tauri', 'icons');

// Read the SVG
const svg = readFileSync(join(rootDir, 'public', 'icon.svg'), 'utf8');

// Icon sizes needed for Tauri
const sizes = [
  { name: '32x32.png', size: 32 },
  { name: '128x128.png', size: 128 },
  { name: '128x128@2x.png', size: 256 },
  { name: 'icon.png', size: 512 },
  // Windows Store logos
  { name: 'Square30x30Logo.png', size: 30 },
  { name: 'Square44x44Logo.png', size: 44 },
  { name: 'Square71x71Logo.png', size: 71 },
  { name: 'Square89x89Logo.png', size: 89 },
  { name: 'Square107x107Logo.png', size: 107 },
  { name: 'Square142x142Logo.png', size: 142 },
  { name: 'Square150x150Logo.png', size: 150 },
  { name: 'Square284x284Logo.png', size: 284 },
  { name: 'Square310x310Logo.png', size: 310 },
  { name: 'StoreLogo.png', size: 50 },
];

console.log('Generating icons from SVG...\n');

for (const { name, size } of sizes) {
  const resvg = new Resvg(svg, {
    fitTo: {
      mode: 'width',
      value: size,
    },
  });
  
  const pngData = resvg.render();
  const pngBuffer = pngData.asPng();
  
  const outPath = join(iconsDir, name);
  writeFileSync(outPath, pngBuffer);
  console.log(`  ✓ ${name} (${size}x${size})`);
}

// Also copy to public for web
writeFileSync(join(rootDir, 'public', 'icon.png'), readFileSync(join(iconsDir, 'icon.png')));
console.log('\n  ✓ public/icon.png');

console.log('\n✅ Icons generated successfully!');
console.log('\nNote: For .icns and .ico files, you may need to use additional tools:');
console.log('  - macOS: iconutil or makeicns');
console.log('  - Windows: Use online converter or png2ico');
console.log('\nOr run: pnpm tauri icon public/icon.svg');
