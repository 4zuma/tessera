/**
 * Squarified treemap with the rules from docs/ipc.md ("Layout rules the core
 * implements"). This is the reference the Rust layout module follows.
 */
import { CAT_NONE, FREE, Flag, Kind, NONE, UNSCANNED, groupId, type Rect } from '../codec';
import type { LayoutStyle, SizeMode } from '../types';
import type { Drive, MNode } from './drive';

interface Item {
  s: number;
  rk: number;
  n?: MNode;
  special?: 'free' | 'unscanned';
  group?: { count: number; cat: number };
}

export interface LayoutInput {
  drive: Drive;
  view: number;
  width: number;
  height: number;
  sizeMode: SizeMode;
  style: LayoutStyle;
  /** pseudo tiles at the volume root */
  free: number | null;
  unscanned: number | null;
}

export const sizeOf = (n: MNode, mode: SizeMode) => (mode === 'disk' ? n.dT : n.dL);

export function visibleKids(drive: Drive, n: MNode, mode: SizeMode): MNode[] {
  const out: MNode[] = [];
  if (!n.kids) return out;
  for (const k of n.kids) {
    const m = drive.nodes[k];
    if (m.seen && sizeOf(m, mode) > 0) out.push(m);
  }
  return out;
}

export function flagsOf(drive: Drive, n: MNode): number {
  let f = 0;
  if (n.lock === 'os' || n.id === 0) f |= Flag.Protected;
  if (n.lock === 'acl') f |= Flag.NoAccess;
  if (n.lock === 'sys') f |= Flag.InUse;
  if (n.cloud) f |= Flag.Cloud;
  if (n.id === drive.binId || n.id === drive.binUserId) f |= Flag.RecycleBin;
  return f;
}

export function squarify<T extends { s: number }>(items: T[], x: number, y: number, w: number, h: number, out: [T, number, number, number, number][]) {
  let total = 0;
  for (const it of items) total += it.s;
  if (total <= 0 || w <= 0 || h <= 0) return;
  const scale = (w * h) / total;
  let i = 0;
  while (i < items.length) {
    const short = Math.min(w, h);
    const s2 = short * short;
    let sum = items[i].s * scale;
    let amax = sum;
    let amin = sum;
    let best = Math.max((s2 * amax) / (sum * sum), (sum * sum) / (s2 * amin));
    let j = i + 1;
    while (j < items.length) {
      const a = items[j].s * scale;
      const ns = sum + a;
      const nmin = Math.min(amin, a);
      const nmax = Math.max(amax, a);
      const wr = Math.max((s2 * nmax) / (ns * ns), (ns * ns) / (s2 * nmin));
      if (wr > best) break;
      sum = ns;
      amin = nmin;
      amax = nmax;
      best = wr;
      j++;
    }
    if (w >= h) {
      const cw = sum / h;
      let yy = y;
      for (let k = i; k < j; k++) {
        const rh = (items[k].s * scale) / cw;
        out.push([items[k], x, yy, cw, rh]);
        yy += rh;
      }
      x += cw;
      w -= cw;
    } else {
      const rh = sum / w;
      let xx = x;
      for (let k = i; k < j; k++) {
        const rw = (items[k].s * scale) / rh;
        out.push([items[k], xx, y, rw, rh]);
        xx += rw;
      }
      y += rh;
      h -= rh;
    }
    i = j;
  }
}

/** sort key with 25 % hysteresis, so a growing scan does not reshuffle siblings */
function rank(c: Item): number {
  if (c.special === 'unscanned') return Infinity;
  if (c.special === 'free') return -1;
  const n = c.n;
  if (!n) return c.s;
  if (!n.rk || Math.abs(c.s - n.rk) > n.rk * 0.25) n.rk = c.s;
  return n.rk;
}

export function layoutDrive(inp: LayoutInput): { rects: Rect[]; bytesPerPx: number } {
  const { drive, style: o, sizeMode } = inp;
  const N = drive.nodes;
  const view = N[inp.view];
  const out: Rect[] = [];
  const kids: Item[] = visibleKids(drive, view, sizeMode).map(n => ({ n, s: sizeOf(n, sizeMode), rk: 0 }));
  if (inp.view === 0) {
    if (inp.unscanned != null && inp.unscanned > 64 * 1024 * 1024) kids.push({ special: 'unscanned', s: inp.unscanned, rk: 0 });
    if (inp.free != null && inp.free > 0) kids.push({ special: 'free', s: inp.free, rk: 0 });
  }
  const total = kids.reduce((a, b) => a + b.s, 0);
  const W = inp.width - 2 * o.outer;
  const H = inp.height - 2 * o.outer;
  // a folder opens only above both thresholds (rule 5)
  const openArea = Math.max(o.minArea * o.recurse, o.openShare * W * H);
  place(view, kids, o.outer, o.outer, W, H, 1, NONE);
  return { rects: out, bytesPerPx: total / Math.max(1, W * H) };

  function place(parent: MNode, items: Item[], x: number, y: number, w: number, h: number, depth: number, pid: number) {
    if (w < 1 || h < 1 || !items.length) return;
    for (const c of items) c.rk = rank(c);
    items.sort((a, b) => b.rk - a.rk);
    let total = 0;
    for (const c of items) total += c.s;
    const k = (w * h) / total;
    const list: Item[] = [];
    let rest = 0;
    let restN = 0;
    const votes = new Map<number, number>();
    for (const c of items) {
      if (c.special || (c.s * k >= o.minArea && list.length < o.maxPerDir)) list.push(c);
      else {
        rest += c.s;
        restN++;
        const ct = c.n!.kids ? CAT_NONE : c.n!.cat;
        votes.set(ct, (votes.get(ct) || 0) + c.s);
      }
    }
    if (rest > 0) {
      let best = CAT_NONE;
      let bb = -1;
      for (const [ct, v] of votes) if (ct !== CAT_NONE && v > bb) { bb = v; best = ct; }
      list.push({ s: rest, rk: 0, group: { count: restN, cat: best } });
    }
    const rs: [Item, number, number, number, number][] = [];
    squarify(list, x, y, w, h, rs);
    const g = o.gap / 2;
    const named = (rw: number, rh: number) => rw >= o.labelMinW && rh >= o.labelMinH;
    for (const [c, rx0, ry0, rw0, rh0] of rs) {
      const rx = rx0 + g;
      const ry = ry0 + g;
      const rw = rw0 - 2 * g;
      const rh = rh0 - 2 * g;
      if (rw < 1 || rh < 1) continue;
      const base = { x: rx, y: ry, w: rw, h: rh, depth, parent: pid, count: 0, header: 0 };
      if (c.group) {
        const id = groupId(parent.id);
        out.push({ ...base, id, target: parent.id, count: c.group.count, size: c.s, name: '', kind: Kind.Group, cat: c.group.cat, flags: 0 });
        continue;
      }
      if (c.special) {
        out.push({ ...base, id: c.special === 'free' ? FREE : UNSCANNED, target: c.special === 'free' ? FREE : UNSCANNED, size: c.s, name: '', kind: c.special === 'free' ? Kind.Free : Kind.Unscanned, cat: CAT_NONE, flags: 0 });
        continue;
      }
      const n = c.n!;
      if (!n.kids) {
        out.push({ ...base, id: n.id, target: n.id, size: c.s, name: named(rw, rh) ? n.name : '', kind: Kind.File, cat: n.cat, flags: flagsOf(drive, n) });
        continue;
      }
      // fold chains of single sub-folders into one tile: "archive › pc › content"
      let name = n.name;
      let deep = n;
      let kidsOf = visibleKids(drive, n, sizeMode);
      while (kidsOf.length === 1 && kidsOf[0].kids) {
        deep = kidsOf[0];
        name += ' › ' + deep.name;
        kidsOf = visibleKids(drive, deep, sizeMode);
      }
      const r: Rect = { ...base, id: n.id, target: deep.id, size: c.s, name: '', kind: Kind.DirLeaf, cat: CAT_NONE, flags: flagsOf(drive, n) | flagsOf(drive, deep) };
      out.push(r);
      const hh = rh >= o.header * 2.2 && rw >= o.minHeaderW ? o.header : 0;
      // an open folder always has a header, so it is never a nameless frame
      if (depth < o.maxDepth && (hh || o.header <= 0) && rw * rh > openArea && kidsOf.length) {
        const ix = rx + o.pad;
        const iy = ry + (hh || o.pad);
        const iw = rw - 2 * o.pad;
        const ih = rh - (hh || o.pad) - o.pad;
        if (iw >= 3 && ih >= 3) {
          r.kind = Kind.Dir;
          r.header = hh;
          place(deep, kidsOf.map(m => ({ n: m, s: sizeOf(m, sizeMode), rk: 0 })), ix, iy, iw, ih, depth + 1, n.id);
        }
      }
      if (r.header || named(rw, rh)) r.name = name;
    }
  }
}
