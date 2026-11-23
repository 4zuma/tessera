/**
 * Mock backend: stands in for the Rust core until it is wired in. The whole tree
 * lives here; the UI only gets layout buffers and small JSON records, exactly as
 * with the real core (see docs/ipc.md).
 */
import { browserChoice, t } from '../../i18n.svelte';
import { encodeLayout, FREE, groupFolder, isGroupId, UNSCANNED } from '../codec';
import type {
  Backend,
  BackendEvents,
  CatStat,
  DeleteResult,
  Details,
  LayoutRequest,
  NavInfo,
  PlatformInfo,
  ScanProgress,
  ScanStarted,
  ShellAction,
  SizeMode,
  Unlisten,
  Volume,
} from '../types';
import { buildC, buildD, GB, MB, type Drive, type MNode } from './drive';
import { flagsOf, layoutDrive, sizeOf, visibleKids } from './layout';

const PROGRESS_MS = 100;

type Listener = (p: never) => void;

interface UndoEntry {
  moved: { id: number; from: number; idx: number }[];
}

export class MockBackend implements Backend {
  readonly platform: PlatformInfo;
  private builders: Record<string, () => Drive> = { 'C:\\': buildC, 'D:\\': buildD };
  private drives = new Map<string, Drive>();
  private drive: Drive | null = null;
  private listeners = new Map<string, Set<Listener>>();
  private rev = 1;
  private scanId = 0;
  private scanTimer: ReturnType<typeof setInterval> | null = null;
  private scanStartAt = 0;
  private order: MNode[] = [];
  private scanPtr = 0;
  private scanning = false;
  /** bytes freed by permanent deletes / taken by the growing download, per drive */
  private freed = new Map<string, number>();
  private grown = new Map<string, number>();
  private undos = new Map<number, UndoEntry>();
  private undoSeq = 1;
  private grow: { id: number; target: number; timer: ReturnType<typeof setInterval> } | null = null;

  private scanMs: number;

  constructor(platform: Partial<PlatformInfo> = {}, opts: { scanMs?: number } = {}) {
    this.scanMs = opts.scanMs ?? 4600;
    this.platform = { elevated: false, backdrop: 'none', accent: null, version: '1.2.1', core: false, native: false, ...browserChoice(), ...platform };
  }

  /* ------------------------------ helpers ------------------------------ */

  private emit<K extends keyof BackendEvents>(ev: K, payload: BackendEvents[K]) {
    const set = this.listeners.get(ev);
    if (set) for (const cb of set) (cb as (p: BackendEvents[K]) => void)(payload);
  }

  private get d(): Drive {
    if (!this.drive) throw new Error('no drive scanned yet');
    return this.drive;
  }

  private node(id: number): MNode | null {
    const d = this.drive;
    if (!d || id < 0 || id >= d.nodes.length) return null;
    return d.nodes[id];
  }

  private used(d: Drive) {
    return d.usedTarget - (this.freed.get(d.root) || 0) + (this.grown.get(d.root) || 0);
  }

  private free(d: Drive) {
    return d.capacity - this.used(d);
  }

  private attached(n: MNode | null): boolean {
    while (n) {
      if (n.id === 0) return true;
      if (n.parent < 0) return false;
      n = this.d.nodes[n.parent];
    }
    return false;
  }

  private isUnder(n: MNode | null, anc: number) {
    while (n) {
      if (n.id === anc) return true;
      n = n.parent >= 0 ? this.d.nodes[n.parent] : null;
    }
    return false;
  }

  private addUp(n: MNode | null, dd: number, dl: number) {
    while (n) {
      n.dT += dd;
      n.dL += dl;
      n = n.parent >= 0 ? this.d.nodes[n.parent] : null;
    }
  }

  private markSeen(n: MNode) {
    if (n.seen) return;
    n.seen = true;
    if (!n.kids) this.addUp(n, n.disk, n.logical);
  }

  private pathOf(id: number) {
    const parts: string[] = [];
    let n = this.node(id);
    while (n) {
      parts.unshift(n.name);
      n = n.parent >= 0 ? this.d.nodes[n.parent] : null;
    }
    return parts[0] + '\\' + parts.slice(1).join('\\');
  }

  private lockReason(n: MNode): string | null {
    const d = this.d;
    if (n.lock === 'sys') return t('lock.inUse');
    if (n.lock === 'acl') return t('lock.noAccess');
    // the same words as the core's reasons (src-tauri i18n.rs)
    if (n.lock === 'mine') return t('core.personal');
    if (n.lock === 'temp') return t('core.temp');
    if (n.lock === 'os' || n.id === 0) return t('mock.systemFolder');
    if (n.id === d.binId || n.id === d.binUserId) return t('lock.bin');
    return null;
  }

  private counts(n: MNode, acc = { files: 0, dirs: -1 }) {
    if (!n.seen) return acc;
    if (n.kids) {
      acc.dirs++;
      for (const k of n.kids) this.counts(this.d.nodes[k], acc);
    } else acc.files++;
    return acc;
  }

  /** topmost ids only: drop ids that sit inside another id of the list */
  private roots(ids: number[]) {
    return ids.filter(id => !ids.some(o => o !== id && this.isUnder(this.node(id), o)));
  }

  /* ------------------------------ Backend ------------------------------ */

  async listVolumes(): Promise<Volume[]> {
    return Object.keys(this.builders).map(root => {
      const d = this.drives.get(root);
      const cap = root === 'C:\\' ? 501.6 * GB : 931.5 * GB;
      const used = d ? this.used(d) : root === 'C:\\' ? 499.8 * GB : 612.4 * GB;
      return { root, label: root === 'C:\\' ? t('core.localDisk') : t('mock.dataLabel'), fs: 'NTFS', capacity: cap, free: cap - used };
    });
  }

  async scanStart(root: string): Promise<ScanStarted> {
    this.stopScan();
    let d = this.drives.get(root);
    if (!d) {
      const build = this.builders[root];
      if (!build) throw new Error(`unknown volume ${root}`);
      d = build();
      this.drives.set(root, d);
    }
    this.drive = d;
    for (const n of d.nodes) {
      n.seen = false;
      n.dT = 0;
      n.dL = 0;
      n.rk = 0;
    }
    d.nodes[0].seen = true;
    this.order = d.nodes.filter(n => this.attached(n)).sort((a, b) => a.t - b.t);
    this.scanPtr = 0;
    this.scanning = true;
    this.scanStartAt = performance.now();
    const scanId = ++this.scanId;
    this.rev++;
    this.scanTimer = setInterval(() => this.tickScan(scanId), PROGRESS_MS);
    queueMicrotask(() => this.tickScan(scanId));
    return { scanId, rootId: 0 };
  }

  private stopScan() {
    if (this.scanTimer) clearInterval(this.scanTimer);
    this.scanTimer = null;
    this.scanning = false;
  }

  private tickScan(scanId: number) {
    if (scanId !== this.scanId || !this.drive) return;
    const d = this.drive;
    const elapsed = performance.now() - this.scanStartAt;
    const t = Math.min(1, elapsed / this.scanMs);
    while (this.scanPtr < this.order.length && this.order[this.scanPtr].t <= t) this.markSeen(this.order[this.scanPtr++]);
    if (t >= 1) {
      for (const n of this.order) this.markSeen(n);
      this.scanPtr = this.order.length;
      this.stopScan();
    }
    let files = 0;
    let dirs = 0;
    for (let i = 0; i < this.scanPtr; i++) this.order[i].kids ? dirs++ : files++;
    this.rev++;
    const p: ScanProgress = {
      scanId,
      files,
      dirs,
      scanned: d.nodes[0].dT,
      used: this.used(d),
      elapsedMs: Math.round(elapsed),
      done: !this.scanning,
      rev: this.rev,
      errors: 0,
    };
    this.emit('scan-progress', p);
  }

  async scanCancel() {
    this.stopScan();
  }

  async layout(req: LayoutRequest): Promise<ArrayBuffer> {
    const d = this.d;
    const view = this.node(req.view) && this.attached(this.node(req.view)) ? req.view : 0;
    const unscanned = this.scanning ? Math.max(0, this.used(d) - d.nodes[0].dT) : null;
    const L = layoutDrive({ drive: d, view, width: req.width, height: req.height, sizeMode: req.sizeMode, style: req.style, free: this.free(d), unscanned });
    return encodeLayout({ view, rev: this.rev, bytesPerPx: L.bytesPerPx, rects: L.rects });
  }

  async details(id: number, view: number, mode: SizeMode): Promise<Details | null> {
    if (!this.drive) return null;
    const d = this.drive;
    const blank = { disk: 0, logical: 0, files: 0, dirs: 0, modified: null, cat: 255, flags: 0, lockReason: null, hint: null };
    if (id === FREE) {
      const f = this.free(d);
      return { ...blank, id, kind: 'free', name: t('free'), path: d.root, size: f, share: f / d.capacity, hint: t('core.freeHint') };
    }
    if (id === UNSCANNED) {
      const u = Math.max(0, this.used(d) - d.nodes[0].dT);
      return { ...blank, id, kind: 'unscanned', name: t('unscanned'), path: d.root, size: u, share: u / d.capacity, hint: t('core.unscannedHint') };
    }
    if (isGroupId(id)) {
      const p = this.node(groupFolder(id));
      if (!p) return null;
      return { ...blank, id, kind: 'group', name: t('core.group'), path: this.pathOf(p.id), size: 0, share: 0, hint: t('core.groupHint') };
    }
    const n = this.node(id);
    if (!n || !this.attached(n)) return null;
    const p = n.parent >= 0 ? d.nodes[n.parent] : null;
    const s = sizeOf(n, mode);
    const cnt = n.kids ? this.counts(n) : null;
    void view;
    return {
      id,
      kind: n.kids ? 'dir' : 'file',
      name: n.id === 0 ? `${d.label} (${d.root.slice(0, 2)})` : n.name,
      path: this.pathOf(id),
      size: s,
      disk: n.kids ? n.dT : n.disk,
      logical: n.kids ? n.dL : n.logical,
      share: p ? s / Math.max(1, sizeOf(p, mode)) : s / d.capacity,
      files: cnt ? cnt.files : 0,
      dirs: cnt ? Math.max(0, cnt.dirs) : 0,
      modified: Date.UTC(2026, 10, 5, 9) - n.age * 864e5,
      cat: n.kids ? 255 : n.cat,
      flags: flagsOf(d, n),
      lockReason: this.lockReason(n),
      hint:
        n.name === 'hiberfil.sys'
          ? t('core.hiberfil')
          : n.name === 'pagefile.sys'
            ? t('core.pagefile')
            : n.name === 'Windows.old'
              ? t('core.windowsOld')
              : n.cloud
                ? t('mock.cloud')
                : null,
    };
  }

  async nav(id: number): Promise<NavInfo> {
    const d = this.d;
    const crumbs: { id: number; name: string }[] = [];
    let n = this.node(id);
    while (n) {
      crumbs.unshift({ id: n.id, name: n.id === 0 ? d.root.slice(0, 2) : n.name });
      n = n.parent >= 0 ? d.nodes[n.parent] : null;
    }
    // "up" skips folders that would only show a single sub-folder
    let p = this.node(id)?.parent ?? -1;
    while (p > 0) {
      const v = visibleKids(d, d.nodes[p], 'disk');
      if (v.length === 1 && v[0].kids) p = d.nodes[p].parent;
      else break;
    }
    return { crumbs, up: p >= 0 ? p : null };
  }

  async catStats(view: number, mode: SizeMode): Promise<CatStat[]> {
    const d = this.d;
    const acc = new Map<number, number>();
    const walk = (n: MNode) => {
      if (!n.seen) return;
      if (n.kids) for (const k of n.kids) walk(d.nodes[k]);
      else acc.set(n.cat, (acc.get(n.cat) || 0) + (mode === 'disk' ? n.disk : n.logical));
    };
    const v = this.node(view);
    if (v) walk(v);
    return [...acc].filter(([, b]) => b > 0).map(([cat, bytes]) => ({ cat, bytes })).sort((a, b) => b.bytes - a.bytes);
  }

  async recycle(ids: number[]): Promise<DeleteResult> {
    const d = this.d;
    const failed: DeleteResult['failed'] = [];
    const moved: UndoEntry['moved'] = [];
    let bytes = 0;
    const bin = d.nodes[d.binUserId];
    for (const id of this.roots(ids)) {
      const n = this.node(id);
      const why = n ? this.lockReason(n) : t('core.noItem');
      if (!n || why) {
        failed.push({ id, reason: why || t('core.noItem') });
        continue;
      }
      const p = d.nodes[n.parent];
      moved.push({ id, from: n.parent, idx: p.kids!.indexOf(id) });
      const dT = n.dT;
      const dL = n.dL;
      p.kids!.splice(p.kids!.indexOf(id), 1);
      this.addUp(p, -dT, -dL);
      // same volume: the bytes move into $Recycle.Bin, nothing is freed yet
      n.parent = bin.id;
      bin.kids!.push(id);
      this.addUp(bin, dT, dL);
      bytes += dT;
    }
    let undo: number | null = null;
    if (moved.length) {
      undo = this.undoSeq++;
      this.undos.set(undo, { moved });
      this.rev++;
    }
    return { ok: moved.map(m => m.id), failed, bytes, undo, rev: this.rev };
  }

  async restore(token: number): Promise<DeleteResult> {
    const d = this.d;
    const u = this.undos.get(token);
    const ok: number[] = [];
    let bytes = 0;
    if (u) {
      this.undos.delete(token);
      const bin = d.nodes[d.binUserId];
      for (const m of u.moved.slice().reverse()) {
        const n = d.nodes[m.id];
        if (n.parent !== bin.id) continue;
        bin.kids!.splice(bin.kids!.indexOf(m.id), 1);
        this.addUp(bin, -n.dT, -n.dL);
        n.parent = m.from;
        const p = d.nodes[m.from];
        p.kids!.splice(Math.min(m.idx, p.kids!.length), 0, m.id);
        this.addUp(p, n.dT, n.dL);
        ok.push(m.id);
        bytes += n.dT;
      }
      this.rev++;
    }
    return { ok, failed: [], bytes, undo: null, rev: this.rev };
  }

  async purge(ids: number[]): Promise<DeleteResult> {
    const d = this.d;
    const failed: DeleteResult['failed'] = [];
    const ok: number[] = [];
    let bytes = 0;
    for (const id of this.roots(ids)) {
      const n = this.node(id);
      const why = n ? this.lockReason(n) : t('core.noItem');
      if (!n || why || !this.attached(n)) {
        failed.push({ id, reason: why || t('core.noItem') });
        continue;
      }
      const p = d.nodes[n.parent];
      p.kids!.splice(p.kids!.indexOf(id), 1);
      this.addUp(p, -n.dT, -n.dL);
      n.parent = -2;
      bytes += n.dT;
      ok.push(id);
    }
    this.freed.set(d.root, (this.freed.get(d.root) || 0) + bytes);
    this.rev++;
    return { ok, failed, bytes, undo: null, rev: this.rev };
  }

  async emptyRecycleBin(): Promise<DeleteResult> {
    const d = this.d;
    const bin = d.nodes[d.binUserId];
    const ids = bin.kids!.slice();
    let bytes = 0;
    for (const id of ids) {
      const n = d.nodes[id];
      bytes += n.dT;
      this.addUp(bin, -n.dT, -n.dL);
      n.parent = -2;
    }
    bin.kids!.length = 0;
    this.undos.clear();
    this.freed.set(d.root, (this.freed.get(d.root) || 0) + bytes);
    this.rev++;
    return { ok: ids, failed: [], bytes, undo: null, rev: this.rev };
  }

  async reveal() {
    /* the preview has no Explorer to open */
  }

  async shellMenu(): Promise<ShellAction> {
    /* the preview has no shell menu */
    return 'none';
  }

  async relaunchElevated() {
    /* nothing to relaunch in the preview */
  }

  async on<K extends keyof BackendEvents>(event: K, cb: (payload: BackendEvents[K]) => void): Promise<Unlisten> {
    let set = this.listeners.get(event);
    if (!set) this.listeners.set(event, (set = new Set()));
    set.add(cb as Listener);
    return () => set!.delete(cb as Listener);
  }

  /** live-update demo: a download that grows in Downloads, pushed every 500 ms */
  demo = {
    startDownload: (): boolean => {
      const d = this.drive;
      if (!d || d.downloadsId < 0 || this.scanning || this.grow) return false;
      const dl = d.nodes[d.downloadsId];
      const n: MNode = { id: d.nodes.length, name: 'Win11_25H2_Chinese_Simplified_x64.iso.crdownload', parent: dl.id, kids: null, disk: 0, logical: 0, cat: 4, age: 0, lock: '', lockDeep: '', cloud: false, seen: true, t: 1, dT: 0, dL: 0, rk: 0 };
      d.nodes.push(n);
      dl.kids!.push(n.id);
      const target = Math.min(5.8 * GB, Math.max(200 * MB, this.free(d) - 160 * MB));
      const timer = setInterval(() => {
        if (!this.attached(n)) return this.stopGrow();
        const add = Math.min(96 * MB, target - n.disk);
        n.disk += add;
        n.logical += add;
        this.addUp(n, add, add);
        this.grown.set(d.root, (this.grown.get(d.root) || 0) + add);
        const done = n.disk >= target - 1;
        if (done) n.name = n.name.replace('.crdownload', '');
        this.rev++;
        this.emit('tree-changed', { rev: this.rev, bytesDelta: add });
        if (done) this.stopGrow();
      }, 500);
      this.grow = { id: n.id, target, timer };
      return true;
    },
  };

  private stopGrow() {
    if (this.grow) clearInterval(this.grow.timer);
    this.grow = null;
  }
}
