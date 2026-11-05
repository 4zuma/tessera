/**
 * Layout buffer codec, see docs/ipc.md. The Rust core writes this format; the mock
 * backend writes it with `encodeLayout` so the UI always goes through `decodeLayout`.
 */

export const MAGIC = 0x4c525354; // "TSRL"
export const VERSION = 1;
export const HEADER_SIZE = 32;
export const RECORD_SIZE = 52;

export const NONE = 0xffffffff;
export const FREE = 0xfffffffe;
export const UNSCANNED = 0xfffffffd;
export const GROUP_BIT = 0x80000000;

export const Kind = {
  File: 0,
  Dir: 1,
  DirLeaf: 2,
  Group: 3,
  Free: 4,
  Unscanned: 5,
} as const;
export type Kind = (typeof Kind)[keyof typeof Kind];

export const Flag = {
  Protected: 1,
  NoAccess: 2,
  InUse: 4,
  Cloud: 8,
  Hardlink: 16,
  RecycleBin: 32,
  Reparse: 64,
  Scanning: 128,
} as const;

export const CAT_NONE = 255;

/** One rectangle as the UI uses it. */
export interface Rect {
  id: number;
  parent: number;
  target: number;
  count: number;
  x: number;
  y: number;
  w: number;
  h: number;
  size: number;
  /** empty when the core did not send a name (rect too small for a label) */
  name: string;
  depth: number;
  kind: Kind;
  cat: number;
  flags: number;
  header: number;
}

export interface Layout {
  view: number;
  rev: number;
  bytesPerPx: number;
  rects: Rect[];
}

export const isGroupId = (id: number) => id !== NONE && id !== FREE && id !== UNSCANNED && id >= GROUP_BIT;
export const isSpecialId = (id: number) => id === FREE || id === UNSCANNED || id === NONE;
/** a real tree node (not a group, free space, or the unscanned remainder) */
export const isNodeId = (id: number) => id < GROUP_BIT;
export const groupFolder = (id: number) => id - GROUP_BIT;
export const groupId = (folder: number) => GROUP_BIT + folder;

const utf8 = new TextDecoder('utf-8');
const utf8enc = new TextEncoder();

export class LayoutFormatError extends Error {}

export function decodeLayout(buf: ArrayBuffer): Layout {
  if (buf.byteLength < HEADER_SIZE) throw new LayoutFormatError(`layout buffer too short (${buf.byteLength} bytes)`);
  const dv = new DataView(buf);
  const magic = dv.getUint32(0, true);
  if (magic !== MAGIC) throw new LayoutFormatError(`bad layout magic 0x${magic.toString(16)}`);
  const version = dv.getUint16(4, true);
  if (version !== VERSION) throw new LayoutFormatError(`unsupported layout version ${version}`);
  const headerSize = dv.getUint16(6, true);
  const count = dv.getUint32(8, true);
  const recordSize = dv.getUint32(12, true);
  const namesLen = dv.getUint32(16, true);
  const view = dv.getUint32(20, true);
  const rev = dv.getUint32(24, true);
  const bytesPerPx = dv.getFloat32(28, true);
  if (recordSize < RECORD_SIZE) throw new LayoutFormatError(`record size ${recordSize} < ${RECORD_SIZE}`);
  const namesAt = headerSize + count * recordSize;
  if (namesAt + namesLen > buf.byteLength) throw new LayoutFormatError('layout buffer truncated');
  const pool = new Uint8Array(buf, namesAt, namesLen);

  const rects: Rect[] = new Array(count);
  let o = headerSize;
  for (let i = 0; i < count; i++, o += recordSize) {
    const nameOff = dv.getUint32(o + 40, true);
    const nameLen = dv.getUint16(o + 44, true);
    rects[i] = {
      id: dv.getUint32(o, true),
      parent: dv.getUint32(o + 4, true),
      target: dv.getUint32(o + 8, true),
      count: dv.getUint32(o + 12, true),
      x: dv.getFloat32(o + 16, true),
      y: dv.getFloat32(o + 20, true),
      w: dv.getFloat32(o + 24, true),
      h: dv.getFloat32(o + 28, true),
      size: dv.getUint32(o + 32, true) + dv.getUint32(o + 36, true) * 4294967296,
      name: nameLen && nameOff + nameLen <= namesLen ? utf8.decode(pool.subarray(nameOff, nameOff + nameLen)) : '',
      depth: dv.getUint8(o + 46),
      kind: dv.getUint8(o + 47) as Kind,
      cat: dv.getUint8(o + 48),
      flags: dv.getUint8(o + 49),
      header: dv.getUint8(o + 50),
    };
  }
  return { view, rev, bytesPerPx, rects };
}

/** Writes the same format the Rust core produces (used by the mock and by tests). */
export function encodeLayout(l: Layout): ArrayBuffer {
  const names: Uint8Array[] = [];
  const offs = new Uint32Array(l.rects.length);
  const lens = new Uint16Array(l.rects.length);
  let namesLen = 0;
  l.rects.forEach((r, i) => {
    if (!r.name) return;
    let b = utf8enc.encode(r.name);
    if (b.length > 0xffff) b = b.subarray(0, 0xffff);
    offs[i] = namesLen;
    lens[i] = b.length;
    names.push(b);
    namesLen += b.length;
  });
  const buf = new ArrayBuffer(HEADER_SIZE + l.rects.length * RECORD_SIZE + namesLen);
  const dv = new DataView(buf);
  dv.setUint32(0, MAGIC, true);
  dv.setUint16(4, VERSION, true);
  dv.setUint16(6, HEADER_SIZE, true);
  dv.setUint32(8, l.rects.length, true);
  dv.setUint32(12, RECORD_SIZE, true);
  dv.setUint32(16, namesLen, true);
  dv.setUint32(20, l.view >>> 0, true);
  dv.setUint32(24, l.rev >>> 0, true);
  dv.setFloat32(28, l.bytesPerPx, true);
  let o = HEADER_SIZE;
  l.rects.forEach((r, i) => {
    dv.setUint32(o, r.id >>> 0, true);
    dv.setUint32(o + 4, r.parent >>> 0, true);
    dv.setUint32(o + 8, r.target >>> 0, true);
    dv.setUint32(o + 12, r.count >>> 0, true);
    dv.setFloat32(o + 16, r.x, true);
    dv.setFloat32(o + 20, r.y, true);
    dv.setFloat32(o + 24, r.w, true);
    dv.setFloat32(o + 28, r.h, true);
    const size = Math.max(0, Math.round(r.size));
    dv.setUint32(o + 32, size % 4294967296, true);
    dv.setUint32(o + 36, Math.floor(size / 4294967296), true);
    dv.setUint32(o + 40, offs[i], true);
    dv.setUint16(o + 44, lens[i], true);
    dv.setUint8(o + 46, Math.min(255, r.depth));
    dv.setUint8(o + 47, r.kind);
    dv.setUint8(o + 48, r.cat);
    dv.setUint8(o + 49, r.flags);
    dv.setUint8(o + 50, Math.min(255, Math.round(r.header)));
    o += RECORD_SIZE;
  });
  const pool = new Uint8Array(buf, o, namesLen);
  let p = 0;
  for (const b of names) {
    pool.set(b, p);
    p += b.length;
  }
  return buf;
}
