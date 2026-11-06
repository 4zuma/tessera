/**
 * Application state and actions. Components read the $state fields; the canvas
 * engine reads the same object as its host (selection, hover, filter, theme).
 */
import { SvelteSet } from 'svelte/reactivity';
import { createBackend } from './backend';
import { CAT_NAMES } from './backend/categories';
import { decodeLayout, Flag, FREE, groupFolder, isGroupId, isNodeId, Kind, NONE, UNSCANNED, type Layout, type Rect } from './backend/codec';
import type { Backend, CatStat, Crumb, Details, PlatformInfo, ScanProgress, SizeMode, Volume } from './backend/types';
import { fmtBytes, fmtSecs } from './format';
import type { IconName } from './icons';
import { applyThemeCss, makeTheme, type Scheme, type Theme } from './theme';
import { TreemapEngine, type ApplyOpts, type EngineHost, type EngineStats, type MotionKind } from './treemap/engine';
import type { GpuMode } from './treemap/gpu';
import { fluentLayout, fluentMotion } from './treemap/motion';
import { FluentPainter } from './treemap/paint';

export type Phase = 'boot' | 'pick' | 'map';

export interface ToastAction {
  label: string;
  icon?: IconName;
  run: () => void;
}
export interface ToastData {
  id: number;
  kind: 'info' | 'success' | 'recycle' | 'error';
  title?: string;
  text: string;
  actions: ToastAction[];
}

export type MenuRow =
  | { type: 'head'; label: string }
  | { type: 'sep' }
  | { type: 'note'; label: string; lock?: boolean }
  | { type: 'item'; label: string; icon: IconName; key?: string; danger?: boolean; disabled?: boolean; run: () => void };

export interface MenuData {
  x: number;
  y: number;
  rows: MenuRow[];
}

export interface ConfirmData {
  title: string;
  lead: string;
  size: string;
  body: string;
  ok: string;
  run: () => void;
}

export interface TipData {
  x: number;
  y: number;
  id: number;
  name: string;
  size: number;
  kind: string;
  share: number | null;
}

export interface ScanState {
  running: boolean;
  files: number;
  dirs: number;
  scanned: number;
  used: number;
  elapsedMs: number;
  errors: number;
}

/** what the selection card shows */
export interface SelInfo {
  count: number;
  bytes: number;
  lockReason: string | null;
}

const PRIORITY: Record<MotionKind, number> = { instant: 0, update: 1, live: 2, scan: 2, delete: 4, mode: 5, morph: 5, zoom: 6 };
const SCAN_LAYOUT_MS = 250;

let toastSeq = 0;

function lockFromFlags(flags: number): string | null {
  if (flags & Flag.InUse) return '系统正在使用的文件，不能删除';
  if (flags & Flag.NoAccess) return '没有访问权限';
  // the flag says only that it is locked; the core's details say why (Windows, a personal folder…)
  if (flags & Flag.Protected) return '受保护，不能删除';
  if (flags & Flag.RecycleBin) return '请使用“清空回收站”';
  return null;
}

/** files and folders can be selected; groups, free and unscanned space cannot */
function selectable(r: Rect): boolean {
  return isNodeId(r.id) && (r.kind === Kind.File || r.kind === Kind.Dir || r.kind === Kind.DirLeaf);
}

export class App implements EngineHost {
  backend: Backend | null = null;
  phase = $state<Phase>('boot');
  platform = $state<PlatformInfo | null>(null);
  volumes = $state<Volume[]>([]);
  volume = $state<Volume | null>(null);
  view = $state(0);
  crumbs = $state<Crumb[]>([]);
  up = $state<number | null>(null);
  sizeMode = $state<SizeMode>('disk');
  readonly sel = new SvelteSet<number>();
  hoverId = $state<number | null>(null);
  filter = $state<number | null>(null);
  scan = $state<ScanState | null>(null);
  rev = $state(0);
  bytesPerPx = $state(0);
  legend = $state<CatStat[]>([]);
  details = $state<Details | null>(null);
  selInfo = $state<SelInfo>({ count: 0, bytes: 0, lockReason: null });
  toast = $state<ToastData | null>(null);
  menu = $state<MenuData | null>(null);
  confirm = $state<ConfirmData | null>(null);
  tip = $state<TipData | null>(null);
  scheme = $state<Scheme>('light');
  theme = $state.raw<Theme>(makeTheme('light', null));
  stats = $state<EngineStats>({ rects: 0, fps: 0, frameMs: 0, busy: false, gpu: false });
  showHud = $state(false);
  error = $state<string | null>(null);
  reduced = false;
  undoToken = $state<number | null>(null);
  /** where a Shift+click run starts */
  private anchor: number | null = null;

  engine: TreemapEngine | null = null;
  private ro: ResizeObserver | null = null;
  /** until this time, the next stage resize comes from the details pane (see paneToggled) */
  private paneUntil = 0;
  private pending: { kind: MotionKind; opt: ApplyOpts } | null = null;
  private inFlight = false;
  private scanLayoutTimer: ReturnType<typeof setTimeout> | null = null;
  private lastScanLayout = 0;
  private legendTimer: ReturnType<typeof setTimeout> | null = null;
  private lastLegend = 0;
  private detailsSeq = 0;
  private selSeq = 0;
  private scanId = 0;
  private toastTimer: ReturnType<typeof setTimeout> | null = null;
  private nameCache = new Map<number, string>();
  /** bytes shown in the current view (without free space) */
  private viewTotal = 0;

  get scanning() {
    return !!this.scan?.running;
  }

  /* ------------------------------------------------------------------ boot */

  async init() {
    this.reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
    try {
      this.showHud = localStorage.getItem('tessera.hud') === '1';
    } catch {
      /* storage can be unavailable */
    }
    const b = await createBackend();
    this.backend = b;
    this.platform = b.platform;
    document.documentElement.dataset.backdrop = b.platform.backdrop;
    document.documentElement.dataset.host = b.platform.native ? 'native' : 'browser';
    // follow the system theme; a host page may pin one with data-theme on <html>
    const mq = matchMedia('(prefers-color-scheme: dark)');
    const root = document.documentElement;
    const resolve = (): Scheme => {
      const pinned = root.getAttribute('data-theme');
      return pinned === 'dark' || pinned === 'light' ? pinned : mq.matches ? 'dark' : 'light';
    };
    this.setScheme(resolve());
    mq.addEventListener('change', () => this.setScheme(resolve()));
    new MutationObserver(() => this.setScheme(resolve())).observe(root, { attributes: true, attributeFilter: ['data-theme'] });
    await b.on('scan-progress', p => this.onProgress(p));
    await b.on('tree-changed', e => this.onTreeChanged(e.rev, e.bytesDelta));
    this.volumes = await b.listVolumes();
    this.phase = 'pick';
    // the browser preview opens on a scan of the sample C: so the map is visible right away
    if (!b.platform.native && this.volumes.length) setTimeout(() => void this.startScan(this.volumes[0].root), 300);
  }

  setScheme(s: Scheme) {
    this.scheme = s;
    this.theme = makeTheme(s, this.platform?.accent ?? null);
    applyThemeCss(this.theme);
    this.engine?.painter.reset();
    this.engine?.kick();
  }

  toggleHud() {
    this.showHud = !this.showHud;
    try {
      localStorage.setItem('tessera.hud', this.showHud ? '1' : '0');
    } catch {
      /* ignore */
    }
  }

  /* ------------------------------------------------------------------ canvas */

  attach(canvas: HTMLCanvasElement, stage: HTMLElement, gpuCanvas: HTMLCanvasElement | null = null) {
    // 'tessera.gpu' = 'off' keeps the treemap on the 2D canvas, 'force' takes even a software WebGL
    let gpuMode: GpuMode = 'auto';
    try {
      const v = localStorage.getItem('tessera.gpu');
      if (v === 'off' || v === 'force') gpuMode = v;
    } catch {
      /* storage can be unavailable */
    }
    this.engine = new TreemapEngine(canvas, this, new FluentPainter(), fluentMotion, fluentLayout.outer, gpuCanvas, gpuMode);
    this.engine.onStats = s => (this.stats = { ...s });
    let first = true;
    const measure = () => {
      const r = stage.getBoundingClientRect();
      const changed = this.engine!.resize(Math.max(50, Math.floor(r.width)), Math.max(50, Math.floor(r.height)), Math.min(3, window.devicePixelRatio || 1));
      if (changed) {
        this.engine!.drawNow();
        // dragging the window edge snaps; the details pane opening or closing glides
        const pane = performance.now() < this.paneUntil;
        this.paneUntil = 0;
        this.requestLayout(first ? 'instant' : this.scanning ? 'scan' : pane ? 'update' : 'instant');
        first = false;
      }
    };
    this.ro = new ResizeObserver(measure);
    this.ro.observe(stage);
    // moving the window to a monitor with another scale does not resize the element
    const watchDpr = () => {
      const mq = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      mq.addEventListener('change', () => {
        measure();
        watchDpr();
      }, { once: true });
    };
    watchDpr();
    measure();
  }

  /**
   * The details pane is about to open or close. The stage changes width once (at once when
   * it opens, after the pane has slid out when it closes), and that resize animates the map
   * into its new size instead of snapping.
   */
  paneToggled() {
    this.paneUntil = performance.now() + 1000;
  }

  detach() {
    this.ro?.disconnect();
    this.engine?.destroy();
    this.engine = null;
  }

  rectOf(id: number): Rect | null {
    return this.engine?.items.get(id)?.r ?? null;
  }

  /* ------------------------------------------------------------------ layout */

  /** at most one layout request in flight; later requests merge, the stronger motion wins */
  requestLayout(kind: MotionKind = 'update', opt: ApplyOpts = {}) {
    if (!this.backend || !this.engine || !this.volume) return;
    if (!this.pending || PRIORITY[kind] >= PRIORITY[this.pending.kind]) this.pending = { kind, opt };
    if (!this.inFlight) void this.pump();
  }

  private async pump() {
    this.inFlight = true;
    try {
      while (this.pending && this.engine && this.backend) {
        const { kind, opt } = this.pending;
        this.pending = null;
        const e = this.engine;
        const view = this.view;
        const buf = await this.backend.layout({ view, width: e.W, height: e.H, sizeMode: this.sizeMode, style: fluentLayout });
        let L: Layout;
        try {
          L = decodeLayout(buf);
        } catch (err) {
          this.fail('布局数据无法解析', err);
          break;
        }
        // the user moved on while we waited: the newer request is already pending
        if (view !== this.view) continue;
        if (L.view !== view) {
          // the folder vanished (deleted outside Tessera); the core fell back to another view
          this.view = L.view;
          void this.refreshNav();
        }
        for (const r of L.rects) if (r.name && isNodeId(r.id)) this.nameCache.set(r.id, r.name);
        e.apply(L, kind, opt);
        this.bytesPerPx = L.bytesPerPx;
        this.afterLayout(L);
      }
    } catch (err) {
      this.fail('请求布局失败', err);
    } finally {
      this.inFlight = false;
    }
  }

  private afterLayout(L: Layout) {
    const ids = new Set<number>();
    let total = 0;
    for (const r of L.rects) {
      ids.add(r.id);
      if (r.depth === 1 && r.kind !== Kind.Free) total += r.size;
    }
    this.viewTotal = total;
    let selChanged = false;
    for (const id of [...this.sel]) if (!ids.has(id)) {
      this.sel.delete(id);
      selChanged = true;
    }
    if (selChanged) void this.refreshSel();
    if (this.hoverId != null && !ids.has(this.hoverId)) this.hover(null);
    if (L.rev !== this.rev) this.rev = L.rev;
    void this.refreshDetails();
  }

  /* ------------------------------------------------------------------ scanning */

  async startScan(root: string) {
    const b = this.backend;
    if (!b) return;
    const vol = this.volumes.find(v => v.root === root) ?? null;
    this.closeOverlays();
    this.sel.clear();
    this.filter = null;
    this.undoToken = null;
    this.volume = vol;
    this.view = 0;
    this.phase = 'map';
    this.legend = [];
    this.nameCache.clear();
    this.engine?.items.clear();
    this.scan = { running: true, files: 0, dirs: 0, scanned: 0, used: vol ? vol.capacity - vol.free : 0, elapsedMs: 0, errors: 0 };
    try {
      const s = await b.scanStart(root);
      this.scanId = s.scanId;
      this.view = s.rootId;
      await this.refreshNav();
      this.requestLayout('instant');
    } catch (err) {
      this.scan = null;
      this.fail('无法开始扫描', err);
    }
  }

  async rescan() {
    if (this.volume) await this.startScan(this.volume.root);
  }

  async cancelScan() {
    await this.backend?.scanCancel();
    if (this.scan) this.scan = { ...this.scan, running: false };
    this.requestLayout('update');
  }

  private onProgress(p: ScanProgress) {
    if (p.scanId !== this.scanId && this.scanId !== 0) return;
    this.scan = { running: !p.done, files: p.files, dirs: p.dirs, scanned: p.scanned, used: p.used, elapsedMs: p.elapsedMs, errors: p.errors };
    if (this.phase !== 'map') return;
    const now = performance.now();
    if (p.done) {
      if (this.scanLayoutTimer) clearTimeout(this.scanLayoutTimer);
      this.scanLayoutTimer = null;
      this.requestLayout('scan');
      this.refreshLegend(true);
      void this.refreshVolumes();
      return;
    }
    const wait = SCAN_LAYOUT_MS - (now - this.lastScanLayout);
    if (wait <= 0) {
      this.lastScanLayout = now;
      this.requestLayout('scan');
    } else if (!this.scanLayoutTimer) {
      this.scanLayoutTimer = setTimeout(() => {
        this.scanLayoutTimer = null;
        this.lastScanLayout = performance.now();
        this.requestLayout('scan');
      }, wait);
    }
    this.refreshLegend(false);
  }

  private onTreeChanged(rev: number, bytesDelta: number) {
    this.rev = rev;
    if (this.volume && bytesDelta) this.volume = { ...this.volume, free: this.volume.free - bytesDelta };
    this.requestLayout('live');
    this.refreshLegend(false);
  }

  private async refreshVolumes() {
    if (!this.backend) return;
    this.volumes = await this.backend.listVolumes();
    if (this.volume) this.volume = this.volumes.find(v => v.root === this.volume!.root) ?? this.volume;
  }

  /* ------------------------------------------------------------------ side data */

  async refreshNav() {
    if (!this.backend) return;
    const v = this.view;
    const n = await this.backend.nav(v);
    if (v !== this.view) return;
    this.crumbs = n.crumbs;
    this.up = n.up;
  }

  /** legend by type: immediately on view changes, at most once a second while things change */
  refreshLegend(now: boolean) {
    const run = async () => {
      this.legendTimer = null;
      this.lastLegend = performance.now();
      if (!this.backend || !this.volume) return;
      const v = this.view;
      const st = await this.backend.catStats(v, this.sizeMode);
      if (v === this.view) this.legend = st;
    };
    if (now) {
      if (this.legendTimer) clearTimeout(this.legendTimer);
      void run();
      return;
    }
    if (this.legendTimer) return;
    this.legendTimer = setTimeout(run, Math.max(0, 1000 - (performance.now() - this.lastLegend)));
  }

  /** the details card follows hover, else a single selection, else the current folder */
  get focusId(): number {
    if (this.hoverId != null) return this.hoverId;
    if (this.sel.size === 1) return [...this.sel][0];
    return this.view;
  }

  async refreshDetails() {
    if (!this.backend || !this.volume) return;
    const id = this.focusId;
    const seq = ++this.detailsSeq;
    const d = await this.backend.details(id, this.view, this.sizeMode);
    if (seq !== this.detailsSeq) return;
    if (d && isGroupId(id)) d.size = this.rectOf(id)?.size ?? 0;
    this.details = d;
  }

  async refreshSel() {
    const ids = [...this.sel];
    const seq = ++this.selSeq;
    let bytes = 0;
    let lock: string | null = null;
    for (const id of ids) {
      const r = this.rectOf(id);
      if (r) {
        bytes += r.size;
        lock ??= lockFromFlags(r.flags);
      }
    }
    this.selInfo = { count: ids.length, bytes, lockReason: lock };
    if (!this.backend || ids.length > 24) return;
    // the core knows reasons the flags do not carry (files in use, ACLs) and says why a
    // protected folder is locked, which the flag alone cannot
    const ds = await Promise.all(ids.map(id => this.backend!.details(id, this.view, this.sizeMode)));
    if (seq !== this.selSeq) return;
    const why = ds.map(d => d?.lockReason).find(Boolean) ?? null;
    if (why) this.selInfo = { ...this.selInfo, lockReason: why };
  }

  /* ------------------------------------------------------------------ pointer */

  hover(id: number | null, x = 0, y = 0) {
    if (id !== this.hoverId) {
      this.hoverId = id;
      void this.refreshDetails();
    }
    if (id == null || this.menu || this.confirm) {
      this.tip = null;
      return;
    }
    const r = this.rectOf(id);
    if (!r) {
      this.tip = null;
      return;
    }
    const parent = r.parent !== NONE ? this.rectOf(r.parent) : null;
    const viewSize = this.viewTotal || null;
    const kind =
      r.kind === Kind.DirLeaf && r.cat < CAT_NAMES.length ? `文件夹 · 多为${CAT_NAMES[r.cat]}` : r.kind === Kind.Dir || r.kind === Kind.DirLeaf ? '文件夹' : r.kind === Kind.Group ? '合并显示的小项' : r.kind === Kind.Free ? '可用空间' : r.kind === Kind.Unscanned ? '扫描中' : (CAT_NAMES[r.cat] ?? '文件');
    const name =
      r.kind === Kind.Group ? `${r.count} 个小项` : r.kind === Kind.Free ? '可用空间' : r.kind === Kind.Unscanned ? '尚未扫描' : r.name || this.nameCache.get(id) || '';
    this.tip = { x, y, id, name, size: r.size, kind, share: parent ? r.size / Math.max(1, parent.size) : r.kind === Kind.Free ? null : viewSize ? r.size / viewSize : null };
    if (!name && isNodeId(id)) {
      void this.backend?.details(id, this.view, this.sizeMode).then(d => {
        if (!d) return;
        this.nameCache.set(id, d.name);
        if (this.tip && this.tip.id === id) this.tip = { ...this.tip, name: d.name };
      });
    }
  }

  moveTip(x: number, y: number) {
    if (this.tip) this.tip = { ...this.tip, x, y };
  }

  /**
   * Explorer-style selection. Ctrl toggles one tile. Shift selects the run of siblings
   * between the anchor (the last plain or Ctrl click) and this tile, in the order they are
   * laid out, which is by size; Ctrl+Shift adds that run to the selection. When the two
   * tiles are not siblings there is no run, so Shift just adds the tile.
   */
  click(id: number | null, mods: { ctrl?: boolean; shift?: boolean } = {}) {
    this.menu = null;
    const r = id != null ? this.rectOf(id) : null;
    if (!r || !selectable(r)) {
      if (this.sel.size && !mods.ctrl && !mods.shift) this.sel.clear();
    } else if (mods.shift) {
      const run = this.anchor != null ? this.siblingRun(this.anchor, r.id) : null;
      if (run) {
        if (!mods.ctrl) this.sel.clear();
        for (const x of run) this.sel.add(x);
      } else {
        this.sel.add(r.id);
        this.anchor ??= r.id;
      }
    } else if (mods.ctrl) {
      if (this.sel.has(r.id)) this.sel.delete(r.id);
      else this.sel.add(r.id);
      this.anchor = r.id;
    } else {
      this.selectOnly(r.id);
    }
    void this.refreshSel();
    void this.refreshDetails();
    this.engine?.kick();
  }

  private selectOnly(id: number) {
    this.sel.clear();
    this.sel.add(id);
    this.anchor = id;
  }

  /** the tiles from a to b (inclusive) among their siblings in layout order, or null */
  private siblingRun(a: number, b: number): number[] | null {
    const items = this.engine?.items;
    const A = items?.get(a);
    const B = items?.get(b);
    if (!items || !A || !B || A.rm || B.rm || A.r.parent !== B.r.parent) return null;
    const lo = Math.min(A.ord, B.ord);
    const hi = Math.max(A.ord, B.ord);
    const run: { id: number; ord: number }[] = [];
    for (const it of items.values())
      if (!it.rm && it.r.parent === A.r.parent && it.ord >= lo && it.ord <= hi && selectable(it.r)) run.push(it);
    return run.sort((x, y) => x.ord - y.ord).map(it => it.id);
  }

  /* ------------------------------------------------------------------ navigation */

  zoomInto(id: number | null) {
    if (id == null) return;
    if (isGroupId(id)) return void this.zoomTo(groupFolder(id));
    if (!isNodeId(id)) return;
    const r = this.rectOf(id);
    if (!r) return;
    if (r.kind === Kind.Dir || r.kind === Kind.DirLeaf) return void this.zoomTo(r.target, id);
    // a file: open its folder if that is not the current view
    if (r.parent !== NONE) this.zoomTo(this.rectOf(r.parent)?.target ?? r.parent, r.parent);
  }

  /**
   * Camera move. Zooming in grows the clicked tile to fill the stage; zooming out
   * shrinks the stage back into the old folder's tile in the parent's layout.
   */
  async zoomTo(target: number, viaTile?: number) {
    if (!this.backend || target === this.view) return;
    this.closeOverlays();
    const prev = this.view;
    const e = this.engine;
    const tile = viaTile ?? target;
    const A = e?.items.get(tile);
    const prevCrumbs = this.crumbs;
    const isAncestor = prevCrumbs.some(c => c.id === target);
    let cam: ApplyOpts['cam'];
    if (A && !A.rm && e && !isAncestor) {
      const box = { x: A.x, y: A.y, w: Math.max(1, A.w), h: Math.max(1, A.h) };
      cam = () => ({ A: box, B: e.stageBox() });
    } else if (isAncestor && e) {
      // the folder we leave, or its nearest ancestor that is drawn in the new layout
      const chain = [prev, ...prevCrumbs.map(c => c.id).reverse()];
      cam = rects => {
        for (const id of chain) {
          if (id === target) return null;
          const r = rects.find(q => q.id === id);
          if (r) return { A: e.stageBox(), B: r };
        }
        return null;
      };
    }
    this.view = target;
    this.sel.clear();
    this.selInfo = { count: 0, bytes: 0, lockReason: null };
    this.hoverId = null;
    this.tip = null;
    this.requestLayout(cam ? 'zoom' : 'morph', { cam });
    await this.refreshNav();
    this.refreshLegend(true);
  }

  zoomOut() {
    if (this.up != null) void this.zoomTo(this.up);
  }

  setSizeMode(m: SizeMode) {
    if (m === this.sizeMode) return;
    this.sizeMode = m;
    this.requestLayout('mode');
    this.refreshLegend(true);
    this.showToast(
      'info',
      m === 'disk' ? '按“占用空间”显示：文件在磁盘上实际占用的空间，清理时能腾出的就是这么多。' : '按“实际大小”显示：文件内容的字节数。OneDrive 仅在线文件和压缩过的系统文件会显得更大。',
    );
  }

  setFilter(cat: number | null) {
    this.filter = this.filter === cat ? null : cat;
    this.engine?.kick();
  }

  /* ------------------------------------------------------------------ deleting */

  targets(id?: number | null): number[] {
    if (id != null && this.sel.has(id) && this.sel.size > 1) return [...this.sel];
    if (id != null) return [id];
    return [...this.sel];
  }

  private deletable(ids: number[]) {
    return ids.filter(id => isNodeId(id) && !lockFromFlags(this.rectOf(id)?.flags ?? 0));
  }

  private nameOf(id: number) {
    return this.rectOf(id)?.name || this.nameCache.get(id) || '';
  }

  async recycle(ids: number[]) {
    ids = this.deletable(ids);
    if (!ids.length || !this.backend) return;
    this.closeOverlays();
    const names = ids.map(id => this.nameOf(id));
    const res = await this.backend.recycle(ids);
    this.sel.clear();
    this.selInfo = { count: 0, bytes: 0, lockReason: null };
    if (res.ok.length) {
      this.undoToken = res.undo;
      this.requestLayout('delete', { reflow: true });
      this.refreshLegend(true);
      const what = res.ok.length > 1 ? `${res.ok.length} 项` : names[ids.indexOf(res.ok[0])] || '1 项';
      const actions: ToastAction[] = [];
      if (res.undo != null) actions.push({ label: '撤销', icon: 'undo', run: () => void this.undo() });
      actions.push({ label: '清空回收站', icon: 'sweep', run: () => this.confirmEmptyBin() });
      this.showToast('recycle', '回收站和这些文件在同一个盘上，清空回收站后才会真正腾出空间。', actions, `已移到回收站：${what}（${fmtBytes(res.bytes)}）`);
    }
    if (res.failed.length) this.showToast('error', res.failed.map(f => `${this.nameOf(f.id) || f.id}：${f.reason}`).join('\n'), [], '有项目没能删除');
  }

  async undo() {
    const tok = this.undoToken;
    if (tok == null || !this.backend) return;
    this.undoToken = null;
    const res = await this.backend.restore(tok);
    this.requestLayout('delete', { reflow: true });
    this.refreshLegend(true);
    for (const id of res.ok) this.sel.add(id);
    void this.refreshSel();
    this.showToast('info', '已撤销，文件放回了原处。');
  }

  confirmPurge(ids: number[]) {
    ids = this.deletable(ids);
    if (!ids.length) return;
    const rs = ids.map(id => this.rectOf(id)!).filter(Boolean);
    const total = rs.reduce((a, r) => a + r.size, 0);
    const first = this.nameOf(ids[0]) || '所选项目';
    this.openConfirm({
      title: '永久删除？',
      lead: ids.length > 1 ? `${ids.length} 项` : first,
      size: fmtBytes(total),
      body: `${ids.length > 1 ? ids.slice(0, 3).map(id => this.nameOf(id)).filter(Boolean).join('、') + (ids.length > 3 ? ' 等。' : '。') : ''}不经过回收站，删除后无法恢复，会立刻腾出 ${fmtBytes(total)}。`,
      ok: '永久删除',
      run: () => void this.purge(ids),
    });
  }

  async purge(ids: number[]) {
    if (!this.backend) return;
    const res = await this.backend.purge(ids);
    this.sel.clear();
    this.selInfo = { count: 0, bytes: 0, lockReason: null };
    this.undoToken = null;
    this.requestLayout('delete', { reflow: true });
    this.refreshLegend(true);
    if (this.volume) this.volume = { ...this.volume, free: this.volume.free + res.bytes };
    if (res.ok.length) this.showToast('success', `可用空间现在是 ${fmtBytes(this.volume?.free ?? 0)}。`, [], `已永久删除，腾出 ${fmtBytes(res.bytes)}`);
    if (res.failed.length) this.showToast('error', res.failed.map(f => `${this.nameOf(f.id) || f.id}：${f.reason}`).join('\n'), [], '有项目没能删除');
  }

  confirmEmptyBin() {
    this.closeOverlays();
    this.openConfirm({
      title: '清空回收站？',
      lead: `${this.volume?.root.slice(0, 2) ?? ''} 上的回收站`,
      size: '',
      body: '回收站里的所有文件会被永久删除，删除后无法恢复。',
      ok: '清空回收站',
      run: () => void this.emptyBin(),
    });
  }

  async emptyBin() {
    if (!this.backend || !this.volume) return;
    const res = await this.backend.emptyRecycleBin(this.volume.root);
    this.undoToken = null;
    this.requestLayout('delete', { reflow: true });
    this.refreshLegend(true);
    this.volume = { ...this.volume, free: this.volume.free + res.bytes };
    if (!res.failed.length) {
      this.showToast('success', `可用空间现在是 ${fmtBytes(this.volume.free)}。`, [], `回收站已清空，腾出 ${fmtBytes(res.bytes)}`);
      return;
    }
    const lines = res.failed.slice(0, 5).map(f => `${this.nameOf(f.id) || f.id}：${f.reason}`);
    if (res.failed.length > 5) lines.push(`还有 ${res.failed.length - 5} 项`);
    this.showToast('error', `腾出了 ${fmtBytes(res.bytes)}，这些项目还在回收站里：\n${lines.join('\n')}`, [], '回收站没能全部清空');
  }

  async copyPath(ids: number[]) {
    if (!this.backend) return;
    const ds = await Promise.all(ids.filter(isNodeId).map(id => this.backend!.details(id, this.view, this.sizeMode)));
    const text = ds.map(d => d?.path).filter(Boolean).join('\r\n');
    try {
      await navigator.clipboard.writeText(text);
      this.showToast('info', ids.length > 1 ? `已复制 ${ids.length} 个路径` : '已复制路径');
    } catch {
      this.showToast('info', text, [], '路径');
    }
  }

  async reveal(id: number) {
    if (!this.backend || !isNodeId(id)) return;
    if (!this.platform?.core) {
      const d = await this.backend.details(id, this.view, this.sizeMode);
      this.showToast('info', `接入扫描核心后会执行 explorer.exe /select,"${d?.path ?? ''}"`, [], '预览数据里没有真实文件');
      return;
    }
    await this.backend.reveal(id);
  }

  async relaunchElevated() {
    if (!this.platform?.native) {
      this.showToast('info', '在 Tessera 程序里，这个按钮会以管理员身份重启，改用读取 MFT 的方式扫描，速度快很多。', [], '预览里不会重启');
      return;
    }
    await this.backend?.relaunchElevated();
  }

  demoDownload() {
    if (this.backend?.demo?.startDownload()) this.showToast('info', 'Downloads 里有个文件正在下载，每 500 毫秒推一次变化，树图在两次之间补间，不会整屏重画闪烁。', [], '模拟实时更新');
  }

  /* ------------------------------------------------------------------ overlays */

  openMenu(x: number, y: number, id: number | null) {
    this.tip = null;
    if (id == null || id === UNSCANNED) return void (this.menu = null);
    const r = this.rectOf(id);
    if (!r) return;
    if (isNodeId(id) && !this.sel.has(id)) {
      this.selectOnly(id);
      void this.refreshSel();
      this.engine?.kick();
    }
    const ids = this.targets(id);
    const rows: MenuRow[] = [];
    if (id === FREE) {
      rows.push({ type: 'head', label: `可用空间 · ${fmtBytes(r.size)}` });
      rows.push({ type: 'note', label: '这个盘上还空着的部分。先看看最大的几块。' });
    } else if (isGroupId(id)) {
      rows.push({ type: 'head', label: `${r.count} 个小项 · ${fmtBytes(r.size)}` });
      rows.push({ type: 'item', icon: 'zoom', label: '放大查看这些小项', key: 'Enter', run: () => this.zoomInto(id) });
    } else {
      const multi = ids.length > 1;
      const total = ids.reduce((a, x) => a + (this.rectOf(x)?.size ?? 0), 0);
      const isDir = r.kind === Kind.Dir || r.kind === Kind.DirLeaf;
      rows.push({ type: 'head', label: multi ? `已选 ${ids.length} 项 · ${fmtBytes(total)}` : `${this.nameOf(id) || (isDir ? '文件夹' : '文件')} · ${fmtBytes(r.size)}` });
      if (!multi && isDir) rows.push({ type: 'item', icon: 'zoom', label: '放大此文件夹', key: 'Enter', run: () => this.zoomInto(id) });
      rows.push({ type: 'item', icon: 'reveal', label: '在资源管理器中显示', run: () => void this.reveal(id) });
      rows.push({ type: 'item', icon: 'copy', label: multi ? `复制 ${ids.length} 个路径` : '复制路径', key: 'Ctrl+C', run: () => void this.copyPath(ids) });
      rows.push({ type: 'sep' });
      if (r.flags & Flag.RecycleBin) {
        rows.push({ type: 'item', icon: 'sweep', label: '清空回收站…', danger: true, run: () => this.confirmEmptyBin() });
      } else {
        const flagged = ids.find(x => lockFromFlags(this.rectOf(x)?.flags ?? 0));
        // the details card usually already holds the core's reason for the item under the pointer
        const d = this.details;
        const why = flagged == null ? null : (d?.id === flagged && d.lockReason) || lockFromFlags(this.rectOf(flagged)?.flags ?? 0);
        rows.push({ type: 'item', icon: 'trash', label: '移到回收站', key: 'Del', disabled: !!why, run: () => void this.recycle(ids) });
        rows.push({ type: 'item', icon: 'purge', label: '永久删除…', key: 'Shift+Del', danger: true, disabled: !!why, run: () => this.confirmPurge(ids) });
        if (why) rows.push({ type: 'note', label: why, lock: true });
      }
    }
    this.menu = { x, y, rows };
  }

  openConfirm(c: ConfirmData) {
    this.menu = null;
    this.tip = null;
    this.confirm = c;
  }

  /** give keyboard focus back to the map */
  focusMap() {
    this.engine?.canvas.focus({ preventScroll: true });
  }

  closeConfirm(ok: boolean) {
    const c = this.confirm;
    this.confirm = null;
    if (ok && c) c.run();
  }

  closeOverlays() {
    this.menu = null;
    this.confirm = null;
    this.tip = null;
  }

  showToast(kind: ToastData['kind'], text: string, actions: ToastAction[] = [], title?: string) {
    if (this.toastTimer) clearTimeout(this.toastTimer);
    this.toast = { id: ++toastSeq, kind, title, text, actions };
    this.toastTimer = setTimeout(() => (this.toast = null), actions.length ? 10000 : kind === 'error' ? 9000 : 5200);
  }

  hideToast() {
    if (this.toastTimer) clearTimeout(this.toastTimer);
    this.toast = null;
  }

  private fail(msg: string, err: unknown) {
    console.error(msg, err);
    this.showToast('error', String(err instanceof Error ? err.message : err), [], msg);
  }

  /* ------------------------------------------------------------------ keyboard */

  onKey(e: KeyboardEvent) {
    if (this.phase !== 'map') return;
    const typing = e.target instanceof HTMLElement && e.target.matches('input, textarea, select');
    if (this.confirm) {
      if (e.key === 'Escape') {
        e.preventDefault();
        this.closeConfirm(false);
      }
      return;
    }
    if (this.menu) {
      if (e.key === 'Escape') {
        e.preventDefault();
        this.menu = null;
      }
      return;
    }
    if (typing) return;
    const sel = [...this.sel];
    const ctrl = e.ctrlKey || e.metaKey;
    if (e.key === 'Delete') {
      e.preventDefault();
      if (e.shiftKey) this.confirmPurge(sel);
      else void this.recycle(sel);
    } else if (e.key === 'Enter' && sel.length === 1) {
      e.preventDefault();
      this.zoomInto(sel[0]);
    } else if (e.key === 'Backspace' || (e.altKey && e.key === 'ArrowLeft') || e.key === 'BrowserBack') {
      e.preventDefault();
      this.zoomOut();
    } else if (e.key === 'Escape') {
      this.sel.clear();
      this.filter = null;
      this.hideToast();
      void this.refreshSel();
      void this.refreshDetails();
      this.engine?.kick();
    } else if (e.key.startsWith('Arrow') && !e.altKey) {
      e.preventDefault();
      const next = this.engine?.neighbor(sel[sel.length - 1], e.key as 'ArrowLeft', selectable);
      if (next) {
        this.selectOnly(next.id);
        void this.refreshSel();
        void this.refreshDetails();
        this.engine?.kick();
      }
    } else if (ctrl && e.key.toLowerCase() === 'z') {
      e.preventDefault();
      void this.undo();
    } else if (ctrl && e.key.toLowerCase() === 'c' && sel.length) {
      e.preventDefault();
      void this.copyPath(sel);
    } else if (e.key === 'F5') {
      e.preventDefault();
      void this.rescan();
    } else if (ctrl && e.shiftKey && e.key.toLowerCase() === 'f') {
      e.preventDefault();
      this.toggleHud();
    }
  }

  get scanText(): string {
    const s = this.scan;
    if (!s) return '';
    if (s.running) return `正在扫描 · ${s.used ? Math.min(100, Math.round((s.scanned / s.used) * 100)) : 0}% · 已找到 ${s.files.toLocaleString('zh-CN')} 个文件`;
    return `扫描完成 · ${s.files.toLocaleString('zh-CN')} 个文件，${s.dirs.toLocaleString('zh-CN')} 个文件夹 · 用时 ${fmtSecs(s.elapsedMs)}`;
  }
}

export const app = new App();
