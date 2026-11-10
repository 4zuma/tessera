#!/usr/bin/env node
// Records the animation at the top of the README: the browser preview, with its sample data,
// scans C:, zooms into Program Files (x86) and goes back up.
//
//   node scripts/readme-media.mjs            all four: English and Chinese, light and dark
//   node scripts/readme-media.mjs zh dark    only the ones that match every word given
//
// Writes, for each language (en, zh) and theme (light, dark):
//   docs/readme/demo-<lang>-<theme>.webp    the animation, an animated WebP that loops
//   docs/readme/still-<lang>-<theme>.webp   the finished map as a still, shown instead of the
//                                           animation to visitors who ask for reduced motion
//
// Needs the app's npm packages (cd app && npm install) and Playwright, which is not a dependency
// of the repo:
//   npm i -g playwright && npx playwright install chromium
// On Windows it falls back to Microsoft Edge when Playwright's Chromium is not installed, and
// `--channel msedge` asks for Edge directly. Record on Windows, so the text is set in Segoe UI
// as in the app.
//
// The page runs on Playwright's fake clock, which this script moves on one frame at a time, so
// every frame is exact however fast the machine is. The frames are encoded with the browser's
// own WebP encoder, and each one stores only the rectangle that changed since the one before.

import { execSync } from 'node:child_process';
import { createReadStream, existsSync, mkdirSync, statSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { dirname, extname, join, normalize, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const appDir = join(root, 'app');
const dist = join(appDir, 'dist');
const out = join(root, 'docs', 'readme');

// The page is 1240 x 780 CSS px, and the preview draws its window 20 px in from each edge on a
// transparent desk. At 1.5x the files are 1860 px wide: sharp on high-DPI screens, where GitHub
// shows the README about 900 px wide.
const VIEW = { width: 1240, height: 780 };
const SCALE = 1.25;
// 30 frames a second, and 60 while the camera zooms: Fluent's zoom curve covers most of its way
// in the first 150 ms, which at 30 fps would be four frames.
const FPS = 30;
const ZOOM_FPS = 30;
const QUALITY = 0.75; // WebP quality of the animation's frames
const STILL_QUALITY = 0.9;
// Double-clicked to zoom in: the header of C:\Program Files (x86) in the sample data at this size.
const ZOOM_AT = { x: 530, y: 126 };
const ZOOM_FOLDER = 'Program Files (x86)';

const LANGS = { en: 'en-US', zh: 'zh-CN' };
const THEMES = ['light', 'dark'];

// Preview-only parts of the title bar (the "sample data" chip and its buttons) are hidden, and the
// desk behind the window is made transparent, so the window's own shadow shows on GitHub's page.
const CAPTURE_CSS = `
  html, body { background: transparent !important; }
  .tb .chip, .tb .sbtn { display: none !important; }
`;

// Playwright is not a dependency of the repo, so look for it next to the working folder, next to
// the repo, and in npm's global folder (where `npm i -g` puts it; Node does not search there).
function loadPlaywright() {
  const dirs = [process.cwd(), root];
  try {
    dirs.push(execSync('npm root -g', { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim());
  } catch {
    // no npm on PATH: the local folders are all we can try
  }
  for (const dir of dirs) {
    try {
      return createRequire(join(dir, 'noop.js'))('playwright');
    } catch {
      // not here, try the next folder
    }
  }
  console.error('Playwright is needed: npm i -g playwright && npx playwright install chromium');
  process.exit(1);
}

async function launch(chromium, channel) {
  if (channel) return chromium.launch({ channel });
  try {
    return await chromium.launch();
  } catch (e) {
    if (process.platform !== 'win32') throw e;
    console.log("Playwright's Chromium is not installed, using Microsoft Edge");
    return chromium.launch({ channel: 'msedge' });
  }
}

// Serves the built app on a free port.
function serve() {
  const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.png': 'image/png', '.svg': 'image/svg+xml', '.ico': 'image/x-icon' };
  const server = createServer((req, res) => {
    let file = normalize(join(dist, decodeURIComponent(new URL(req.url, 'http://x').pathname)));
    if (!file.startsWith(dist + sep) || !existsSync(file) || statSync(file).isDirectory()) file = join(dist, 'index.html');
    res.writeHead(200, { 'content-type': types[extname(file)] ?? 'application/octet-stream' });
    createReadStream(file).pipe(res);
  });
  return new Promise(resolve => server.listen(0, '127.0.0.1', () => resolve(server)));
}

// ---- animated WebP ----
// https://developers.google.com/speed/webp/docs/riff_container

const u24 = (b, at, v) => b.writeUIntLE(v, at, 3);

function chunk(tag, data) {
  const head = Buffer.alloc(8);
  head.write(tag, 0, 'ascii');
  head.writeUInt32LE(data.length, 4);
  return Buffer.concat(data.length % 2 ? [head, data, Buffer.alloc(1)] : [head, data]);
}

// The image data of a still WebP (its ALPH and VP8 or VP8L chunks), to go inside an ANMF frame.
function imageChunks(webp) {
  if (webp.toString('ascii', 0, 4) !== 'RIFF' || webp.toString('ascii', 8, 12) !== 'WEBP') throw new Error('the browser did not return a WebP');
  const keep = [];
  for (let at = 12; at + 8 <= webp.length; ) {
    const tag = webp.toString('ascii', at, at + 4);
    const size = webp.readUInt32LE(at + 4);
    if (tag === 'ALPH' || tag === 'VP8 ' || tag === 'VP8L') keep.push(webp.subarray(at, at + 8 + size + (size % 2)));
    at += 8 + size + (size % 2);
  }
  return Buffer.concat(keep);
}

function animatedWebp(width, height, frames) {
  const vp8x = Buffer.alloc(10);
  vp8x[0] = 0x12; // animation and alpha
  u24(vp8x, 4, width - 1);
  u24(vp8x, 7, height - 1);
  const anim = Buffer.alloc(6); // transparent background, loop forever
  const parts = [Buffer.from('WEBP'), chunk('VP8X', vp8x), chunk('ANIM', anim)];
  for (const f of frames) {
    const head = Buffer.alloc(16);
    u24(head, 0, f.x / 2);
    u24(head, 3, f.y / 2);
    u24(head, 6, f.w - 1);
    u24(head, 9, f.h - 1);
    u24(head, 12, f.ms);
    head[15] = 0x02; // replace the rectangle (no blending), keep it for the next frame
    parts.push(chunk('ANMF', Buffer.concat([head, f.data])));
  }
  const body = Buffer.concat(parts);
  const riff = Buffer.alloc(8);
  riff.write('RIFF', 0, 'ascii');
  riff.writeUInt32LE(body.length, 4);
  return Buffer.concat([riff, body]);
}

// Runs in a blank page: decodes each PNG screenshot, finds what changed since the previous one,
// and encodes that rectangle as WebP. Offsets in an animated WebP must be even.
function encoderScript() {
  let prev = null;
  const base64 = bytes => {
    let s = '';
    for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    return btoa(s);
  };
  const decode = async png => {
    const bin = Uint8Array.from(atob(png), c => c.charCodeAt(0));
    const bmp = await createImageBitmap(new Blob([bin], { type: 'image/png' }));
    const c = new OffscreenCanvas(bmp.width, bmp.height);
    const ctx = c.getContext('2d', { willReadFrequently: true });
    ctx.drawImage(bmp, 0, 0);
    return ctx.getImageData(0, 0, bmp.width, bmp.height);
  };
  const encode = async (img, x, y, w, h, quality) => {
    const c = new OffscreenCanvas(w, h);
    c.getContext('2d').putImageData(img, -x, -y, x, y, w, h);
    const blob = await c.convertToBlob({ type: 'image/webp', quality });
    return base64(new Uint8Array(await blob.arrayBuffer()));
  };
  window.resetClip = () => {
    prev = null;
  };
  window.addFrame = async (png, quality) => {
    const img = await decode(png);
    const { width: W, height: H } = img;
    const px = new Uint32Array(img.data.buffer);
    let x0 = 0, y0 = 0, x1 = W - 1, y1 = H - 1;
    if (prev) {
      x0 = W; y0 = H; x1 = -1; y1 = -1;
      for (let y = 0; y < H; y++) {
        const row = y * W;
        for (let x = 0; x < W; x++) {
          if (px[row + x] !== prev[row + x]) {
            if (x < x0) x0 = x;
            if (x > x1) x1 = x;
            if (y < y0) y0 = y;
            y1 = y;
          }
        }
      }
    }
    prev = px;
    if (x1 < 0) return { W, H, same: true };
    x0 &= ~1;
    y0 &= ~1;
    const w = x1 - x0 + 1, h = y1 - y0 + 1;
    return { W, H, x: x0, y: y0, w, h, webp: await encode(img, x0, y0, w, h, quality) };
  };
  window.still = async (png, quality) => {
    const img = await decode(png);
    return encode(img, 0, 0, img.width, img.height, quality);
  };
}

class Clip {
  constructor(encoder) {
    this.encoder = encoder;
    this.frames = [];
    this.t = 0; // ms, kept exact so rounding never adds up
  }
  async start() {
    await this.encoder.evaluate(() => window.resetClip());
  }
  #span(ms) {
    const d = Math.round(this.t + ms) - Math.round(this.t);
    this.t += ms;
    return d;
  }
  async add(png, ms) {
    const r = await this.encoder.evaluate(([p, q]) => window.addFrame(p, q), [png.toString('base64'), QUALITY]);
    this.width = r.W;
    this.height = r.H;
    if (r.same && this.frames.length) return void (this.frames.at(-1).ms += this.#span(ms));
    this.frames.push({ x: r.x, y: r.y, w: r.w, h: r.h, ms: this.#span(ms), data: imageChunks(Buffer.from(r.webp, 'base64')) });
  }
  hold(ms) {
    this.frames.at(-1).ms += this.#span(ms);
  }
  webp() {
    return animatedWebp(this.width, this.height, this.frames);
  }
}

async function record(browser, encoder, lang, theme) {
  const page = await browser.newPage({ viewport: VIEW, deviceScaleFactor: SCALE, colorScheme: theme, locale: LANGS[lang], reducedMotion: 'no-preference' });
  // install() lets the fake time flow with real time; paused, it moves only when runFor() says so
  const t0 = new Date('2026-11-01T10:00:00').getTime();
  await page.clock.install({ time: t0 });
  await page.clock.pauseAt(t0 + 1);
  const server = await serve();
  try {
    await page.goto(`http://127.0.0.1:${server.address().port}/`);
    await page.addStyleTag({ content: CAPTURE_CSS });
    await page.evaluate(() => document.fonts.ready);

    const clip = new Clip(encoder);
    await clip.start();
    const shot = () => page.screenshot({ omitBackground: true, type: 'png' });
    // let `ms` of page time pass, `speed` times faster than real time, at `fps` frames a second;
    // with `until`, stop as soon as it says so
    const play = async (ms, { speed = 1, fps = FPS, until } = {}) => {
      const frame = 1000 / fps;
      for (let i = Math.round(ms / speed / frame); i > 0; i--) {
        await page.clock.runFor(frame * speed);
        await clip.add(await shot(), frame);
        if (until && (await until())) return;
      }
      if (until) throw new Error(`still waiting after ${ms} ms of page time`);
    };
    // the scan has started (its progress bar showed) and ended (the bar went again)
    let started = false;
    const scanDone = async () => {
      const active = (await page.locator('[role="progressbar"].active').count()) > 0;
      started ||= active;
      return started && !active;
    };

    // the drive picker; the preview starts a scan of C: by itself 300 ms after loading
    await page.clock.runFor(50);
    await clip.add(await shot(), 900);
    // the sample scan (4.6 s), 2.5 times faster at 20 fps, until its progress bar goes; then
    // time for the last layout to settle
    await play(15000, { speed: 2.5, fps: 20, until: scanDone });
    await play(800, { speed: 2.5, fps: 20 });
    const still = Buffer.from(await encoder.evaluate(([p, q]) => window.still(p, q), [(await shot()).toString('base64'), STILL_QUALITY]), 'base64');
    clip.hold(1200);
    // zoom into Program Files (x86), then move the pointer off the map so nothing stays hovered
    await page.mouse.dblclick(ZOOM_AT.x, ZOOM_AT.y);
    await page.mouse.move(2, 2);
    await play(700, { fps: ZOOM_FPS });
    const crumbs = await page.locator('nav.crumbs').innerText();
    if (!crumbs.includes(ZOOM_FOLDER)) throw new Error(`the double-click at ${ZOOM_AT.x},${ZOOM_AT.y} opened "${crumbs.replace(/\s+/g, ' ')}", not ${ZOOM_FOLDER}: update ZOOM_AT`);
    clip.hold(1800);
    // back up to C:; drop the keyboard focus so the map gets no focus ring
    await page.keyboard.press('Backspace');
    await page.evaluate(() => document.activeElement?.blur());
    await play(700, { fps: ZOOM_FPS });
    clip.hold(900);

    const webp = clip.webp();
    writeFileSync(join(out, `demo-${lang}-${theme}.webp`), webp);
    writeFileSync(join(out, `still-${lang}-${theme}.webp`), still);
    const kb = n => `${Math.round(n / 1024)} KB`;
    console.log(`docs/readme/demo-${lang}-${theme}.webp: ${clip.frames.length} frames, ${(clip.t / 1000).toFixed(1)} s, ${kb(webp.length)}; still ${kb(still.length)}`);
  } finally {
    server.close();
    await page.close();
  }
}

async function main() {
  const args = process.argv.slice(2);
  const ci = args.indexOf('--channel');
  const channel = ci >= 0 ? args.splice(ci, 2)[1] : undefined;
  const jobs = Object.keys(LANGS).flatMap(lang => THEMES.map(theme => [lang, theme])).filter(job => args.every(a => job.includes(a)));
  if (!jobs.length) {
    console.error(`Nothing matches "${args.join(' ')}": use en, zh, light or dark`);
    process.exit(1);
  }
  if (!existsSync(join(appDir, 'node_modules'))) {
    console.error('Install the app first: cd app && npm install');
    process.exit(1);
  }
  execSync('npm run build', { cwd: appDir, stdio: 'inherit' });

  const { chromium } = loadPlaywright();
  const browser = await launch(chromium, channel);
  try {
    const encoder = await browser.newPage();
    await encoder.evaluate(encoderScript);
    mkdirSync(out, { recursive: true });
    for (const [lang, theme] of jobs) await record(browser, encoder, lang, theme);
  } finally {
    await browser.close();
  }
}

main().catch(e => {
  console.error(e);
  process.exit(1);
});
