/**
 * Fluent painter for the treemap canvas.
 *
 * Rules from the design review:
 * - hover and selection only change fill, stroke and brightness, never position,
 *   size, corner radius or padding;
 * - a tile that cannot hold a whole line of text gets no label (the tooltip has it);
 * - folder depth stays readable: header bands, nested translucent fills that build up
 *   with depth, and an outline whose contrast steps down with depth.
 *
 * The work is split in two so that a big window stays at 60 fps: `scene()` paints the map
 * itself (it changes only with the layout, the theme or the category filter, and the
 * engine keeps it in a cached layer), `overlay()` paints what changes from frame to frame
 * (hover, selection, the pointer light, the moving stripes of the unscanned tile). Between
 * layouts the engine restores just the region that changed from the cache and runs
 * `overlay()` clipped to it.
 *
 * With a GPU batch, `scene()` hands every folder, header band and tile to it (one draw call
 * on a WebGL canvas under this one) and paints only text and textures here.
 */
import { Flag, Kind, Note } from '../backend/codec';
import { fmtBytes } from '../format';
import { t as tr, tn } from '../i18n.svelte';
import { mix, rgba } from '../theme';
import type { Box, Item, Painter, PaintState } from './engine';
import { NONE, type BoxBatch, type RGBA } from './gpu';

export const UI_FONT = '"Segoe UI Variable Text", "Segoe UI", "Microsoft YaHei UI", "PingFang SC", "Noto Sans SC", system-ui, sans-serif';
const F_REG = `400 12px ${UI_FONT}`;
const F_SEMI = `600 12px ${UI_FONT}`;

const LABEL_MIN_W = 44;
const LINE = 16;
/**
 * Corner radii of the tiles (files, closed folders) and of open folders. A live object rather
 * than constants so the design-system build (app/design) can turn them into sliders; the app
 * itself never changes them.
 */
export const paintStyle = {
  radius: 3,
  folderRadius: 4,
  /**
   * How folders are drawn. 'classic' is the shipped look (an open folder is a frame with a
   * header band, a closed one a tinted tile). 'band' is the 1.3.1 candidate (folder-look
   * thread, not decided yet): every folder is one frame, a band on top whose fill runs on
   * as a 1 px ring inside the outline and rounds into the white area's corners; content sits
   * `white` px in from the white area on all four sides; a closed folder shows its main
   * colour as a band along the bottom. Only the design system's canvas switches it for now.
   */
  folders: 'classic' as 'classic' | 'band',
  /** band look: the white margin round a folder's content, px */
  white: 4,
  /** band look: the header band's fill runs on round the sides and bottom, 1 px inside the outline */
  ring: true,
  /** band look: height of a closed folder's colour band above its outline, px */
  foot: 5,
  /** band look: the colour band mixed further into the ground, 0 = the file colour .. 3 = the 1.3.0 closed-folder tint */
  tone: 0,
  /** band look: at the seams where the side ring meets the colour band, the colour runs up the ring, fading out over this many px (0 = a hard edge) */
  fade: 0,
};
/** band look: the header band's height, px */
export const BAND = 21;
/** band look: the least white area that still reads as one, px */
const BAND_MIN_WHITE = 6;
/** band look: how much further each tone step mixes the file colour into the ground */
const TONE_MIX = [0, 0.25, 0.42, 0.58];
/**
 * The layout the band look needs (fluentLayout): the core insets content by pad plus half the
 * 2 px gap, so pad = white (+ 1 for the ring) puts it `white` px inside the white area, and
 * header = band - 1 + white does the same below the band. A closed folder is at least the
 * band, a little white area and the colour band tall.
 */
export function bandLayout() {
  const ring = paintStyle.ring ? 1 : 0;
  return { pad: paintStyle.white + ring, header: BAND - 1 + paintStyle.white, minFolderH: BAND + BAND_MIN_WHITE + paintStyle.foot + 1 };
}
/** radius of the pointer light, CSS px */
const LIGHT_R = 90;

/** a rect snapped to device pixels, so edges stay crisp at any DPR */
function snapBox(it: Item, dpr: number): Box {
  const x0 = Math.round(it.x * dpr) / dpr;
  const y0 = Math.round(it.y * dpr) / dpr;
  return { x: x0, y: y0, w: Math.round((it.x + it.w) * dpr) / dpr - x0, h: Math.round((it.y + it.h) * dpr) / dpr - y0 };
}

class TextFit {
  private w = new Map<string, number>();
  measure(ctx: CanvasRenderingContext2D, s: string) {
    const k = ctx.font + '\u0001' + s;
    let v = this.w.get(k);
    if (v === undefined) {
      v = ctx.measureText(s).width;
      if (this.w.size > 40000) this.w.clear();
      this.w.set(k, v);
    }
    return v;
  }
  /** the longest prefix (with an ellipsis) that fits, or '' if not even 2 characters do */
  fit(ctx: CanvasRenderingContext2D, s: string, maxW: number) {
    if (maxW < 12 || !s) return '';
    const full = this.measure(ctx, s);
    if (full <= maxW) return s;
    let n = Math.min(s.length - 1, Math.floor((s.length * maxW) / full));
    while (n >= 2) {
      const t = s.slice(0, n) + '…';
      if (this.measure(ctx, t) <= maxW) return t;
      n--;
    }
    return '';
  }
  clear() {
    this.w.clear();
  }
}

export const textFit = new TextFit();

type Pat = { pat: CanvasPattern; size: number };

export class FluentPainter implements Painter {
  readonly light = LIGHT_R;
  private pats = new Map<string, Pat>();
  private batches = new Map<string, Path2D>();
  /** folder outline per depth, per scheme */
  private strokes = new Map<string, string[]>();
  /** band look: a closed folder's band colour per file colour and tone step */
  private bandTones = new Map<string, string>();
  /** band look: the fade above a colour band, per context, colour and length (drawn translated) */
  private fades = new WeakMap<CanvasRenderingContext2D, Map<string, CanvasGradient>>();

  reset() {
    this.pats.clear();
    this.strokes.clear();
    this.bandTones.clear();
    this.fades = new WeakMap();
    textFit.clear();
  }

  private bandTone(fill: string, dark: boolean) {
    const step = Math.max(0, Math.min(TONE_MIX.length - 1, Math.round(paintStyle.tone)));
    if (!step) return fill;
    const k = fill + '|' + step + (dark ? 'd' : 'l');
    let c = this.bandTones.get(k);
    if (!c) {
      c = mix(fill, dark ? '#202020' : '#ffffff', TONE_MIX[step]);
      this.bandTones.set(k, c);
    }
    return c;
  }

  private fade(ctx: CanvasRenderingContext2D, colour: string, len: number) {
    let m = this.fades.get(ctx);
    if (!m) this.fades.set(ctx, (m = new Map()));
    const k = colour + '|' + len;
    let g = m.get(k);
    if (!g) {
      g = ctx.createLinearGradient(0, 0, 0, len);
      g.addColorStop(0, rgba(colour, 0));
      g.addColorStop(1, colour);
      m.set(k, g);
    }
    return g;
  }

  private folderStroke(dark: boolean, depth: number) {
    const k = dark ? 'dark' : 'light';
    let a = this.strokes.get(k);
    if (!a) {
      a = [];
      for (let d = 0; d < 32; d++) a.push(dark ? `rgba(255,255,255,${Math.max(0.05, 0.13 - d * 0.012)})` : `rgba(0,0,0,${Math.max(0.055, 0.15 - d * 0.016)})`);
      this.strokes.set(k, a);
    }
    return a[Math.min(depth, 31)];
  }

  private pattern(ctx: CanvasRenderingContext2D, key: string, size: number, dpr: number, paint: (g: CanvasRenderingContext2D, s: number) => void): Pat {
    const k = key + '|' + dpr;
    let p = this.pats.get(k);
    if (p) return p;
    const c = document.createElement('canvas');
    c.width = c.height = Math.round(size * dpr);
    const g = c.getContext('2d')!;
    g.scale(dpr, dpr);
    paint(g, size);
    p = { pat: ctx.createPattern(c, 'repeat')!, size };
    this.pats.set(k, p);
    return p;
  }

  private usePattern(p: Pat, dpr: number, ox = 0, oy = 0) {
    p.pat.setTransform(new DOMMatrix().translate(ox, oy).scale(1 / dpr));
    return p.pat;
  }

  scene(ctx: CanvasRenderingContext2D, list: readonly Item[], S: PaintState, gpu: BoxBatch | null = null) {
    const th = S.theme;
    const t = th.tok;
    const dark = th.scheme === 'dark';
    const dpr = S.dpr;
    ctx.clearRect(0, 0, S.W, S.H);
    ctx.textBaseline = 'alphabetic';
    ctx.globalAlpha = 1;
    // the band look is drawn in 2D only for now (design canvas); its folders' translucent
    // fills would cover GPU tiles under this canvas, so the whole scene stays here
    const bandLook = paintStyle.folders === 'band';
    if (bandLook) gpu = null;

    // a closed folder is washed with the category that fills it most (or its age band),
    // lighter than any file, and keeps its outline and neutral ink so it never reads as a file
    const tone = S.coloring;
    const n = tone.fill.length;
    const leafFill = (it: Item) => {
      const c = tone.key(it.r);
      const ok = c >= 0 && c < n;
      if (it.r.kind === Kind.DirLeaf) return ok ? tone.tint[c] : t['tm-leafdir'];
      return ok ? tone.fill[c] : t['tm-leafdir'];
    };
    const leafInk = (it: Item) => {
      const c = tone.key(it.r);
      return c >= 0 && c < n && it.r.kind !== Kind.DirLeaf ? tone.ink[c] : t['tm-leafdir-ink'];
    };
    const outlined = (it: Item) => {
      if (it.r.kind === Kind.DirLeaf) return true;
      const c = tone.key(it.r);
      return c < 0 || c >= n;
    };
    const box = (it: Item) => snapBox(it, dpr);
    /** dots for a group, hatching for a locked file, a dashed edge when not to scale */
    const overlaid = (r: Item['r']) => r.kind === Kind.Group || r.flags & (Flag.NoAccess | Flag.InUse) || (r.note ?? 0) & Note.NotToScale;

    /* ---------- pass 1: open folders (containers), shallow to deep ---------- */
    const notes: Item[] = [];
    for (const it of list) {
      const r = it.r;
      const closedBand = bandLook && r.kind === Kind.DirLeaf;
      if ((r.kind !== Kind.Dir && !closedBand) || it.va <= 0.004 || it.w < 2 || it.h < 2) continue;
      const b = box(it);
      ctx.globalAlpha = it.va;
      if (bandLook) {
        const c = closedBand ? tone.key(r) : -1;
        this.folderBand(ctx, it, b, S, c >= 0 && c < n ? this.bandTone(tone.fill[c], dark) : null);
        if (closedBand && (r.note ?? 0) & Note.NotToScale) notes.push(it);
        continue;
      }
      if (gpu) {
        // the outline's outer edge lies half a pixel outside the 2D path, so its radius is half a pixel more
        gpu.push(b.x * dpr, b.y * dpr, b.w * dpr, b.h * dpr, (paintStyle.folderRadius + 0.5) * dpr, gpu.color(t['tm-folder'], it.va), gpu.color(this.folderStroke(dark, r.depth), it.va), dpr);
      } else {
        ctx.beginPath();
        ctx.roundRect(b.x + 0.5, b.y + 0.5, b.w - 1, b.h - 1, paintStyle.folderRadius);
        ctx.fillStyle = t['tm-folder'];
        ctx.fill();
        ctx.lineWidth = 1;
        ctx.strokeStyle = this.folderStroke(dark, r.depth);
        ctx.stroke();
      }
      if (r.header && b.h > r.header + 2) {
        if (gpu) {
          // rounded on top only: a box that runs on past its bottom corners, cut at the band's bottom
          gpu.push((b.x + 1) * dpr, (b.y + 1) * dpr, (b.w - 2) * dpr, (r.header - 1 + 2 * paintStyle.folderRadius) * dpr, (paintStyle.folderRadius - 1) * dpr, gpu.color(t['tm-folder-head'], it.va), null, 0, (b.y + r.header) * dpr);
        } else {
          ctx.beginPath();
          ctx.roundRect(b.x + 1, b.y + 1, b.w - 2, r.header - 1, [paintStyle.folderRadius - 1, paintStyle.folderRadius - 1, 0, 0]);
          ctx.fillStyle = t['tm-folder-head'];
          ctx.fill();
        }
        this.header(ctx, it, b, S);
      }
    }

    /* ---------- pass 2: tiles; steady ones batched by colour ---------- */
    const batches = this.batches;
    batches.clear();
    const outlines = new Path2D();
    let hasOutlines = false;
    const loose: Item[] = [];
    const overlays: Item[] = notes;
    for (const it of list) {
      const r = it.r;
      if (r.kind === Kind.Dir || (bandLook && r.kind === Kind.DirLeaf) || r.kind === Kind.Unscanned || it.va <= 0.004 || it.w < 0.6 || it.h < 0.6) continue;
      if (r.kind === Kind.Free || it.va < 0.995 || r.flags & Flag.Cloud) {
        loose.push(it);
        continue;
      }
      const b = box(it);
      const col = leafFill(it);
      if (gpu) {
        this.gpuTile(gpu, b, dpr, gpu.color(col), outlined(it) ? gpu.color(t['tm-leafdir-stroke']) : null);
        if (overlaid(r)) overlays.push(it);
        continue;
      }
      let p = batches.get(col);
      if (!p) batches.set(col, (p = new Path2D()));
      if (b.w < 5 || b.h < 5) p.rect(b.x, b.y, b.w, b.h);
      else p.roundRect(b.x, b.y, b.w, b.h, paintStyle.radius);
      if (outlined(it) && b.w > 3 && b.h > 3) {
        // a closed folder: outlined like the open ones, so it never reads as a file type
        outlines.roundRect(b.x + 0.5, b.y + 0.5, b.w - 1, b.h - 1, Math.min(paintStyle.radius, b.w / 2, b.h / 2));
        hasOutlines = true;
      }
      if (overlaid(r)) overlays.push(it);
    }
    ctx.globalAlpha = 1;
    for (const [col, p] of batches) {
      ctx.fillStyle = col;
      ctx.fill(p);
    }
    if (hasOutlines) {
      ctx.lineWidth = 1;
      ctx.strokeStyle = t['tm-leafdir-stroke'];
      ctx.stroke(outlines);
    }
    for (const it of loose) {
      const r = it.r;
      const b = box(it);
      if (r.kind === Kind.Free) {
        this.special(ctx, it, b, S);
        continue;
      }
      ctx.globalAlpha = it.va * (r.flags & Flag.Cloud ? 0.55 : 1);
      if (gpu) {
        const a = ctx.globalAlpha;
        this.gpuTile(gpu, b, dpr, gpu.color(leafFill(it), a), outlined(it) ? gpu.color(t['tm-leafdir-stroke'], a) : null);
        if (overlaid(r)) overlays.push(it);
        continue;
      }
      ctx.beginPath();
      if (b.w < 5 || b.h < 5) ctx.rect(b.x, b.y, b.w, b.h);
      else ctx.roundRect(b.x, b.y, b.w, b.h, paintStyle.radius);
      ctx.fillStyle = leafFill(it);
      ctx.fill();
      if (outlined(it) && b.w > 3 && b.h > 3) {
        ctx.beginPath();
        ctx.roundRect(b.x + 0.5, b.y + 0.5, b.w - 1, b.h - 1, Math.min(paintStyle.radius, b.w / 2, b.h / 2));
        ctx.lineWidth = 1;
        ctx.strokeStyle = t['tm-leafdir-stroke'];
        ctx.stroke();
      }
      if (overlaid(r)) overlays.push(it);
    }

    /* ---------- pass 3: overlays (merged items, locked files, not to scale) ---------- */
    for (const it of overlays) {
      const r = it.r;
      const b = box(it);
      ctx.globalAlpha = it.va;
      ctx.beginPath();
      ctx.roundRect(b.x, b.y, b.w, b.h, Math.min(paintStyle.radius, b.w / 2, b.h / 2));
      if (r.kind === Kind.Group) {
        const p = this.pattern(ctx, 'dots' + th.scheme, 6, dpr, g => {
          g.fillStyle = t['tm-dots'];
          g.fillRect(2, 2, 1.5, 1.5);
        });
        ctx.fillStyle = this.usePattern(p, dpr);
        ctx.fill();
      } else if (r.flags & (Flag.NoAccess | Flag.InUse) && !(bandLook && r.kind === Kind.DirLeaf)) {
        const p = this.pattern(ctx, 'hatch' + th.scheme, 8, dpr, stripes(t['tm-hatch'], 8, 2));
        ctx.fillStyle = this.usePattern(p, dpr);
        ctx.fill();
      }
      if ((r.note ?? 0) & Note.NotToScale && b.w > 3 && b.h > 3) {
        // a level's group, or the one item it left over, drawn smaller or bigger than its
        // size: a dashed edge says its area is not a share of the map (docs/ipc.md rule 9)
        ctx.beginPath();
        ctx.roundRect(b.x + 0.5, b.y + 0.5, b.w - 1, b.h - 1, Math.min(paintStyle.radius, b.w / 2, b.h / 2));
        ctx.setLineDash([4, 3]);
        ctx.lineWidth = 1;
        ctx.globalAlpha = it.va * 0.7;
        ctx.strokeStyle = leafInk(it);
        ctx.stroke();
        ctx.setLineDash([]);
      }
    }

    /* ---------- pass 4: labels ---------- */
    for (const it of list) {
      const r = it.r;
      if (r.kind === Kind.Dir || (bandLook && r.kind === Kind.DirLeaf) || r.kind === Kind.Free || r.kind === Kind.Unscanned || it.va < 0.25) continue;
      if (it.w < LABEL_MIN_W || it.h < 20) continue;
      const name = r.kind === Kind.Group ? tn('smallItems', r.count) : r.name;
      if (!name) continue;
      const b = box(it);
      ctx.globalAlpha = it.va * (r.flags & Flag.Cloud ? 0.8 : 1);
      const ink = leafInk(it);
      this.label(ctx, name, fmtBytes(r.size), b, ink, ink);
    }

    ctx.globalAlpha = 1;
  }

  /** a tile on the GPU, as pass 2 paints it: the fill, then a closed folder's outline over it */
  private gpuTile(gpu: BoxBatch, b: Box, dpr: number, fill: RGBA, stroke: RGBA | null) {
    gpu.push(b.x * dpr, b.y * dpr, b.w * dpr, b.h * dpr, b.w < 5 || b.h < 5 ? 0 : paintStyle.radius * dpr, fill);
    if (stroke && b.w > 3 && b.h > 3) gpu.push(b.x * dpr, b.y * dpr, b.w * dpr, b.h * dpr, (Math.min(paintStyle.radius, b.w / 2, b.h / 2) + 0.5) * dpr, NONE, stroke, dpr);
  }

  inOverlay(it: Item) {
    return it.r.kind === Kind.Unscanned;
  }

  overlay(ctx: CanvasRenderingContext2D, list: readonly Item[], marked: readonly Item[], S: PaintState) {
    const t = S.theme.tok;
    const dark = S.theme.scheme === 'dark';
    const dpr = S.dpr;
    const box = (it: Item) => snapBox(it, dpr);

    /* ---------- the unscanned tile: its stripes move while the scan runs ---------- */
    for (const it of list) {
      if (it.r.kind === Kind.Unscanned && it.va > 0.004 && it.w >= 0.6 && it.h >= 0.6) this.special(ctx, it, box(it), S);
    }

    /* ---------- pass 5: hover and selection (paint only, geometry untouched) ---------- */
    for (const it of marked) {
      const r = it.r;
      const b = box(it);
      const folder = r.kind === Kind.Dir || (paintStyle.folders === 'band' && r.kind === Kind.DirLeaf);
      if (it.hv > 0.01) {
        ctx.globalAlpha = it.va * it.hv;
        ctx.beginPath();
        if (folder) {
          ctx.roundRect(b.x + 0.5, b.y + 0.5, b.w - 1, b.h - 1, paintStyle.folderRadius);
          ctx.lineWidth = 1;
          ctx.strokeStyle = t['ctl-strong'];
          ctx.stroke();
        } else {
          ctx.roundRect(b.x, b.y, b.w, b.h, Math.min(paintStyle.radius, b.w / 2, b.h / 2));
          ctx.fillStyle = t['tm-hover'];
          ctx.fill();
        }
      }
      if (S.sel.has(it.id) && b.w > 3 && b.h > 3) {
        const rad = folder ? paintStyle.folderRadius : paintStyle.radius;
        ctx.globalAlpha = it.va;
        ctx.beginPath();
        ctx.roundRect(b.x + 1, b.y + 1, b.w - 2, b.h - 2, Math.max(0, Math.min(rad, b.w / 2 - 1, b.h / 2 - 1)));
        ctx.lineWidth = 2;
        ctx.strokeStyle = t.accent;
        ctx.stroke();
        if (b.w > 8 && b.h > 8) {
          ctx.beginPath();
          ctx.roundRect(b.x + 2.5, b.y + 2.5, b.w - 5, b.h - 5, Math.max(0, Math.min(rad - 1.5, b.w / 2 - 3, b.h / 2 - 3)));
          ctx.lineWidth = 1;
          ctx.strokeStyle = dark ? 'rgba(0,0,0,0.55)' : 'rgba(255,255,255,0.9)';
          ctx.stroke();
        }
      }
    }

    /* ---------- pass 6: pointer light on nearby tile edges ---------- */
    if (S.mouse.in) {
      const mx = S.mouse.x;
      const my = S.mouse.y;
      const R = LIGHT_R;
      const g = ctx.createRadialGradient(mx, my, 0, mx, my, R);
      g.addColorStop(0, t['tm-reveal']);
      g.addColorStop(1, 'rgba(0,0,0,0)');
      ctx.globalAlpha = 1;
      ctx.beginPath();
      for (const it of list) {
        if (it.rm || it.r.kind === Kind.Dir || it.w < 4 || it.h < 4) continue;
        if (it.x > mx + R || it.x + it.w < mx - R || it.y > my + R || it.y + it.h < my - R) continue;
        const b = box(it);
        ctx.rect(b.x + 0.5, b.y + 0.5, b.w - 1, b.h - 1);
      }
      ctx.lineWidth = 1;
      ctx.strokeStyle = g;
      ctx.stroke();
    }
    ctx.globalAlpha = 1;
  }

  /**
   * A folder in the band look (paintStyle.folders === 'band'). One frame for every folder:
   * the band's fill runs down into the top corners of the white area (and, with the ring, on
   * round the sides and bottom 1 px inside the outline), so the white area's four corners
   * match. A closed folder with a colour shows it as a band along the bottom, inside the
   * outline, which keeps its grey line all the way round.
   */
  private folderBand(ctx: CanvasRenderingContext2D, it: Item, b: Box, S: PaintState, colour: string | null) {
    const r = it.r;
    const t = S.theme.tok;
    const dark = S.theme.scheme === 'dark';
    const closed = r.kind === Kind.DirLeaf;
    const R = paintStyle.folderRadius;
    const ring = paintStyle.ring ? 1 : 0;
    const E = closed && colour ? paintStyle.foot + 1 : 1 + ring;
    const yW = b.y + BAND;
    const yWb = b.y + b.h - E;
    const wh = yWb - yW;
    const wx = b.x + 1 + ring;
    const ww = b.w - 2 - 2 * ring;
    const wr = Math.max(0, Math.min(R - 0.5, ww / 2, wh / 2));
    const white = new Path2D();
    if (wh > 0 && ww > 0) white.roundRect(wx, yW, ww, wh, wr);
    const outline = new Path2D();
    outline.roundRect(b.x + 0.5, b.y + 0.5, b.w - 1, b.h - 1, Math.max(0, Math.min(R, b.w / 2 - 0.5, b.h / 2 - 0.5)));
    ctx.fillStyle = t['tm-folder'];
    ctx.fill(outline);
    const hasBand = b.h > BAND + 2 && b.w > 8;
    if (hasBand) {
      // the band: the folder above the white area's middle that is not the white area (with
      // the ring, all of it that is not). The rectangle runs 2 px past the clip so the clip's
      // soft edge never crosses the white area: no faint line through its middle.
      ctx.save();
      ctx.clip(outline);
      const yMid = ring ? b.y + b.h + 2 : wh > 0 ? yW + wh / 2 : b.y + b.h;
      ctx.beginPath();
      ctx.rect(b.x, b.y, b.w, yMid - b.y);
      ctx.clip();
      const p = new Path2D();
      p.rect(b.x, b.y, b.w, yMid + 2 - b.y);
      p.addPath(white);
      ctx.fillStyle = t['tm-folder-head'];
      ctx.fill(p, 'evenodd');
      ctx.restore();
    }
    if (closed && colour && wh > 0) this.footBand(ctx, b, colour, white, ring ? Math.max(yW + wh / 2, yWb - wr) : yW + wh / 2, yW);
    ctx.lineWidth = 1;
    ctx.strokeStyle = this.folderStroke(dark, r.depth);
    ctx.stroke(outline);
    if (closed && wh > 0 && r.flags & (Flag.NoAccess | Flag.InUse)) {
      const p = this.pattern(ctx, 'hatchsoft' + S.theme.scheme, 8, S.dpr, stripes(dark ? 'rgba(255,255,255,0.07)' : 'rgba(0,0,0,0.06)', 8, 2));
      ctx.fillStyle = this.usePattern(p, S.dpr);
      ctx.fill(white);
    }
    if (!hasBand || (!closed && !r.header)) return;
    this.headerBand(ctx, it, b, S, closed ? wh : 0, yW);
  }

  /**
   * Band look: a closed folder's colour band, the band's mirror below the white area (inside
   * the outline, from `yCut`: where the white area's bottom corners start to round). With
   * paintStyle.fade the colour also runs up the ring beside the white area from that seam,
   * fading out; the white area itself is never tinted.
   */
  private footBand(ctx: CanvasRenderingContext2D, b: Box, colour: string, white: Path2D, yCut: number, yW: number) {
    const R = paintStyle.folderRadius;
    const inner = new Path2D();
    inner.roundRect(b.x + 1, b.y + 1, b.w - 2, b.h - 2, Math.max(0, Math.min(R - 0.5, (b.w - 2) / 2, (b.h - 2) / 2)));
    ctx.save();
    ctx.clip(inner);
    const G = Math.min(paintStyle.fade, yCut - yW);
    if (G > 0) {
      // only beside the white area: the ring's strip from yCut - G to the seam. The gradient is
      // cached by length, so it is drawn translated, and the path with it.
      const y0 = yCut - G;
      ctx.save();
      ctx.beginPath();
      ctx.rect(b.x, y0, b.w, G);
      ctx.clip();
      ctx.translate(0, y0);
      const f = new Path2D();
      f.rect(b.x, -2, b.w, G + 4);
      f.addPath(white, { f: -y0 });
      ctx.fillStyle = this.fade(ctx, colour, G);
      ctx.fill(f, 'evenodd');
      ctx.restore();
    }
    ctx.beginPath();
    ctx.rect(b.x, yCut, b.w, b.y + b.h - yCut);
    ctx.clip();
    const q = new Path2D();
    q.rect(b.x, yCut - 2, b.w, b.y + b.h - yCut + 2);
    q.addPath(white);
    ctx.fillStyle = colour;
    ctx.fill(q, 'evenodd');
    ctx.restore();
  }

  /** band look: a folder's name (and size) on the band; a closed folder's size goes into its white area when the band has no room */
  private headerBand(ctx: CanvasRenderingContext2D, it: Item, b: Box, S: PaintState, roomBelow: number, yW: number) {
    const r = it.r;
    const t = S.theme.tok;
    if (b.w < 30 || !r.name) return;
    let x = b.x + 8;
    const base = b.y + Math.round(BAND / 2 + 4.5);
    if (r.flags & (Flag.Protected | Flag.NoAccess) && b.w > 80) {
      lockGlyph(ctx, x, b.y + BAND / 2 - 5.5, t['text-3']);
      x += 15;
    }
    ctx.font = F_REG;
    const sz = fmtBytes(r.size);
    const sw = textFit.measure(ctx, sz);
    const showSize = b.w > sw + 96;
    ctx.font = F_SEMI;
    const nm = textFit.fit(ctx, r.name, b.x + b.w - x - 8 - (showSize ? sw + 10 : 0));
    if (nm) {
      ctx.fillStyle = t['tm-folder-text'];
      ctx.fillText(nm, x, base);
    }
    ctx.font = F_REG;
    if (showSize) {
      ctx.fillStyle = t['text-3'];
      ctx.fillText(sz, b.x + b.w - sw - 8, base);
    } else if (roomBelow >= 18) {
      const s = textFit.fit(ctx, sz, b.w - 16);
      if (s) {
        ctx.fillStyle = t['text-2'];
        ctx.fillText(s, b.x + 8, yW + 14);
      }
    }
  }

  private header(ctx: CanvasRenderingContext2D, it: Item, b: { x: number; y: number; w: number; h: number }, S: PaintState) {
    const r = it.r;
    const t = S.theme.tok;
    if (b.w < 30 || !r.name) return;
    let x = b.x + 8;
    const base = b.y + Math.round(r.header / 2 + 4.5);
    if (r.flags & (Flag.Protected | Flag.NoAccess) && b.w > 80) {
      lockGlyph(ctx, x, b.y + r.header / 2 - 5.5, t['text-3']);
      x += 15;
    }
    ctx.font = F_REG;
    const sz = fmtBytes(r.size);
    const sw = textFit.measure(ctx, sz);
    const showSize = b.w > sw + 96;
    ctx.font = F_SEMI;
    const nm = textFit.fit(ctx, r.name, b.x + b.w - x - 8 - (showSize ? sw + 10 : 0));
    if (nm) {
      ctx.fillStyle = t['tm-folder-text'];
      ctx.fillText(nm, x, base);
    }
    if (showSize) {
      ctx.font = F_REG;
      ctx.fillStyle = t['text-3'];
      ctx.fillText(sz, b.x + b.w - sw - 8, base);
    }
  }

  private label(ctx: CanvasRenderingContext2D, name: string, size: string, b: { x: number; y: number; w: number; h: number }, ink: string, ink2: string) {
    const px = 7;
    ctx.font = F_REG;
    const nm = textFit.fit(ctx, name, b.w - px * 2);
    if (!nm) return;
    ctx.fillStyle = ink;
    ctx.fillText(nm, b.x + px, b.y + 16);
    if (b.h >= 16 + LINE + 6) {
      ctx.font = F_SEMI;
      const s = textFit.fit(ctx, size, b.w - px * 2);
      if (s) {
        ctx.fillStyle = ink2;
        ctx.fillText(s, b.x + px, b.y + 16 + LINE);
      }
    }
  }

  private special(ctx: CanvasRenderingContext2D, it: Item, b: { x: number; y: number; w: number; h: number }, S: PaintState) {
    const t = S.theme.tok;
    const free = it.r.kind === Kind.Free;
    ctx.globalAlpha = it.va;
    ctx.beginPath();
    ctx.roundRect(b.x + 0.5, b.y + 0.5, b.w - 1, b.h - 1, paintStyle.folderRadius);
    if (free) {
      ctx.fillStyle = rgba(t.accent, S.theme.scheme === 'dark' ? 0.08 : 0.06);
      ctx.fill();
    } else {
      const p = this.pattern(ctx, 'unscanned' + S.theme.scheme, 12, S.dpr, stripes(t['tm-stripe'], 12, 4));
      ctx.fillStyle = this.usePattern(p, S.dpr, S.scanning ? (S.now / 40) % 12 : 0, 0);
      ctx.fill();
    }
    ctx.setLineDash([4, 3]);
    ctx.lineWidth = 1;
    ctx.strokeStyle = free ? t.accent : t['ctl-strong'];
    ctx.stroke();
    ctx.setLineDash([]);
    if (b.w >= 60 && b.h >= 22) {
      const name = free ? tr('free') : tr('unscanned');
      ctx.font = F_SEMI;
      const nm = textFit.fit(ctx, name, b.w - 16);
      if (!nm) return;
      ctx.fillStyle = free ? t.accent : t['text-2'];
      ctx.fillText(nm, b.x + 8, b.y + 17);
      if (b.h >= 40) {
        ctx.font = F_REG;
        ctx.fillStyle = t['text-3'];
        ctx.fillText(textFit.fit(ctx, fmtBytes(it.r.size), b.w - 16), b.x + 8, b.y + 17 + LINE);
      }
    }
  }
}

function stripes(color: string, size: number, width: number) {
  return (g: CanvasRenderingContext2D, s: number) => {
    g.strokeStyle = color;
    g.lineWidth = width;
    g.beginPath();
    for (let i = -s; i <= s * 2; i += size) {
      g.moveTo(i, s);
      g.lineTo(i + s, 0);
    }
    g.stroke();
  };
}

function lockGlyph(ctx: CanvasRenderingContext2D, x: number, y: number, color: string) {
  ctx.save();
  ctx.strokeStyle = color;
  ctx.fillStyle = color;
  ctx.lineWidth = 1.2;
  ctx.beginPath();
  ctx.roundRect(x + 1, y + 5, 9, 6.5, 1.2);
  ctx.fill();
  ctx.beginPath();
  ctx.arc(x + 5.5, y + 5, 2.8, Math.PI, 0);
  ctx.stroke();
  ctx.restore();
}
