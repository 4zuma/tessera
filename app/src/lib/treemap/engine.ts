/**
 * Treemap view engine: keeps one tweened item per rect id, runs the frame loop only
 * while something moves, and does hit-testing locally (no round trip to Rust).
 * It knows nothing about the backend; the app hands it decoded layouts.
 */
import { Kind, type Layout, type Rect } from '../backend/codec';
import type { Theme } from '../theme';
import type { Motion } from './ease';

export type MotionKind = 'instant' | 'update' | 'scan' | 'live' | 'zoom' | 'morph' | 'mode' | 'delete';

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}
interface Frame extends Box {
  a: number;
}

export interface MotionSet {
  zoom: Motion;
  update: Motion;
  scan: Motion;
  live: Motion;
  morph: Motion;
  mode: Motion;
  delete: Motion;
  instant: Motion;
  remove: Motion;
  reflow: Motion;
  reflowDelay: number;
  appear: Motion;
  appearFrom(r: Box): Box;
  removeTo(r: Box): Box;
}

export class Item {
  x = 0;
  y = 0;
  w = 0;
  h = 0;
  a = 0;
  /** hover amount 0..1, eased */
  hv = 0;
  /** dimmed by the category filter 0..1, eased */
  dm = 0;
  /** visible alpha after dimming */
  va = 0;
  /** leaving the map */
  rm = false;
  done = true;
  ord = 0;
  private f: Frame = { x: 0, y: 0, w: 0, h: 0, a: 0 };
  private t: Frame = { x: 0, y: 0, w: 0, h: 0, a: 0 };
  private t0 = 0;
  private dur = 1;
  private ease: (t: number) => number = t => t;
  constructor(
    readonly id: number,
    public r: Rect,
  ) {}

  sample(now: number) {
    if (this.done) return;
    const t = (now - this.t0) / this.dur;
    const f = this.f;
    const g = this.t;
    if (t <= 0) {
      this.x = f.x; this.y = f.y; this.w = f.w; this.h = f.h; this.a = f.a;
      return;
    }
    if (t >= 1) {
      this.x = g.x; this.y = g.y; this.w = g.w; this.h = g.h; this.a = g.a;
      this.done = true;
      return;
    }
    const p = this.ease(t);
    const q = p < 0 ? 0 : p > 1 ? 1 : p;
    this.x = f.x + (g.x - f.x) * p;
    this.y = f.y + (g.y - f.y) * p;
    this.w = Math.max(0, f.w + (g.w - f.w) * p);
    this.h = Math.max(0, f.h + (g.h - f.h) * p);
    this.a = f.a + (g.a - f.a) * q;
  }

  go(from: Frame, to: Frame, now: number, m: Motion, delay = 0) {
    this.f = { ...from };
    this.t = { x: to.x, y: to.y, w: to.w, h: to.h, a: to.a };
    this.t0 = now + delay;
    this.dur = m.dur;
    this.ease = m.ease;
    this.done = false;
  }

  snap(r: Box) {
    this.x = r.x; this.y = r.y; this.w = r.w; this.h = r.h; this.a = 1;
    this.t = { x: r.x, y: r.y, w: r.w, h: r.h, a: 1 };
    this.done = true;
  }

  /** already heading to this target */
  headingTo(r: Box) {
    const t = this.t;
    return !this.done && t.x === r.x && t.y === r.y && t.w === r.w && t.h === r.h && t.a === 1;
  }
}

const cur = (it: Item): Frame => ({ x: it.x, y: it.y, w: it.w, h: it.h, a: it.a });
/** map a box from camera A's frame into camera B's frame */
export function camMap(A: Box, B: Box, r: Box): Box {
  const sx = B.w / A.w;
  const sy = B.h / A.h;
  return { x: B.x + (r.x - A.x) * sx, y: B.y + (r.y - A.y) * sy, w: r.w * sx, h: r.h * sy };
}

export interface EngineHost {
  sel: { has(id: number): boolean };
  hoverId: number | null;
  /** category filter: tiles of other categories are dimmed */
  filter: number | null;
  scanning: boolean;
  reduced: boolean;
  theme: Theme;
}

export interface PaintState {
  now: number;
  dpr: number;
  W: number;
  H: number;
  theme: Theme;
  sel: { has(id: number): boolean };
  hoverId: number | null;
  mouse: { x: number; y: number; in: boolean };
  scanning: boolean;
}

export interface Painter {
  draw(ctx: CanvasRenderingContext2D, list: readonly Item[], S: PaintState): void;
  /** forget caches that depend on the theme or DPR */
  reset(): void;
}

export interface ApplyOpts {
  /** camera move for zooms: A is the old frame of the region, B the new one */
  cam?: (rects: Rect[]) => { A: Box; B: Box } | null;
  /** existing tiles slide with the reflow curve after a short delay (deletes) */
  reflow?: boolean;
}

export interface EngineStats {
  rects: number;
  fps: number;
  frameMs: number;
  busy: boolean;
}

export class TreemapEngine {
  readonly items = new Map<number, Item>();
  drawList: Item[] = [];
  W = 0;
  H = 0;
  dpr = 1;
  readonly mouse = { x: -1, y: -1, in: false, moved: false };
  readonly stats: EngineStats = { rects: 0, fps: 0, frameMs: 0, busy: false };
  onStats: ((s: EngineStats) => void) | null = null;
  private ctx: CanvasRenderingContext2D;
  private raf = 0;
  private lastNow = 0;
  private frames: number[] = [];
  private statsAt = 0;

  constructor(
    readonly canvas: HTMLCanvasElement,
    private host: EngineHost,
    public painter: Painter,
    public motion: MotionSet,
    public outer: number,
  ) {
    const ctx = canvas.getContext('2d', { alpha: true });
    if (!ctx) throw new Error('canvas 2d unavailable');
    this.ctx = ctx;
  }

  /** returns true when the size or DPR changed (a new layout is needed) */
  resize(W: number, H: number, dpr: number): boolean {
    const changed = W !== this.W || H !== this.H || dpr !== this.dpr;
    if (!changed) return false;
    if (dpr !== this.dpr) this.painter.reset();
    this.W = W;
    this.H = H;
    this.dpr = dpr;
    this.canvas.width = Math.round(W * dpr);
    this.canvas.height = Math.round(H * dpr);
    this.canvas.style.width = W + 'px';
    this.canvas.style.height = H + 'px';
    this.kick();
    return true;
  }

  stageBox(): Box {
    const o = this.outer;
    return { x: o, y: o, w: this.W - 2 * o, h: this.H - 2 * o };
  }

  apply(L: Layout, kind: MotionKind, opt: ApplyOpts = {}) {
    const rects = L.rects;
    const now = performance.now();
    const M = this.motion;
    const m = M[kind] ?? M.update;
    const cam = opt.cam ? opt.cam(rects) : null;
    const instant = kind === 'instant' || this.host.reduced;
    const seen = new Set<number>();
    for (let i = 0; i < rects.length; i++) {
      const r = rects[i];
      seen.add(r.id);
      let it = this.items.get(r.id);
      if (instant) {
        if (!it) this.items.set(r.id, (it = new Item(r.id, r)));
        it.r = r;
        it.ord = i;
        it.rm = false;
        it.snap(r);
        continue;
      }
      if (it) {
        it.sample(now);
        const from = cur(it);
        it.rm = false;
        it.r = r;
        it.ord = i;
        if (!it.headingTo(r) && (it.x !== r.x || it.y !== r.y || it.w !== r.w || it.h !== r.h || it.a !== 1))
          it.go(from, { ...r, a: 1 }, now, opt.reflow ? M.reflow : m, opt.reflow ? M.reflowDelay : 0);
      } else {
        this.items.set(r.id, (it = new Item(r.id, r)));
        it.ord = i;
        let from: Frame;
        if (cam) from = { ...camMap(cam.B, cam.A, r), a: 0 };
        else if (kind === 'morph' || kind === 'mode') from = { ...r, a: 0 };
        else from = { ...M.appearFrom(r), a: 0 };
        it.go(from, { ...r, a: 1 }, now, cam || kind === 'morph' ? m : M.appear);
      }
    }
    for (const [id, it] of this.items) {
      if (seen.has(id)) continue;
      if (instant) {
        this.items.delete(id);
        continue;
      }
      if (it.rm && !cam) continue;
      it.sample(now);
      const from = cur(it);
      it.rm = true;
      if (cam) it.go(from, { ...camMap(cam.A, cam.B, from), a: 0 }, now, m);
      else it.go(from, { ...M.removeTo(from), a: 0 }, now, M.remove);
    }
    this.rebuild();
    this.stats.rects = rects.length;
    this.kick();
  }

  private rebuild() {
    this.drawList = [...this.items.values()].sort((a, b) => a.r.depth - b.r.depth || +a.rm - +b.rm || a.ord - b.ord);
  }

  kick() {
    if (!this.raf) this.raf = requestAnimationFrame(this.frame);
  }

  private frame = (now: number) => {
    this.raf = 0;
    const t0 = performance.now();
    const dt = Math.min(64, this.lastNow ? now - this.lastNow : 16);
    this.lastNow = now;
    const host = this.host;
    let busy = false;
    let purge = false;
    const k = 1 - Math.exp(-dt / 55);
    const kd = 1 - Math.exp(-dt / 140);
    const hover = host.hoverId;
    const f = host.filter;
    for (const it of this.drawList) {
      if (!it.done) {
        it.sample(now);
        if (!it.done) busy = true;
        else if (it.rm) purge = true;
      }
      const ht = it.id === hover ? 1 : 0;
      if (it.hv !== ht) {
        it.hv += (ht - it.hv) * (host.reduced ? 1 : k);
        if (Math.abs(it.hv - ht) < 0.004) it.hv = ht;
        else busy = true;
      }
      const r = it.r;
      const dtgt = f != null && r.kind !== Kind.Dir && (r.kind === Kind.Free || r.kind === Kind.Unscanned || r.cat !== f) ? 1 : 0;
      if (it.dm !== dtgt) {
        it.dm += (dtgt - it.dm) * (host.reduced ? 1 : kd);
        if (Math.abs(it.dm - dtgt) < 0.004) it.dm = dtgt;
        else busy = true;
      }
      it.va = it.a * (1 - it.dm * 0.78);
    }
    if (purge) {
      for (const it of this.drawList) if (it.rm && it.done) this.items.delete(it.id);
      this.drawList = this.drawList.filter(it => !(it.rm && it.done));
    }
    const ctx = this.ctx;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    this.painter.draw(ctx, this.drawList, {
      now,
      dpr: this.dpr,
      W: this.W,
      H: this.H,
      theme: host.theme,
      sel: host.sel,
      hoverId: hover,
      mouse: this.mouse,
      scanning: host.scanning,
    });
    const cost = performance.now() - t0;
    this.stats.frameMs = this.stats.frameMs ? this.stats.frameMs * 0.9 + cost * 0.1 : cost;
    this.frames.push(now);
    while (this.frames.length && this.frames[0] < now - 1000) this.frames.shift();
    const cont = (host.scanning && !host.reduced) || (this.mouse.in && this.mouse.moved);
    this.mouse.moved = false;
    const going = busy || cont;
    if (going) this.kick();
    else this.frames.length = 0;
    this.stats.fps = this.frames.length;
    if (this.stats.busy !== going || now - this.statsAt > 250) {
      this.stats.busy = going;
      this.statsAt = now;
      this.onStats?.(this.stats);
    }
  };

  hit(x: number, y: number): Item | null {
    const list = this.drawList;
    for (let i = list.length - 1; i >= 0; i--) {
      const it = list[i];
      if (it.rm || it.a < 0.4) continue;
      if (x >= it.x && x < it.x + it.w && y >= it.y && y < it.y + it.h) return it;
    }
    return null;
  }

  /** the tile next to `id` in an arrow-key direction, among the leaf tiles that `ok` accepts */
  neighbor(id: number | undefined, key: 'ArrowLeft' | 'ArrowRight' | 'ArrowUp' | 'ArrowDown', ok: (r: Rect) => boolean = () => true): Item | null {
    const [dx, dy] = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[key];
    const leaves = this.drawList.filter(it => !it.rm && it.r.kind !== Kind.Dir && it.r.kind !== Kind.Unscanned && ok(it.r) && it.w * it.h > 40);
    const c = id != null ? this.items.get(id) : undefined;
    if (!c || c.rm) return leaves.slice().sort((a, b) => b.w * b.h - a.w * a.h)[0] ?? null;
    let best: Item | null = null;
    let bs = Infinity;
    for (const it of leaves) {
      if (it === c) continue;
      let along: number;
      let perp: number;
      if (dx) {
        along = dx > 0 ? it.x - (c.x + c.w) : c.x - (it.x + it.w);
        perp = Math.max(0, Math.max(it.y, c.y) - Math.min(it.y + it.h, c.y + c.h));
      } else {
        along = dy > 0 ? it.y - (c.y + c.h) : c.y - (it.y + it.h);
        perp = Math.max(0, Math.max(it.x, c.x) - Math.min(it.x + it.w, c.x + c.w));
      }
      if (along < -1) continue;
      const cdist = dx ? Math.abs(it.y + it.h / 2 - (c.y + c.h / 2)) : Math.abs(it.x + it.w / 2 - (c.x + c.w / 2));
      const score = along + perp * 4 + cdist * 0.05;
      if (score < bs) {
        bs = score;
        best = it;
      }
    }
    return best;
  }

  destroy() {
    if (this.raf) cancelAnimationFrame(this.raf);
    this.raf = 0;
  }
}
