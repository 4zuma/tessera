/**
 * Drives the real app (src/lib/app.svelte.ts) inside a design-system preview or a canvas
 * artboard: one boot per page, the mock backend, and named states ("scenarios") reached
 * through the app's own methods, so a preview shows what the app really draws.
 */
import { app } from '../../src/lib/app.svelte';
import { Flag, Kind } from '../../src/lib/backend/codec';
import { setLang } from '../../src/lib/i18n.svelte';
import { fluentLayout } from '../../src/lib/treemap/motion';
import { bandLayout, paintStyle } from '../../src/lib/treemap/paint';
import type { Rect } from '../../src/lib/backend/codec';
import { config } from './config';

export const SCENARIOS = ['map', 'picker', 'scanning', 'selected', 'folder', 'menu', 'confirm', 'search', 'toast', 'tooltip'] as const;
export type Scenario = (typeof SCENARIOS)[number];

/** what any component of the bundle may set; the last value set wins for the whole page */
export interface PageProps {
  scenario?: Scenario;
  scheme?: 'light' | 'dark';
  lang?: 'en' | 'zh-CN';
  /** what the tiles are coloured by (the pane's "By type" / "By age") */
  colorBy?: 'type' | 'age';
  /** search box text for the "search" scenario */
  query?: string;
  /** treemap layout (src/lib/treemap/motion.ts › fluentLayout), CSS px */
  pad?: number;
  gap?: number;
  header?: number;
  minHeaderW?: number;
  minArea?: number;
  minSide?: number;
  minWidth?: number;
  labelMinW?: number;
  labelMinH?: number;
  /** tile and open-folder corner radii (src/lib/treemap/paint.ts › paintStyle) */
  radius?: number;
  folderRadius?: number;
  /**
   * The folder look (paintStyle.folders): 'classic' as shipped, 'band' the 1.3.1 candidate.
   * With 'band' the layout's pad, header and folder minimum height follow from the band
   * props below, and the pad / header props are ignored.
   */
  folderLook?: 'classic' | 'band';
  /** band look: white margin round a folder's content, px */
  white?: number;
  /** band look: the band's fill runs on as a 1 px ring inside the outline ('on' / 'off') */
  ring?: 'on' | 'off';
  /** band look: a closed folder's colour band height, px */
  foot?: number;
  /** band look: 0 = the file colour .. 3 = the 1.3.0 closed-folder tint */
  tone?: number;
  /** band look: the colour runs up the side ring from its seam with the colour band, fading out over this many px (0 = none) */
  fade?: number;
  /** band look: a closed folder too small to show its size in the white area has no white area ('on' / 'off') */
  compact?: 'on' | 'off';
  /** band look: the least white area a folder gets, px (6; 18 holds a closed folder's size); the folder minimum height follows */
  minWhite?: number;
  /** band look: every folder gets room for its own size label, on one line or the number over the unit ('on' / 'off'); minWidth still holds (the name), minWhite no longer applies */
  fitSize?: 'on' | 'off';
}

const LAYOUT_KEYS = ['pad', 'gap', 'header', 'minHeaderW', 'minArea', 'minSide', 'minWidth', 'labelMinW', 'labelMinH'] as const;
const LOCKS = Flag.InUse | Flag.NoAccess | Flag.Protected | Flag.RecycleBin;
const DEFAULT_QUERY = '*.vhdx';

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));
async function until(cond: () => boolean, ms = 10000): Promise<boolean> {
  const t0 = performance.now();
  while (!cond()) {
    if (performance.now() - t0 > ms) return false;
    await sleep(40);
  }
  return true;
}

/* ------------------------------------------------------------------ boot */

let booted: Promise<void> | null = null;
let firstScenario: Scenario | null = null;
const origInit = app.init.bind(app);
const origStartScan = app.startScan.bind(app);
let autoScanSkipped = false;

// App.svelte's onMount and every other component share one boot
app.init = () => (booted ??= origInit());
// the browser preview scans C: on its own after boot; the drive picker state skips that once
app.startScan = (root: string) => {
  if (firstScenario === 'picker' && !autoScanSkipped) {
    autoScanSkipped = true;
    return Promise.resolve();
  }
  return origStartScan(root);
};

/** called before the first mount, so the boot itself lands on the right state */
export function prime(p: PageProps) {
  if (booted) return;
  if (p.scenario) {
    firstScenario = p.scenario;
    config.freezeAt = p.scenario === 'scanning' ? 0.45 : null;
  }
  if (p.lang) config.lang = p.lang;
  if (p.scheme) document.documentElement.setAttribute('data-theme', p.scheme);
  applyTreemapStyle(p, false);
}

export function boot(): Promise<void> {
  return app.init();
}

/* ------------------------------------------------------------------ props */

let applied: PageProps = {};
let run = 0;

/**
 * A component's page props, after every render. Only what it sets counts: a title bar with no
 * scenario does not undo the menu a treemap next to it opened.
 */
export async function apply(p: PageProps) {
  if (p.scheme && p.scheme !== applied.scheme) document.documentElement.setAttribute('data-theme', p.scheme);
  if (p.lang && p.lang !== applied.lang) setLang(p.lang, p.lang);
  applyTreemapStyle(p, true);
  const by = p.colorBy;
  if (by && by !== applied.colorBy) void boot().then(() => app.setColorBy(by));
  const prev = applied;
  applied = { ...applied, ...p };
  const s = p.scenario;
  if (!s) return;
  const q = p.query ?? DEFAULT_QUERY;
  if (s === prev.scenario && (s !== 'search' || q === (prev.query ?? DEFAULT_QUERY))) return;
  const mine = ++run;
  await boot();
  if (mine === run) await play(s, q, () => mine === run);
}

/** the layout as the app ships it, for going back from the band look */
const SHIPPED = { pad: fluentLayout.pad, header: fluentLayout.header, minHeight: fluentLayout.minHeight, sizeFit: fluentLayout.sizeFit };

function applyTreemapStyle(p: PageProps, live: boolean) {
  let paint = false;
  let layout = false;
  // the folder look first: with the band look, pad and header follow from it
  const wasBand = paintStyle.folders === 'band';
  if ((p.folderLook === 'classic' || p.folderLook === 'band') && p.folderLook !== paintStyle.folders) {
    paintStyle.folders = p.folderLook;
    paint = true;
  }
  const band = paintStyle.folders === 'band';
  for (const k of LAYOUT_KEYS) {
    if (band && (k === 'pad' || k === 'header')) continue;
    const v = p[k];
    if (typeof v === 'number' && Number.isFinite(v) && fluentLayout[k] !== v) {
      fluentLayout[k] = v;
      layout = true;
    }
  }
  if (typeof p.radius === 'number' && p.radius !== paintStyle.radius) {
    paintStyle.radius = p.radius;
    paint = true;
  }
  if (typeof p.folderRadius === 'number' && p.folderRadius !== paintStyle.folderRadius) {
    paintStyle.folderRadius = p.folderRadius;
    paint = true;
  }
  for (const k of ['white', 'foot', 'tone', 'fade', 'minWhite'] as const) {
    const v = p[k];
    if (typeof v === 'number' && Number.isFinite(v) && paintStyle[k] !== v) {
      paintStyle[k] = v;
      paint = true;
    }
  }
  if ((p.ring === 'on' || p.ring === 'off') && paintStyle.ring !== (p.ring === 'on')) {
    paintStyle.ring = p.ring === 'on';
    paint = true;
  }
  if ((p.compact === 'on' || p.compact === 'off') && paintStyle.compact !== (p.compact === 'on')) {
    paintStyle.compact = p.compact === 'on';
    paint = true;
  }
  if ((p.fitSize === 'on' || p.fitSize === 'off') && paintStyle.fitSize !== (p.fitSize === 'on')) {
    paintStyle.fitSize = p.fitSize === 'on';
    paint = true;
  }
  // the layout the band look needs, or the shipped one back when it is switched off
  const want = band ? bandLayout() : wasBand ? { pad: p.pad ?? SHIPPED.pad, header: p.header ?? SHIPPED.header, minFolderH: SHIPPED.minHeight, sizeFit: SHIPPED.sizeFit } : null;
  if (want && (fluentLayout.pad !== want.pad || fluentLayout.header !== want.header || fluentLayout.minHeight !== want.minFolderH || JSON.stringify(fluentLayout.sizeFit) !== JSON.stringify(want.sizeFit))) {
    fluentLayout.pad = want.pad;
    fluentLayout.header = want.header;
    fluentLayout.minHeight = want.minFolderH;
    fluentLayout.sizeFit = want.sizeFit;
    layout = true;
  }
  if (!live) return;
  if (layout) app.requestLayout('update');
  if (paint) {
    // kick alone only repaints what moved; a new look needs the whole scene again
    app.engine?.painter.reset();
    app.engine?.invalidate();
  }
}

/* ------------------------------------------------------------------ scenarios */

function resetUi() {
  app.closeOverlays();
  app.hideToast();
  app.tip = null;
  if (app.search.text) app.clearSearch();
  app.click(null);
}

async function scanState(frozen: boolean) {
  // the browser preview starts its own scan 300 ms after boot: wait for it rather than race it
  await until(() => !!app.scan || autoScanSkipped, 1500);
  const vol = app.volume ?? app.volumes[0];
  if (!vol) return;
  const running = !!app.scan?.running;
  config.freezeAt = frozen ? 0.45 : null;
  if (frozen ? !running : running || app.phase !== 'map' || !app.scan) await app.startScan(vol.root);
}

/** the scan is done and the treemap has settled */
async function mapReady() {
  // a page with no treemap on it (a search box alone) has no engine and no tiles to wait for
  await until(() => app.phase === 'map' && !!app.scan && !app.scan.running && (!app.engine || app.engine.items.size > 0));
  await sleep(600);
}

function tiles(): Rect[] {
  const out: Rect[] = [];
  for (const it of app.engine?.items.values() ?? []) if (!it.rm) out.push(it.r);
  return out;
}

/** the biggest file (or folder) big enough to point at, preferring ones that can be deleted */
function pick(kind: 'file' | 'folder', deletable: boolean): Rect | null {
  const ok = (r: Rect) =>
    (kind === 'file' ? r.kind === Kind.File : r.kind === Kind.Dir || r.kind === Kind.DirLeaf) && r.w >= 64 && r.h >= 40 && (!deletable || !(r.flags & LOCKS));
  return tiles().filter(ok).sort((a, b) => b.size - a.size)[0] ?? null;
}

/** client coordinates of a point inside a tile, below its label */
function pointIn(r: Rect): { x: number; y: number } {
  const c = app.engine?.canvas.getBoundingClientRect();
  const x = (c?.left ?? 0) + r.x + Math.min(r.w * 0.5, 120);
  const y = (c?.top ?? 0) + r.y + Math.min(r.h * 0.5, 80);
  return { x, y };
}

async function play(s: Scenario, query: string, live: () => boolean) {
  if (!live()) return;
  if (s === 'picker') {
    resetUi();
    app.phase = 'pick';
    return;
  }
  await scanState(s === 'scanning');
  if (s === 'scanning') {
    resetUi();
    return;
  }
  await mapReady();
  if (!live()) return;
  resetUi();
  if (s === 'map') return;
  if (s === 'search') {
    app.setSearch(query);
    app.searchNow();
    return;
  }
  const file = pick('file', s === 'confirm' || s === 'toast') ?? pick('file', false);
  const r = s === 'folder' ? pick('folder', false) : file;
  if (!r) return;
  const at = pointIn(r);
  if (s === 'tooltip') {
    app.hover(r.id, at.x, at.y);
    return;
  }
  app.click(r.id);
  if (s === 'menu') app.openMenu(at.x, at.y, r.id);
  else if (s === 'confirm') app.confirmPurge([r.id]);
  else if (s === 'toast') {
    await app.recycle([r.id]);
    // keep it up: a preview is looked at longer than a toast lives
    const t = (app as unknown as { toastTimer: ReturnType<typeof setTimeout> | null }).toastTimer;
    if (t) clearTimeout(t);
  }
}
