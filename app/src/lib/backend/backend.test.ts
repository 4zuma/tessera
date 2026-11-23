import { describe, expect, it } from 'vitest';
import { contentOf, decodeLayout, encodeLayout, FREE, GROUP_BIT, isNodeId, Kind, NONE, RECORD_SIZE, HEADER_SIZE, UNSCANNED, type Rect } from './codec';
import { categoryOf, Cat } from './categories';
import { t } from '../i18n.svelte';
import { MockBackend } from './mock';
import { BadSize, parseQuery } from './mock/search';
import type { LayoutRequest, ScanProgress } from './types';

const STYLE = { outer: 6, pad: 3, gap: 2, header: 21, minHeaderW: 60, minArea: 110, recurse: 60, openShare: 0.0025, maxDepth: 14, maxPerDir: 400, labelMinW: 44, labelMinH: 20 };
const req = (view = 0): LayoutRequest => ({ view, width: 1200, height: 800, sizeMode: 'disk', style: STYLE });

async function scanned(b: MockBackend, root = 'C:\\') {
  const done = new Promise<ScanProgress>(res => b.on('scan-progress', p => p.done && res(p)));
  await b.scanStart(root);
  return done;
}

describe('codec', () => {
  it('round-trips records, big sizes and UTF-8 names', () => {
    const rects: Rect[] = [
      { id: 7, parent: NONE, target: 9, count: 0, x: 1.5, y: 2, w: 300, h: 200, size: Math.round(499.8 * 2 ** 30), name: '屏幕截图 › archive', depth: 1, kind: Kind.Dir, cat: 255, flags: 1, header: 21, hit: 0 },
      { id: GROUP_BIT + 7, parent: 7, target: 7, count: 42, x: 4, y: 30, w: 10, h: 10, size: 12345, name: '', depth: 2, kind: Kind.Group, cat: 4, flags: 0, header: 0, hit: 127 },
      { id: FREE, parent: NONE, target: FREE, count: 0, x: 0, y: 0, w: 5, h: 5, size: 2 ** 40 + 3, name: '', depth: 1, kind: Kind.Free, cat: 255, flags: 0, header: 0, hit: 255 },
    ];
    const buf = encodeLayout({ view: 3, rev: 99, bytesPerPx: 1024, search: 5, rects });
    expect(buf.byteLength).toBe(HEADER_SIZE + 3 * RECORD_SIZE + new TextEncoder().encode(rects[0].name).length);
    const l = decodeLayout(buf);
    expect(l.view).toBe(3);
    expect(l.rev).toBe(99);
    expect(l.search).toBe(5);
    expect(l.rects).toEqual(rects);
  });

  it('points a tile at the folder whose contents it shows', () => {
    const r = (kind: Kind, id: number, target: number): Rect => ({ id, parent: NONE, target, count: 0, x: 0, y: 0, w: 9, h: 9, size: 1, name: '', depth: 1, kind, cat: 255, flags: 0, header: 0, hit: 0 });
    expect(contentOf(r(Kind.Dir, 7, 9))).toBe(9); // a › b › c: c
    expect(contentOf(r(Kind.DirLeaf, 7, 9))).toBe(9);
    expect(contentOf(r(Kind.Dir, 7, 7))).toBe(7);
    expect(contentOf(r(Kind.File, 5, 5))).toBe(5);
    expect(contentOf(r(Kind.Group, GROUP_BIT + 7, 7))).toBe(GROUP_BIT + 7);
    expect(contentOf(r(Kind.Free, FREE, FREE))).toBe(FREE);
  });

  it('rejects foreign buffers', () => {
    expect(() => decodeLayout(new ArrayBuffer(8))).toThrow();
    expect(() => decodeLayout(new ArrayBuffer(64))).toThrow(/magic/);
  });
});

describe('categories', () => {
  it('maps extensions case-insensitively', () => {
    expect(categoryOf('DSC01234.ARW')).toBe(Cat.Image);
    expect(categoryOf('ext4.vhdx')).toBe(Cat.VDisk);
    expect(categoryOf('Makefile')).toBe(Cat.Other);
    expect(categoryOf('.gitignore')).toBe(Cat.Other);
    // the same table and .ts rule as tessera_core::category
    expect(categoryOf('Win11.iso')).toBe(Cat.VDisk);
    expect(categoryOf('setup.exe.crdownload')).toBe(Cat.Cache);
    expect(categoryOf('pagefile.sys')).toBe(Cat.System);
    expect(categoryOf('index.d.ts', 2048)).toBe(Cat.Dev);
    expect(categoryOf('capture.ts', 4 * 2 ** 30)).toBe(Cat.Video);
  });
});

describe('mock backend', () => {
  it('scans C: to the volume used size and lays it out without overlaps', async () => {
    const b = new MockBackend({}, { scanMs: 300 });
    const p = await scanned(b);
    expect(Math.abs(p.scanned - p.used) / p.used).toBeLessThan(1e-5);
    const l = decodeLayout(await b.layout(req()));
    expect(l.rects.length).toBeGreaterThan(300);
    expect(l.rects.length).toBeLessThan(10000);
    const byId = new Map(l.rects.map(r => [r.id, r]));
    expect(byId.size).toBe(l.rects.length); // ids are unique
    expect(byId.has(FREE)).toBe(true);
    expect(byId.has(UNSCANNED)).toBe(false); // scan finished
    for (const r of l.rects) {
      expect(r.x).toBeGreaterThanOrEqual(5);
      expect(r.y).toBeGreaterThanOrEqual(5);
      expect(r.x + r.w).toBeLessThanOrEqual(1195.01);
      expect(r.y + r.h).toBeLessThanOrEqual(795.01);
      if (r.parent !== NONE) {
        const p = byId.get(r.parent)!;
        expect(p.kind).toBe(Kind.Dir);
        expect(r.x).toBeGreaterThanOrEqual(p.x);
        expect(r.y).toBeGreaterThanOrEqual(p.y + p.header - 0.01);
        expect(r.depth).toBe(p.depth + 1);
      }
      if (r.header) expect(r.name).not.toBe('');
    }
    // paint order: parents before children
    const seen = new Set<number>();
    for (const r of l.rects) {
      if (r.parent !== NONE) expect(seen.has(r.parent)).toBe(true);
      seen.add(r.id);
    }
  });

  it('folds single-folder chains and shows the unscanned tile while scanning', async () => {
    const b = new MockBackend({}, { scanMs: 300 });
    await scanned(b);
    const all = decodeLayout(await b.layout({ ...req(), width: 4000, height: 3000 })).rects;
    const chain = all.find(r => r.name.startsWith('archive › pc › content'));
    expect(chain).toBeTruthy();
    expect(chain!.target).not.toBe(chain!.id);
    // the tile's path is where its files are: copy path and Explorer go to the deepest folder
    const top = (await b.details(chain!.id, 0, 'disk'))!.path;
    const inner = (await b.details(contentOf(chain!), 0, 'disk'))!.path;
    expect(top.endsWith('\\archive')).toBe(true);
    expect(inner).toBe(`${top}\\${chain!.name.split(' › ').slice(1).join('\\')}`);

    const first = new Promise<void>(res => b.on('scan-progress', p => !p.done && p.scanned > 0 && res()));
    await b.scanStart('C:\\');
    await first;
    const mid = decodeLayout(await b.layout(req())).rects;
    expect(mid.some(r => r.id === UNSCANNED)).toBe(true);
    await b.scanCancel();
  });

  it('searches: the hits reach the layout until cleared', async () => {
    const b = new MockBackend({}, { scanMs: 300 });
    await scanned(b);
    const res = (await b.search('.ISO', 'disk'))!;
    expect(res.count).toBeGreaterThanOrEqual(2);
    const l = decodeLayout(await b.layout({ ...req(), width: 4000, height: 3000 }));
    expect(l.search).toBe(res.id);
    // (small tiles come without a name)
    const files = l.rects.filter(r => r.kind === Kind.File && isNodeId(r.id) && r.name);
    const iso = files.filter(r => r.name.toLowerCase().endsWith('.iso'));
    expect(iso.length).toBeGreaterThanOrEqual(2);
    for (const r of files) expect([r.name, r.hit]).toEqual([r.name, iso.includes(r) ? 255 : 0]);
    expect(iso.reduce((a, r) => a + r.size, 0)).toBeLessThanOrEqual(res.bytes);
    expect(l.rects.find(r => r.id === FREE)!.hit).toBe(0);
    // a folder holding one of them is partly lit, by bytes
    const dl = l.rects.find(r => r.id === iso[0].parent)!;
    expect(dl.hit).toBeGreaterThan(0);
    expect(dl.hit).toBeLessThan(255);
    // a folder matches by name, and everything in it is lit; only the folder counts
    const steam = (await b.search('stardew', 'disk'))!;
    expect(steam.count).toBeGreaterThanOrEqual(1);
    await expect(b.search('>1XB', 'disk')).rejects.toThrow(t('core.badSize', { word: '>1XB' }));
    await b.searchClear();
    const off = decodeLayout(await b.layout(req()));
    expect(off.search).toBe(0);
    expect(off.rects.every(r => r.hit === 0)).toBe(true);
  });

  it('reads queries as the core does', () => {
    expect(parseQuery('  ')).toBeNull();
    expect(parseQuery('"my file" .MP4 *.mkv >=1.5GB <2g')).toEqual({
      words: ['MY FILE'],
      exts: ['.MP4', '.MKV'],
      sizes: [
        ['>=', 1.5 * 2 ** 30],
        ['<', 2 ** 31],
      ],
    });
    expect(parseQuery('. ..')!.words).toEqual(['.', '..']);
    expect(() => parseQuery('>')).toThrow(BadSize);
    expect(() => parseQuery('<5 apples')).not.toThrow();
    expect(() => parseQuery('<5apples')).toThrow(BadSize);
  });

  it('recycles into $Recycle.Bin, restores, and purges', async () => {
    const b = new MockBackend({}, { scanMs: 300 });
    await scanned(b);
    const before = (await b.details(0, 0, 'disk'))!;
    const dl = (await b.nav(0)) && decodeLayout(await b.layout(req())).rects.find(r => r.kind === Kind.File && !r.flags);
    expect(dl).toBeTruthy();
    const res = await b.recycle([dl!.id]);
    expect(res.ok).toEqual([dl!.id]);
    expect(res.undo).not.toBeNull();
    // still on the same volume: total does not change
    expect((await b.details(0, 0, 'disk'))!.size).toBe(before.size);
    expect((await b.details(dl!.id, 0, 'disk'))!.path).toContain('$Recycle.Bin');
    await b.restore(res.undo!);
    expect((await b.details(dl!.id, 0, 'disk'))!.path).not.toContain('$Recycle.Bin');
    const p = await b.purge([dl!.id]);
    expect(p.bytes).toBe(dl!.size);
    expect((await b.details(0, 0, 'disk'))!.size).toBe(before.size - dl!.size);
  });

  it('refuses protected items', async () => {
    const b = new MockBackend({}, { scanMs: 300 });
    await scanned(b);
    const win = (await b.nav(0)).crumbs; // root crumb only
    expect(win[0].name).toBe('C:');
    const rects = decodeLayout(await b.layout(req())).rects;
    const windows = rects.find(r => r.name === 'Windows' || r.name.startsWith('Windows ›'))!;
    const res = await b.recycle([windows.id]);
    expect(res.ok).toEqual([]);
    expect(res.failed[0].reason).toBe(t('mock.systemFolder'));
  });
});
