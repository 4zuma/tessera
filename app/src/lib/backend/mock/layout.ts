/**
 * Squarified treemap with the rules from docs/ipc.md ("Layout rules the core
 * implements"). This is the reference the Rust layout module follows.
 */
import { CAT_NONE, FREE, Flag, Kind, NONE, Note, UNSCANNED, groupId, type Rect } from '../codec';
import { CAT_COUNT } from '../categories';
import type { LayoutStyle, SizeFit, SizeMode } from '../types';
import type { Drive, MNode } from './drive';
import { shareOf } from './search';
import { AGE_UNKNOWN, ageByte } from '../../age';

interface Item {
  /** what the area is laid out from: the size, but not for a level's group (rule 9) */
  s: number;
  /** the size, when `s` is not it */
  size?: number;
  rk: number;
  n?: MNode;
  special?: 'free' | 'unscanned';
  group?: { count: number; dirs: number; cat: number; hit: number; age: number };
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
  /** a search's share per node (see ./search), for the records' `hit` */
  hits?: Uint8Array | null;
  /** a level of the view (rule 9): the children it leaves out */
  hide?: ReadonlySet<number> | null;
}

/** rule 9: in a level, the group takes at most this share of the map */
export const LEVEL_GROUP_SHARE = 1 / 3;

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

/** the category with the most bytes below a folder (lower category on a tie), CAT_NONE if none */
export function mainCat(drive: Drive, n: MNode, mode: SizeMode): number {
  const bytes = new Array<number>(CAT_COUNT).fill(0);
  const stack = [n];
  while (stack.length) {
    for (const k of visibleKids(drive, stack.pop()!, mode)) {
      if (k.kids) stack.push(k);
      else if (k.cat < CAT_COUNT) bytes[k.cat] += sizeOf(k, mode);
    }
  }
  let best = CAT_NONE;
  let bb = 0;
  for (let c = 0; c < bytes.length; c++) if (bytes[c] > bb) { bb = bytes[c]; best = c; }
  return best;
}

export function flagsOf(drive: Drive, n: MNode): number {
  let f = 0;
  if (n.lock === 'os' || n.lock === 'mine' || n.lock === 'temp' || n.id === 0) f |= Flag.Protected;
  if (n.lock === 'acl') f |= Flag.NoAccess;
  if (n.lock === 'sys') f |= Flag.InUse;
  if (n.cloud) f |= Flag.Cloud;
  if (n.id === drive.binId || n.id === drive.binUserId) f |= Flag.RecycleBin;
  return f;
}

/**
 * With `min` > 0 (rule 2's minSide + gap), an item that would come out shorter than `min`
 * along a row that is already `min` thick starts the next row instead, so small items do
 * not line up as slivers beside a big one.
 */
const KB = 1024;
const MB = KB * 1024;
const GB = MB * 1024;
const TB = GB * 1024;

/**
 * How wide a folder must be to hold its size label on one line (rule 2, `sizeFit`). The number
 * is written as the UI writes it (format.ts fmtBytes).
 */
export function labelRoom(b: number, f: SizeFit): number {
  const [num, unit] =
    b >= TB ? [(b / TB).toFixed(2), 4]
    : b >= GB ? [(b / GB).toFixed(b >= 100 * GB ? 0 : 1), 3]
    : b >= MB ? [(b / MB).toFixed(b >= 10 * MB ? 0 : 1), 2]
    : b >= KB ? [String(Math.round(b / KB)), 1]
    : [Math.round(b).toLocaleString('en'), 0];
  let nw = 0;
  for (const ch of num) nw += ch >= '0' && ch <= '9' ? f.digit : f.dot;
  return nw + f.space + f.units[unit] + f.inset;
}

export function squarify<T extends { s: number }>(items: T[], x: number, y: number, w: number, h: number, out: [T, number, number, number, number][], min = 0) {
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
      if (min > 0 && sum >= min * short && nmin * short < min * ns) break;
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
  const cats = new Map<number, number>();
  const folderCat = (n: MNode): number => {
    let c = cats.get(n.id);
    if (c === undefined) cats.set(n.id, (c = mainCat(drive, n, sizeMode)));
    return c;
  };
  const hits = inp.hits ?? null;
  const hitOf = (n: MNode) => (hits ? hits[n.id] ?? 0 : 0);
  // age bytes: a file's own, a folder's the size-weighted mean over what it holds
  const ages = new Map<number, number>();
  const ageOf = (n: MNode): number => {
    if (!n.kids) return ageByte(n.age);
    let a = ages.get(n.id);
    if (a !== undefined) return a;
    let sum = 0;
    let w = 0;
    for (const k of visibleKids(drive, n, sizeMode)) {
      const b = ageOf(k);
      const s = sizeOf(k, sizeMode);
      if (b !== AGE_UNKNOWN && s > 0) {
        sum += b * s;
        w += s;
      }
    }
    a = w > 0 ? Math.round(sum / w) : AGE_UNKNOWN;
    ages.set(n.id, a);
    return a;
  };
  const hitPart = (n: MNode, s: number) => {
    const h = hitOf(n);
    return h === 255 ? s : (s * h) / 254;
  };
  const N = drive.nodes;
  const view = N[inp.view];
  const out: Rect[] = [];
  const hide = inp.hide ?? null;
  const kids: Item[] = visibleKids(drive, view, sizeMode)
    .filter(n => !hide?.has(n.id))
    .map(n => ({ n, s: sizeOf(n, sizeMode), rk: 0 }));
  if (inp.view === 0 && !hide) {
    if (inp.unscanned != null && inp.unscanned > 64 * 1024 * 1024) kids.push({ special: 'unscanned', s: inp.unscanned, rk: 0 });
    if (inp.free != null && inp.free > 0) kids.push({ special: 'free', s: inp.free, rk: 0 });
  }
  const total = kids.reduce((a, b) => a + b.s, 0);
  const W = inp.width - 2 * o.outer;
  const H = inp.height - 2 * o.outer;
  // a folder opens only above both thresholds (rule 5)
  const openArea = Math.max(o.minArea * o.recurse, o.openShare * W * H);
  const drawn = place(view, kids, o.outer, o.outer, W, H, 1, NONE, !!hide);
  // a level's map stands for what was laid out, its group at its weight
  return { rects: out, bytesPerPx: (hide ? drawn : total) / Math.max(1, W * H) };

  /** lays out `items` in the box; returns the bytes (weights) it stands for */
  function place(parent: MNode, items: Item[], x: number, y: number, w: number, h: number, depth: number, pid: number, level = false): number {
    if (w < 1 || h < 1 || !items.length) return 0;
    for (const c of items) c.rk = rank(c);
    items.sort((a, b) => b.rk - a.rk);
    let total = 0;
    for (const c of items) total += c.s;
    const k = (w * h) / total;
    // unscanned first, then the folder's items by rank, the group of small ones, free space last
    const head: Item[] = [];
    const nodes: Item[] = [];
    const tail: Item[] = [];
    // A tile narrower or shorter than minSide is hard to see as a tile at all, and a folder
    // narrower than minWidth has no room for its name (rule 2). Items too small to ever be
    // that big go to the group at once; after a layout, the first tile that came out below
    // minSide, every smaller one and every folder before it that is only narrower than
    // minWidth join the group and the folder is laid out again. A group (or a lone left-over
    // item), or a free / unscanned tile, that is itself too thin is not drawn: the others
    // take its room. A tile still too thin after the last pass is left out and its room stays
    // empty.
    // barely enough area seldom comes out square enough, so ask for 30 % more up front
    const ROOM = 1.3;
    const side = o.minSide > 0 ? o.minSide + o.gap : 0;
    const wide = o.minWidth > 0 ? o.minWidth + o.gap : 0;
    const tall = (o.minHeight ?? 0) > 0 ? o.minHeight! + o.gap : 0;
    const sized = side > 0 || wide > 0 || tall > 0 || !!o.sizeFit;
    const isDir = (c: Item) => !!c.n?.kids;
    // with sizeFit a folder needs room for its own size label on one line instead of
    // minHeight, and still minWidth for its name: its least width follows the label
    const sf = o.sizeFit;
    const labelW = new Map<Item, number>();
    const roomOf = (c: Item) => {
      let r = labelW.get(c);
      if (r === undefined) labelW.set(c, (r = labelRoom(c.size ?? c.s, sf!)));
      return r;
    };
    const short = (cw: number, ch: number, c: Item) => {
      if (!sf) return cw < wide || ch < tall;
      return cw < Math.max(wide, roomOf(c) + o.gap) || ch < sf.height + o.gap;
    };
    // 0 fits, 1 a folder only too narrow (or too short for minHeight / its label), 2 below minSide
    const fit = (cw: number, ch: number, c: Item) => (Math.min(cw, ch) < side ? 2 : isDir(c) && short(cw, ch, c) ? 1 : 0);
    let rest = 0;
    let restN = 0;
    // how many of them are folders, so the group can say what it holds
    let restDirs = 0;
    let restHit = 0;
    let restAge = 0;
    let restAgeW = 0;
    const votes = new Map<number, number>();
    // a folder in the group votes with all its bytes for what fills it most (rule 6)
    const vote = (m: Map<number, number>, c: Item) => {
      const ct = c.n!.kids ? folderCat(c.n!) : c.n!.cat;
      m.set(ct, (m.get(ct) || 0) + c.s);
    };
    const least = (c: Item | null) => {
      if (!c || !isDir(c)) return Math.max(o.minArea, ROOM * side * side);
      if (!sf) return Math.max(o.minArea, ROOM * Math.max(side, wide) * Math.max(side, tall));
      return Math.max(o.minArea, ROOM * Math.max(side, wide, roomOf(c) + o.gap) * Math.max(side, sf.height + o.gap));
    };
    // in a level (rule 9) the group takes at most LEVEL_GROUP_SHARE of the box, so the box
    // stands for min(total, kept / (1 - share)) bytes; an item keeps its tile while every
    // one kept so far still gets its least area at that scale, and the first that fails ends it
    let kept = 0;
    let room = Infinity;
    let open = true;
    const keeps = (c: Item): boolean => {
      if (!level) return c.s * k >= least(c) && nodes.length < o.maxPerDir;
      if (!open || nodes.length >= o.maxPerDir) return (open = false);
      const r = Math.min(room, c.s / least(c));
      const sum = kept + c.s;
      if (r * w * h < Math.min(total, sum / (1 - LEVEL_GROUP_SHARE))) return (open = false);
      kept = sum;
      room = r;
      return true;
    };
    for (const c of items) {
      if (c.special === 'unscanned') head.push(c);
      else if (c.special) tail.push(c);
      else if (keeps(c)) nodes.push(c);
      else {
        rest += c.s;
        restN++;
        if (isDir(c)) restDirs++;
        restHit += hitPart(c.n!, c.s);
        const a = ageOf(c.n!);
        if (a !== AGE_UNKNOWN) {
          restAge += a * c.s;
          restAgeW += c.s;
        }
        vote(votes, c);
      }
    }
    let cut = nodes.length;
    let noGroup = false;
    // a level's group is never left out: while too thin it gets twice its least area
    let boost = 1;
    const gone = new Set<Item>();
    let rs: [Item, number, number, number, number][] = [];
    let drawn = 0;
    for (let pass = 0; pass < 12; pass++) {
      let gs = rest;
      let gn = restN;
      let gd = restDirs;
      let gh = restHit;
      let ga = restAge;
      let gw = restAgeW;
      const gv = new Map(votes);
      for (let i = cut; i < nodes.length; i++) {
        gs += nodes[i].s;
        gn++;
        if (isDir(nodes[i])) gd++;
        gh += hitPart(nodes[i].n!, nodes[i].s);
        const a = ageOf(nodes[i].n!);
        if (a !== AGE_UNKNOWN) {
          ga += a * nodes[i].s;
          gw += nodes[i].s;
        }
        vote(gv, nodes[i]);
      }
      const mid = nodes.slice(0, cut);
      let grp: Item | null = null;
      // a group of one would hide a name for nothing; in a level the one left over is weighed
      // like the group, so it is drawn big enough to be pointed at (a group of one would only
      // ask for another level)
      const lone = gn === 1 ? (cut < nodes.length ? nodes[cut] : items.find(c => !c.special && !nodes.includes(c))!) : null;
      if (lone && !level) grp = lone;
      else if (gn >= 1) {
        let best = CAT_NONE;
        let bb = -1;
        for (const [ct, v] of gv) if (ct !== CAT_NONE && v > bb) { bb = v; best = ct; }
        // in a level, beside `others` bytes of tiles: at most LEVEL_GROUP_SHARE of the box and at
        // least the least area of a tile (its share of the box is weight / (others + weight))
        let weight = gs;
        const others = mid.reduce((a, c) => a + c.s, 0);
        if (level && others > 0) {
          const most = (others * LEVEL_GROUP_SHARE) / (1 - LEVEL_GROUP_SHARE);
          const l = least(lone) * boost;
          const fewest = w * h > 2 * l ? (others * l) / (w * h - l) : 0;
          weight = Math.max(1, Math.round(Math.min(Math.max(gs, fewest), Math.max(most, fewest))));
        }
        grp = lone
          ? { ...lone, s: weight, size: gs, rk: weight }
          : { s: weight, size: gs, rk: weight, group: { count: gn, dirs: gd, cat: best, hit: shareOf(gh, gs), age: gw > 0 ? Math.round(ga / gw) : AGE_UNKNOWN } };
      }
      if (grp && !noGroup) {
        // with minSide the group takes its place by size, not the strip left at the end
        let at = sized ? mid.findIndex(c => c.rk < grp.rk) : -1;
        if (at < 0) at = mid.length;
        mid.splice(at, 0, grp);
      }
      const list = [...head, ...mid, ...tail].filter(c => !gone.has(c));
      drawn = list.reduce((a, c) => a + c.s, 0);
      rs = [];
      squarify(list, x, y, w, h, rs, side);
      if (!sized) break;
      let first = -1;
      const narrow = new Set<Item>();
      let grpThin = false;
      let dropped = false;
      for (const [c, , , cw, ch] of rs) {
        const f = fit(cw, ch, c);
        if (!f) continue;
        if (c === grp) grpThin = true;
        else if (c.special) {
          gone.add(c);
          dropped = true;
        } else {
          const i = nodes.indexOf(c);
          if (i < 0 || i >= cut) continue;
          if (f === 1) narrow.add(c);
          else if (first < 0 || i < first) first = i;
        }
      }
      if (first >= 0 || narrow.size) {
        // the narrow folders move behind the ones kept, in the same order
        const end = first >= 0 ? first : cut;
        const head = nodes.slice(0, end);
        const kept = head.filter(c => !narrow.has(c));
        nodes.splice(0, end, ...kept, ...head.filter(c => narrow.has(c)));
        cut = kept.length;
        noGroup = false;
      } else if (grpThin && level && boost < 16) boost *= 2;
      else if (grpThin && !noGroup) noGroup = true;
      else if (!dropped) break;
    }
    const g = o.gap / 2;
    const named = (rw: number, rh: number) => rw >= o.labelMinW && rh >= o.labelMinH;
    for (const [c, rx0, ry0, rw0, rh0] of rs) {
      const rx = rx0 + g;
      const ry = ry0 + g;
      const rw = rw0 - 2 * g;
      const rh = rh0 - 2 * g;
      if (rw < 1 || rh < 1 || (sized && fit(rw0, rh0, c))) continue;
      const base = { x: rx, y: ry, w: rw, h: rh, depth, parent: pid, count: 0, header: 0, hit: c.n ? hitOf(c.n) : (c.group?.hit ?? 0), age: c.n ? ageOf(c.n) : (c.group?.age ?? AGE_UNKNOWN) };
      if (c.group) {
        const id = groupId(parent.id);
        const size = c.size ?? c.s;
        const holds = (c.group.count > c.group.dirs ? Note.Files : 0) | (c.group.dirs > 0 ? Note.Folders : 0);
        out.push({ ...base, id, target: parent.id, count: c.group.count, size, name: '', kind: Kind.Group, cat: c.group.cat, flags: 0, note: (size !== c.s ? Note.NotToScale : 0) | holds });
        continue;
      }
      if (c.special) {
        out.push({ ...base, id: c.special === 'free' ? FREE : UNSCANNED, target: c.special === 'free' ? FREE : UNSCANNED, size: c.s, name: '', kind: c.special === 'free' ? Kind.Free : Kind.Unscanned, cat: CAT_NONE, flags: 0 });
        continue;
      }
      const n = c.n!;
      // a level's lone left-over item is laid out from its weight (rule 9)
      const size = c.size ?? c.s;
      const note = size !== c.s ? Note.NotToScale : 0;
      if (!n.kids) {
        out.push({ ...base, id: n.id, target: n.id, size, name: named(rw, rh) ? n.name : '', kind: Kind.File, cat: n.cat, flags: flagsOf(drive, n), note });
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
      const r: Rect = { ...base, id: n.id, target: deep.id, size, name: '', kind: Kind.DirLeaf, cat: CAT_NONE, flags: flagsOf(drive, n) | flagsOf(drive, deep), note };
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
      if (r.kind === Kind.DirLeaf) r.cat = folderCat(n);
      // with sizeFit every folder tile has room for its name over its size, however narrow
      if (r.header || named(rw, rh) || o.sizeFit) r.name = name;
    }
    return drawn;
  }
}
