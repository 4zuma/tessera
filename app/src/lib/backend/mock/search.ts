/**
 * The search box's query over the mock drive, with the rules of the Rust core
 * (crates/tessera-core/src/search.rs): words are ANDed, `.ext` / `*.ext` (files only, any
 * of them), `>1GB` / `<=500MB` size bounds in binary units, anything else a part of the
 * name, ignoring case. A folder matches only through name words (and no extension word);
 * everything in a matching folder matches too, but only the folder is counted.
 */
import type { SizeMode } from '../types';
import type { Drive, MNode } from './drive';
import { sizeOf } from './layout';

type Cmp = '>' | '>=' | '<' | '<=';

export interface Query {
  words: string[];
  exts: string[];
  sizes: [Cmp, number][];
}

/** a size word that can't be read; `word` is shown to the user */
export class BadSize extends Error {
  constructor(readonly word: string) {
    super(`not a size: ${word}`);
  }
}

const fold = (s: string) => s.toUpperCase();

const UNITS: Record<string, number> = { '': 0, b: 0, k: 10, kb: 10, kib: 10, m: 20, mb: 20, mib: 20, g: 30, gb: 30, gib: 30, t: 40, tb: 40, tib: 40 };

function parseSize(s: string): number | null {
  const m = /^([0-9.]*)(.*)$/.exec(s)!;
  if (!/^\d*\.?\d*$/.test(m[1]) || !/\d/.test(m[1])) return null;
  const shift = UNITS[m[2].trim().toLowerCase()];
  if (shift === undefined) return null;
  return Math.round(parseFloat(m[1]) * 2 ** shift);
}

function split(text: string): [string, boolean][] {
  const out: [string, boolean][] = [];
  let rest = text;
  for (;;) {
    rest = rest.trimStart();
    if (!rest) return out;
    if (rest[0] === '"') {
      const end = rest.indexOf('"', 1);
      const w = end < 0 ? rest.slice(1) : rest.slice(1, end);
      if (w) out.push([w, true]);
      rest = end < 0 ? '' : rest.slice(end + 1);
    } else {
      const m = /\s/.exec(rest);
      const end = m ? m.index : rest.length;
      out.push([rest.slice(0, end), false]);
      rest = rest.slice(end);
    }
  }
}

/** null for an empty query; throws BadSize */
export function parseQuery(text: string): Query | null {
  const q: Query = { words: [], exts: [], sizes: [] };
  for (const [w, quoted] of split(text)) {
    if (quoted) q.words.push(fold(w));
    else if (w[0] === '>' || w[0] === '<') {
      const eq = w[1] === '=';
      const bytes = parseSize(w.slice(eq ? 2 : 1));
      if (bytes == null) throw new BadSize(w);
      q.sizes.push([(w[0] + (eq ? '=' : '')) as Cmp, bytes]);
    } else {
      const ext = w.startsWith('*.') ? w.slice(2) : w.startsWith('.') ? w.slice(1) : null;
      if (ext && !ext.startsWith('.')) q.exts.push('.' + fold(ext));
      else q.words.push(fold(w));
    }
  }
  return q.words.length || q.exts.length || q.sizes.length ? q : null;
}

const holds = (c: Cmp, size: number, b: number) => (c === '>' ? size > b : c === '>=' ? size >= b : c === '<' ? size < b : size <= b);

/** share of `size` that `part` is, 0 (none) to 255 (all), any part at least 1 */
export function shareOf(part: number, size: number): number {
  if (part <= 0) return 0;
  if (part >= size) return 255;
  return Math.max(1, Math.floor((part * 254) / size));
}

export interface MockHits {
  id: number;
  share: Uint8Array;
  count: number;
  bytes: number;
}

export function runSearch(d: Drive, q: Query, mode: SizeMode, id: number): MockHits {
  const share = new Uint8Array(d.nodes.length);
  const folders = q.words.length > 0 && q.exts.length === 0;
  const fits = (n: MNode) => {
    const dir = !!n.kids;
    if (dir && !folders) return false;
    const size = sizeOf(n, mode);
    if (!q.sizes.every(([c, b]) => holds(c, size, b))) return false;
    const name = fold(n.name);
    if (!dir && q.exts.length && !q.exts.some(e => name.endsWith(e))) return false;
    return q.words.every(w => name.includes(w));
  };
  let count = 0;
  const visit = (n: MNode, inside: boolean): number => {
    if (!n.kids) {
      const hit = inside || fits(n);
      if (!hit) return 0;
      if (!inside) count++;
      share[n.id] = 255;
      return sizeOf(n, mode);
    }
    const own = !inside && n.id !== 0 && fits(n);
    if (own) count++;
    let part = 0;
    for (const k of n.kids) {
      const c = d.nodes[k];
      if (c.seen && c.parent !== -2) part += visit(c, inside || own);
    }
    const size = sizeOf(n, mode);
    if (inside || own) part = size;
    share[n.id] = inside || own ? 255 : shareOf(part, size);
    return part;
  };
  const bytes = visit(d.nodes[0], false);
  return { id, share, count, bytes };
}
