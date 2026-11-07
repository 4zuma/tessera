#!/usr/bin/env node
// Draws the Tessera logo at every size and writes the app icons.
//
//   node scripts/brand-icons.mjs
//
// Writes:
//   docs/brand/tessera.svg          master artwork (48 grid), used from 48 px up and in the README
//   docs/brand/tessera-<N>.svg      hand-snapped artwork for each small size (16 to 40 px)
//   docs/brand/sizes.png            every size on light, dark and accent backgrounds, for review
//   app/src-tauri/icons/*           icon.ico (16 to 256 px), 32x32.png, 128x128.png, 128x128@2x.png, icon.png
//   app/public/favicon.png          32 px, for the browser preview
//
// Rendering needs Playwright with Chromium (not a dependency of the repo):
//   npm i -g playwright && npx playwright install chromium
//
// The logo: a disc (a disk platter seen from above) holding a three-tile treemap, with a hub at
// the centre. Sizes up to 40 px are drawn on their own pixel grid so every gap and edge lands on a
// whole pixel; they also get a solid dark outer ring and a white rim, which keep the outline
// visible on light and dark themes and on any accent-coloured title bar. See docs/brand/README.md.

import { mkdirSync, writeFileSync } from 'node:fs';
import { deflateSync } from 'node:zlib';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const brand = join(root, 'docs', 'brand');
const icons = join(root, 'app', 'src-tauri', 'icons');

// ---- palette ----
const INK = '#14213A'; // outer ring and hub dot
const RIM = '#F4F7FB'; // rim, gaps and hub
const GROUT = ['#FAFCFF', '#E1E8F1']; // rim gradient (large sizes)
const TILE = {
  // [gradient top-left, gradient bottom-right, flat colour for 16 and 20 px]
  big: ['#3C93F2', '#1A66D6', '#2A7FE6'], // left, the largest tile
  top: ['#9AD6FF', '#4FA8F2', '#6CBBF7'], // right, upper
  low: ['#353C9E', '#1B1F62', '#1B1F5E'], // right, lower
};
// Cuts in the 48 grid: the vertical gap is centred on x = 31, the right-hand gap on y = 19, so
// the corner where they meet stays clear of the hub at the centre (24, 24).
const CUT_X = 31;
const CUT_Y = 19;

const fmt = n => String(Math.round(n * 1000) / 1000);
const svg = (n, defs, body) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${n}" height="${n}" viewBox="0 0 ${n} ${n}">` +
  (defs ? `<defs>${defs}</defs>` : '') + body + '</svg>\n';
// gradient at 120 degrees (light from the top left), in user space so it spans one tile
const grad = (id, [a, b], x, y, w, h) =>
  `<linearGradient id="${id}" gradientUnits="userSpaceOnUse" x1="${fmt(x + w * .25)}" y1="${fmt(y)}" x2="${fmt(x + w * .75)}" y2="${fmt(y + h)}">` +
  `<stop offset="0" stop-color="${a}"/><stop offset="1" stop-color="${b}"/></linearGradient>`;
const rect = (x, y, w, h, fill) => `<rect x="${fmt(x)}" y="${fmt(y)}" width="${fmt(w)}" height="${fmt(h)}" fill="${fill}"/>`;

/** Artwork for small sizes (up to 40 px), on an N-pixel grid. */
function small(n) {
  const c = n / 2;
  const rim = Math.max(1, Math.round(n / 16)); // white rim inside the 1 px dark ring
  const r = c - 1 - rim; // radius of the tile disc
  const gap = n >= 36 ? 2 : 1;
  const col = Math.round((CUT_X * n) / 48 - gap / 2); // left edge of the vertical gap
  const row = Math.round((CUT_Y * n) / 48 - gap / 2); // top edge of the right-hand gap
  const tiles = [
    ['big', 0, 0, col, n],
    ['top', col + gap, 0, n - col - gap, row],
    ['low', col + gap, row + gap, n - col - gap, n - row - gap],
  ];
  const flat = n <= 20;
  let defs = `<clipPath id="d"><circle cx="${c}" cy="${c}" r="${r}"/></clipPath>`;
  if (!flat) {
    // the gradients run across the tile disc, not the whole tile rectangle
    for (const [k, x, y, w, h] of tiles) {
      const x0 = Math.max(x, c - r), y0 = Math.max(y, c - r);
      defs += grad(k, TILE[k], x0, y0, Math.min(x + w, c + r) - x0, Math.min(y + h, c + r) - y0);
    }
  }
  let body = `<circle cx="${c}" cy="${c}" r="${c - .5}" fill="${RIM}" stroke="${INK}"/>`;
  body += `<g clip-path="url(#d)">${tiles.map(([k, x, y, w, h]) => rect(x, y, w, h, flat ? TILE[k][2] : `url(#${k})`)).join('')}</g>`;
  if (flat) body += `<circle cx="${c}" cy="${c}" r="${fmt((1.3 * n) / 16)}" fill="${RIM}"/>`;
  else body += `<circle cx="${c}" cy="${c}" r="${fmt((4 * n) / 48)}" fill="${RIM}"/><circle cx="${c}" cy="${c}" r="${fmt(Math.max(.8, (1.5 * n) / 48))}" fill="${INK}"/>`;
  return svg(n, defs, body);
}

/** Master artwork on the 48 grid, for 48 px and up. */
function master() {
  const g = 1; // half the gap
  const tiles = [
    ['big', 4, 4, CUT_X - g - 4, 40],
    ['top', CUT_X + g, 4, 44 - CUT_X - g, CUT_Y - g - 4],
    ['low', CUT_X + g, CUT_Y + g, 44 - CUT_X - g, 44 - CUT_Y - g],
  ];
  const r = 18.5;
  let defs = `<clipPath id="d"><circle cx="24" cy="24" r="${r}"/></clipPath>`;
  defs += `<linearGradient id="rim" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${GROUT[0]}"/><stop offset="1" stop-color="${GROUT[1]}"/></linearGradient>`;
  for (const [k, x, y, w, h] of tiles) {
    const x0 = Math.max(x, 24 - r), y0 = Math.max(y, 24 - r);
    defs += grad(k, TILE[k], x0, y0, Math.min(x + w, 24 + r) - x0, Math.min(y + h, 24 + r) - y0);
  }
  let body = `<circle cx="24" cy="24" r="21.5" fill="url(#rim)" stroke="${INK}" stroke-opacity=".45"/>`;
  body += `<g clip-path="url(#d)">${tiles.map(([k, x, y, w, h]) => rect(x, y, w, h, `url(#${k})`)).join('')}</g>`;
  body += `<circle cx="24" cy="24" r="4" fill="url(#rim)"/><circle cx="24" cy="24" r="1.5" fill="${INK}"/>`;
  return svg(48, defs, body);
}

const SMALL = [16, 20, 24, 30, 32, 36, 40];
const ICO = [16, 20, 24, 30, 32, 36, 40, 48, 64, 96, 128, 256];
const art = n => (n <= 40 ? small(n) : master());

/** Encodes RGBA pixels as a PNG, choosing a filter per row; smaller than the canvas's own PNG. */
function encodePng(n, rgba) {
  const crcTable = Array.from({ length: 256 }, (_, k) => {
    let c = k;
    for (let i = 0; i < 8; i++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = buf => {
    let c = 0xffffffff;
    for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type, data) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type, 'ascii'), data]);
    const c = Buffer.alloc(4);
    c.writeUInt32BE(crc(td));
    return Buffer.concat([len, td, c]);
  };
  const stride = n * 4;
  const raw = Buffer.alloc((stride + 1) * n);
  const paeth = (a, b, c) => {
    const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
    return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
  };
  for (let y = 0; y < n; y++) {
    let best = null, bestSum = Infinity, bestF = 0;
    for (let f = 0; f < 5; f++) {
      const out = Buffer.alloc(stride);
      let sum = 0;
      for (let x = 0; x < stride; x++) {
        const v = rgba[y * stride + x];
        const a = x >= 4 ? rgba[y * stride + x - 4] : 0;
        const b = y > 0 ? rgba[(y - 1) * stride + x] : 0;
        const c = x >= 4 && y > 0 ? rgba[(y - 1) * stride + x - 4] : 0;
        const pred = f === 0 ? 0 : f === 1 ? a : f === 2 ? b : f === 3 ? (a + b) >> 1 : paeth(a, b, c);
        const d = (v - pred) & 0xff;
        out[x] = d;
        sum += d < 128 ? d : 256 - d;
      }
      if (sum < bestSum) { bestSum = sum; best = out; bestF = f; }
    }
    raw[y * (stride + 1)] = bestF;
    best.copy(raw, y * (stride + 1) + 1);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(n, 0);
  ihdr.writeUInt32BE(n, 4);
  ihdr.set([8, 6, 0, 0, 0], 8); // 8-bit RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9, memLevel: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

/** Packs PNG images into an .ico/** Packs PNG images into an .ico (PNG entries, supported since Windows Vista). */
function ico(images) {
  const head = Buffer.alloc(6 + 16 * images.length);
  head.writeUInt16LE(0, 0);
  head.writeUInt16LE(1, 2);
  head.writeUInt16LE(images.length, 4);
  let offset = head.length;
  images.forEach(({ size, png }, i) => {
    const e = 6 + 16 * i;
    head.writeUInt8(size >= 256 ? 0 : size, e);
    head.writeUInt8(size >= 256 ? 0 : size, e + 1);
    head.writeUInt8(0, e + 2);
    head.writeUInt8(0, e + 3);
    head.writeUInt16LE(1, e + 4);
    head.writeUInt16LE(32, e + 6);
    head.writeUInt32LE(png.length, e + 8);
    head.writeUInt32LE(offset, e + 12);
    offset += png.length;
  });
  return Buffer.concat([head, ...images.map(i => i.png)]);
}

async function main() {
  const require = createRequire(join(process.cwd(), 'noop.js'));
  let chromium;
  try {
    ({ chromium } = require('playwright'));
  } catch {
    console.error('Playwright is needed: npm i -g playwright && npx playwright install chromium');
    process.exit(1);
  }
  mkdirSync(brand, { recursive: true });
  writeFileSync(join(brand, 'tessera.svg'), master());
  for (const n of SMALL) writeFileSync(join(brand, `tessera-${n}.svg`), small(n));

  const browser = await chromium.launch();
  const page = await browser.newPage({ deviceScaleFactor: 1 });
  // draw each SVG onto a canvas of exactly N x N pixels
  const render = (svgText, n) =>
    page.evaluate(async ([s, n]) => {
      const img = new Image();
      img.src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(s);
      await img.decode();
      const c = document.createElement('canvas');
      c.width = c.height = n;
      const x = c.getContext('2d');
      x.drawImage(img, 0, 0, n, n);
      // canvas pixels are premultiplied internally; getImageData returns them straight, as PNG wants
      return Array.from(x.getImageData(0, 0, n, n).data);
    }, [svgText, n]).then(px => encodePng(n, Uint8Array.from(px)));

  const png = {};
  for (const n of [...new Set([...ICO, 512])]) png[n] = await render(art(n), n);
  writeFileSync(join(icons, 'icon.ico'), ico(ICO.map(size => ({ size, png: png[size] }))));
  writeFileSync(join(icons, '32x32.png'), png[32]);
  writeFileSync(join(icons, '128x128.png'), png[128]);
  writeFileSync(join(icons, '128x128@2x.png'), png[256]);
  writeFileSync(join(icons, 'icon.png'), png[512]);
  writeFileSync(join(root, 'app', 'public', 'favicon.png'), png[32]);

  // review sheet: every size at its real pixels, magnified 3x, on the backgrounds that matter
  const bgs = [['Light', '#F3F3F3'], ['Dark', '#202020'], ['Accent: blue', '#0078D7'], ['Accent: navy', '#0063B1'], ['Accent: gold', '#FFB900'], ['Accent: red', '#E81123'], ['Accent: graphite', '#4C4A48']];
  const shown = [16, 20, 24, 32, 40, 48, 64];
  const cells = bgs.map(([name, bg]) =>
    `<tr><th>${name}</th>${shown.map(n => `<td style="background:${bg}"><img src="data:image/png;base64,${png[n].toString('base64')}" width="${n * 3}" height="${n * 3}"></td>`).join('')}</tr>`).join('');
  await page.setViewportSize({ width: 1400, height: 800 });
  await page.setContent(`<style>body{margin:16px;font:13px "Segoe UI",system-ui,sans-serif;color:#222;background:#fff}table{border-collapse:collapse}th{text-align:left;padding-right:12px;font-weight:600}td{padding:10px;text-align:center}img{image-rendering:pixelated;display:block}</style>
    <table><tr><th></th>${shown.map(n => `<th style="text-align:center">${n} px</th>`).join('')}</tr>${cells}</table>`);
  await page.locator('table').screenshot({ path: join(brand, 'sizes.png') });
  await browser.close();
  console.log('wrote docs/brand, app/src-tauri/icons and app/public/favicon.png');
}

main();
