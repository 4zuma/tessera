import { describe, expect, it } from 'vitest';
import { contentOf, decodeLayout, encodeLayout, FREE, GROUP_BIT, groupId, isNodeId, Kind, NONE, Note, RECORD_SIZE, HEADER_SIZE, UNSCANNED, type Rect } from './codec';
import { categoryOf, Cat } from './categories';
import { t } from '../i18n.svelte';
import { MockBackend } from './mock';
import { labelRoom } from './mock/layout';
import { AGE_BINS, AGE_EDGE_CHOICES, AGE_UNKNOWN, ageBand, ageBin, ageByte, bandTotals } from '../age';
import { ageTable, BadSize, parseQuery } from './mock/search';
import type { LayoutRequest, ScanProgress, SizeFit } from './types';

const STYLE = { outer: 6, pad: 3, gap: 2, header: 21, minHeaderW: 60, minArea: 110, minSide: 0, minWidth: 0, recurse: 60, openShare: 0.0025, maxDepth: 14, maxPerDir: 400, labelMinW: 44, labelMinH: 20 };
const req = (view = 0): LayoutRequest => ({ view, width: 1200, height: 800, sizeMode: 'disk', style: STYLE });

async function scanned(b: MockBackend, root = 'C:\\') {
  const done = new Promise<ScanProgress>(res => b.on('scan-progress', p => p.done && res(p)));
  await b.scanStart(root);
  return done;
}

describe('codec', () => {
  it('round-trips records, big sizes and UTF-8 names', () => {
    const rects: Rect[] = [
      { id: 7, parent: NONE, target: 9, count: 0, x: 1.5, y: 2, w: 300, h: 200, size: Math.round(499.8 * 2 ** 30), name: '屏幕截图 › archive', depth: 1, kind: Kind.Dir, cat: 255, flags: 1, header: 21, hit: 0, age: 0, note: 0 },
      { id: GROUP_BIT + 7, parent: 7, target: 7, count: 42, x: 4, y: 30, w: 10, h: 10, size: 12345, name: '', depth: 2, kind: Kind.Group, cat: 4, flags: 0, header: 0, hit: 127, age: 200, note: Note.NotToScale },
      { id: FREE, parent: NONE, target: FREE, count: 0, x: 0, y: 0, w: 5, h: 5, size: 2 ** 40 + 3, name: '', depth: 1, kind: Kind.Free, cat: 255, flags: 0, header: 0, hit: 255, age: 1, note: 0 },
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
    const r = (kind: Kind, id: number, target: number): Rect => ({ id, parent: NONE, target, count: 0, x: 0, y: 0, w: 9, h: 9, size: 1, name: '', depth: 1, kind, cat: 255, flags: 0, header: 0, hit: 0, age: 0 });
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

  it('keeps every tile at least minSide either way, and every folder minWidth wide', async () => {
    const b = new MockBackend({}, { scanMs: 300 });
    await scanned(b);
    const style = { ...STYLE, minSide: 24, minWidth: 44 };
    for (const [width, height] of [[900, 640], [1280, 820], [1700, 1050]]) {
      const off = decodeLayout(await b.layout({ ...req(), width, height })).rects;
      const on = decodeLayout(await b.layout({ ...req(), width, height, style })).rects;
      expect(off.some(r => Math.min(r.w, r.h) < 24)).toBe(true);
      const dir = (r: Rect) => r.kind === Kind.Dir || r.kind === Kind.DirLeaf;
      const thin = on.filter(r => r.w < (dir(r) ? 44 : 24) - 1e-3 || r.h < 24 - 1e-3);
      expect(thin).toEqual([]);
      expect(on.length).toBeLessThan(off.length);
      // every folder tile is wide enough for its name; files may be as small as 24 x 24
      expect(on.filter(r => r.kind === Kind.DirLeaf && !r.name)).toEqual([]);
      // groups, often of folders now, are tinted by what fills them (the sample's files all have a type)
      const groups = on.filter(r => r.kind === Kind.Group);
      expect(groups.length).toBeGreaterThan(0);
      expect(groups.filter(r => r.cat === 255)).toEqual([]);
      // and each says what it holds (named small files, small folders or small items)
      expect(groups.filter(r => !((r.note ?? 0) & (Note.Files | Note.Folders)))).toEqual([]);
      expect(groups.some(r => (r.note ?? 0) & Note.Folders)).toBe(true);
      expect(groups.some(r => (r.note ?? 0) & Note.Files)).toBe(true);
    }
  });

  it('gives every folder room for its own size label with sizeFit, on one line or two', async () => {
    const b = new MockBackend({}, { scanMs: 300 });
    await scanned(b);
    // Segoe UI 12 px, roughly: the band look's 45 / 61 px folders
    const sizeFit: SizeFit = { digit: 6.9, dot: 3, space: 3.3, units: [32, 13.7, 17.2, 15.1, 13.3], inset: 16, one: 45, two: 61 };
    const fixed = { ...STYLE, minSide: 24, minWidth: 58, minHeight: 45 };
    for (const [width, height] of [[960, 560], [1600, 900]]) {
      const on = decodeLayout(await b.layout({ ...req(), width, height, style: { ...STYLE, minSide: 24, minWidth: 44, sizeFit } })).rects;
      const folders = on.filter(r => r.kind === Kind.DirLeaf);
      const cramped = folders.filter(r => {
        const [one, two] = labelRoom(r.size, sizeFit);
        return !((r.w >= one - 1e-3 && r.h >= 45 - 1e-3) || (r.w >= two - 1e-3 && r.h >= 61 - 1e-3));
      });
      expect(cramped).toEqual([]);
      expect(folders.filter(r => !r.name)).toEqual([]);
      expect(on.filter(r => Math.min(r.w, r.h) < 24 - 1e-3)).toEqual([]);
      // each folder asks for its own label, not the widest one: more of them keep a tile
      const one = decodeLayout(await b.layout({ ...req(), width, height, style: fixed })).rects;
      expect(folders.length).toBeGreaterThan(one.filter(r => r.kind === Kind.DirLeaf).length);
    }
  });

  it('opens a group as a level, again and again, until every item had a tile (rule 9)', async () => {
    const b = new MockBackend({}, { scanMs: 300 });
    await scanned(b);
    const style = { ...STYLE, minSide: 24, minWidth: 44 };
    const nodes = (b as unknown as { d: { nodes: { id: number; name: string; kids?: number[] }[] } }).d.nodes;
    const shots = nodes.find(n => n.name === 'Screenshots')!;
    const all = [...shots.kids!].sort((x, y) => x - y);
    // 914 x 630 left one item over on the fourth level before it was drawn as itself
    for (const [W, H] of [
      [924, 640],
      [914, 630],
      [700, 500],
    ]) {
      const map = (W - 12) * (H - 12);
      let hide: number[] | undefined;
      const seen: number[] = [];
      for (let level = 0; ; level++) {
        expect(level).toBeLessThan(60);
        const l = decodeLayout(await b.layout({ ...req(shots.id), width: W, height: H, style, hide }));
        const top = l.rects.filter(r => r.parent === NONE);
        expect(l.view).toBe(shots.id);
        expect(top.every(r => r.kind !== Kind.Free && r.kind !== Kind.Unscanned)).toBe(true);
        const tiles = top.filter(r => isNodeId(r.id)).map(r => r.id);
        expect(tiles.length).toBeGreaterThan(0);
        for (const id of tiles) expect(seen.includes(id)).toBe(false);
        seen.push(...tiles);
        const g = top.find(r => r.kind === Kind.Group);
        if (hide && g) expect((g.w + 2) * (g.h + 2)).toBeLessThanOrEqual((map / 3) * 1.01);
        if (!g) break;
        expect(g.id).toBe(groupId(shots.id));
        expect(g.count).toBe(all.length - seen.length);
        // one item left over gets a tile of its own, never a group of one (it would only ask for another level)
        if (hide) expect(g.count).toBeGreaterThan(1);
        // the legend counts only what the level holds
        hide = [...seen];
        const cats = await b.catStats(shots.id, 'disk', hide);
        const sizes = new Map(nodes.map(n => [n.id, (n as unknown as { disk: number }).disk]));
        const left = all.filter(id => !seen.includes(id)).reduce((a, id) => a + (sizes.get(id) ?? 0), 0);
        expect(cats.reduce((a, c) => a + c.bytes, 0)).toBe(left);
        // a level's group that would take over a third of the map is drawn smaller
        const total = top.reduce((a, r) => a + r.size, 0);
        if (level > 0 && g.size > total * 0.34) expect(g.note! & Note.NotToScale).toBe(Note.NotToScale);
        if (level === 0) expect(g.note! & Note.NotToScale).toBe(0);
      }
      expect([...seen].sort((x, y) => x - y)).toEqual(all);
    }
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
      ages: [],
    });
    expect(parseQuery('>1y <=6mo >2W <3天 >1年')!.ages).toEqual([
      ['>', 365],
      ['<=', 180],
      ['>', 14],
      ['<', 3],
      ['>', 365],
    ]);
    expect(() => parseQuery('>1h')).toThrow(BadSize);
    expect(parseQuery('. ..')!.words).toEqual(['.', '..']);
    expect(() => parseQuery('>')).toThrow(BadSize);
    expect(() => parseQuery('<5 apples')).not.toThrow();
    expect(() => parseQuery('<5apples')).toThrow(BadSize);
  });

  it('finds files by age, in the bands the legend shows', async () => {
    const t1 = ageTable([['>', 365]]);
    for (let b = 0; b < 256; b++) expect(t1[b]).toBe(ageBand(b, [365]) === 1);
    const t2 = ageTable([['<', 7]]);
    expect(t2[1]).toBe(true);
    expect(t2[AGE_UNKNOWN]).toBe(false);
    expect(t2[ageByte(30)]).toBe(false);
    const b = new MockBackend({}, { scanMs: 300 });
    await scanned(b);
    const old = (await b.search('>1y', 'disk'))!;
    const recent = (await b.search('<1y', 'disk'))!;
    const all = (await b.search('>=0d', 'disk'))!;
    expect(old.count).toBeGreaterThan(0);
    expect(recent.count).toBeGreaterThan(0);
    // files only, so the two halves add up (no folder is counted for its own sake)
    expect(old.count + recent.count).toBe(all.count);
    expect(old.bytes + recent.bytes).toBe(all.bytes);
    const hist = await b.ageStats(0, 'disk');
    expect(hist).toHaveLength(AGE_BINS);
    expect(bandTotals(hist, [365])[1]).toBe(old.bytes);
    expect(bandTotals(hist, [365])[0]).toBe(recent.bytes);
  });

  it('bins ages so that any choice of edges sums them as the tiles are coloured', () => {
    const edges = [3, 30, 1095];
    for (let b = 1; b < 256; b++) {
      const bins = new Array<number>(AGE_BINS).fill(0);
      bins[ageBin(b)] = 1;
      expect(bandTotals(bins, edges)[ageBand(b, edges)]).toBe(1);
    }
    expect(ageBin(AGE_UNKNOWN)).toBe(0);
    expect(ageBin(255)).toBe(AGE_EDGE_CHOICES.length + 1);
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
