/**
 * A synthetic but realistic drive for the mock backend: a 501.6 GB C: that is nearly
 * full (the user's real situation), and a calmer D: data drive. Deterministic, so
 * screenshots and tests are stable.
 */
import { Cat, categoryOf } from '../categories';

export const KB = 1024;
export const MB = KB * 1024;
export const GB = MB * 1024;

export type Lock = '' | 'os' | 'acl' | 'sys';

export interface MNode {
  id: number;
  name: string;
  parent: number; // -1 root, -2 deleted
  kids: number[] | null;
  disk: number;
  logical: number;
  cat: number;
  /** days since last modification */
  age: number;
  lock: Lock;
  lockDeep: Lock;
  cloud: boolean;
  /** discovered by the (simulated) scan */
  seen: boolean;
  /** discovery time in [0, 1] */
  t: number;
  /** scanned totals (disk / logical) */
  dT: number;
  dL: number;
  /** size this node was last ranked at (sibling-order hysteresis) */
  rk: number;
}

export interface Drive {
  root: string;
  label: string;
  fs: string;
  capacity: number;
  usedTarget: number;
  nodes: MNode[];
  binId: number;
  binUserId: number;
  downloadsId: number;
}

export function mulberry32(seed: number) {
  let a = seed;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

interface FillOpts {
  ext: string[];
  cat?: number;
  age?: [number, number];
  compress?: number;
  cloud?: boolean;
  lock?: Lock;
  sigma?: number;
  name?: (i: number, ext: string) => string;
  dirs?: DirsOpts;
}
interface DirsOpts {
  n: number;
  sigma?: number;
  name: (i: number) => string;
  next?: DirsOpts;
}

class Builder {
  N: MNode[] = [];
  rnd: () => number;
  constructor(seed: number) {
    this.rnd = mulberry32(seed);
  }
  pick = <T>(a: readonly T[]): T => a[Math.floor(this.rnd() * a.length)];
  int = (n: number) => Math.floor(this.rnd() * n);
  hex = (n: number) => {
    let s = '';
    for (let i = 0; i < n; i++) s += '0123456789abcdef'[this.int(16)];
    return s;
  };
  lognorm(s: number) {
    let u = 0;
    let v = 0;
    while (u === 0) u = this.rnd();
    while (v === 0) v = this.rnd();
    return Math.exp(s * Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * v));
  }
  mk(parent: MNode | null, name: string, isDir: boolean, o: { cat?: number; age?: number; lock?: Lock; lockDeep?: Lock; cloud?: boolean } = {}) {
    const n: MNode = {
      id: this.N.length,
      name,
      parent: parent ? parent.id : -1,
      kids: isDir ? [] : null,
      disk: 0,
      logical: 0,
      cat: o.cat ?? Cat.Other,
      age: o.age ?? 30,
      lock: o.lock || (parent && parent.lockDeep ? parent.lockDeep : ''),
      lockDeep: o.lockDeep || (parent ? parent.lockDeep : ''),
      cloud: !!o.cloud,
      seen: false,
      t: 0,
      dT: 0,
      dL: 0,
      rk: 0,
    };
    if (parent) parent.kids!.push(n.id);
    this.N.push(n);
    return n;
  }
  D = (p: MNode | null, name: string, o: { lock?: Lock; lockDeep?: Lock } = {}) => this.mk(p, name, true, o);
  F(p: MNode, name: string, bytes: number, o: { cat?: number; age?: number; compress?: number; cloud?: boolean; lock?: Lock } = {}) {
    const n = this.mk(p, name, false, { ...o, cat: o.cat ?? categoryOf(name, bytes) });
    const round4k = (b: number) => Math.ceil(b / 4096) * 4096;
    n.logical = Math.round(bytes);
    n.disk = o.cloud ? 4096 : o.compress ? round4k(bytes * o.compress) : round4k(bytes);
    return n;
  }
  split(total: number, n: number, sigma: number) {
    const w: number[] = [];
    let s = 0;
    for (let i = 0; i < n; i++) {
      const x = this.lognorm(sigma);
      w.push(x);
      s += x;
    }
    return w.map(x => (total * x) / s);
  }
  ageIn = (r: [number, number]) => r[0] + (r[1] - r[0]) * Math.pow(this.rnd(), 1.4);
  WORDS = ['core', 'runtime', 'shader', 'asset', 'index', 'data', 'backup', 'export', 'render', 'clip', 'frame', 'model', 'texture', 'sound', 'patch', 'update', 'setup', 'config', 'profile', 'session', 'thumb', 'preview', 'chunk', 'segment', 'blob', 'pack', 'delta', 'journal', 'store', 'bundle'];
  genName = (ext: string) => `${this.pick(this.WORDS)}${this.pick(['_', '-', ''])}${this.pick(this.WORDS)}${this.rnd() < 0.5 ? '_' + this.hex(4) : ''}.${ext}`;

  /** random files (and optional nested folders) whose sizes add up to `bytes` */
  fill(dir: MNode, bytes: number, count: number, P: FillOpts) {
    if (P.dirs) {
      const k = P.dirs.n;
      const parts = this.split(bytes, k, P.dirs.sigma ?? 1.1);
      let left = count;
      for (let i = 0; i < k; i++) {
        const c = i === k - 1 ? Math.max(1, left) : Math.max(1, Math.round((count * parts[i]) / bytes));
        left -= c;
        const sub = this.D(dir, P.dirs.name(i), { lock: P.lock });
        this.fill(sub, parts[i], c, { ...P, dirs: P.dirs.next });
      }
      return;
    }
    const sizes = this.split(bytes, count, P.sigma ?? 1.7);
    for (let i = 0; i < count; i++) {
      const ext = this.pick(P.ext);
      this.F(dir, P.name ? P.name(i, ext) : this.genName(ext), sizes[i], {
        cat: P.cat,
        age: this.ageIn(P.age || [10, 600]),
        compress: P.compress,
        cloud: P.cloud,
        lock: P.lock,
      });
    }
  }
}

function finish(b: Builder, d: Omit<Drive, 'nodes'>): Drive {
  const drive: Drive = { ...d, nodes: b.N };
  schedule(drive, b.rnd);
  return drive;
}

/**
 * Simulated parallel scan: 8 workers enumerate subtrees; each node gets a discovery
 * time t in [0, 1]. The volume's used bytes are known up front (as on NTFS), so the
 * unscanned remainder can be drawn as one tile from the start.
 */
function schedule(drive: Drive, rnd: () => number) {
  const N = drive.nodes;
  const root = N[0];
  const tasks: number[] = [];
  for (const c of root.kids!) {
    const n = N[c];
    if (n.kids && n.kids.length > 2) {
      n.t = 0;
      for (const g of n.kids) tasks.push(g);
    } else tasks.push(c);
  }
  const count = (id: number): number => {
    let s = 1;
    const n = N[id];
    if (n.kids) for (const k of n.kids) s += count(k);
    return s;
  };
  const sized = tasks.map(id => ({ id, c: count(id) })).sort((a, b) => b.c - a.c);
  const lanes = Array.from({ length: 8 }, () => ({ load: 0, list: [] as number[] }));
  for (const t of sized) {
    lanes.sort((a, b) => a.load - b.load);
    lanes[0].list.push(t.id);
    lanes[0].load += t.c;
  }
  const maxLoad = Math.max(...lanes.map(l => l.load));
  for (const lane of lanes) {
    let i = 0;
    const walk = (id: number) => {
      const n = N[id];
      n.t = Math.pow((i++ + rnd() * 0.5) / maxLoad, 0.55);
      if (n.kids) for (const k of n.kids) walk(k);
    };
    for (const id of lane.list) walk(id);
  }
  root.t = 0;
}

export function buildC(): Drive {
  const b = new Builder(20261105);
  const { D, pick, int, hex } = b;
  const F = b.F.bind(b);
  const fill = b.fill.bind(b);
  const pad2 = (n: number) => String(n).padStart(2, '0');
  const CAPACITY = 501.6 * GB;
  const USED_TARGET = 499.8 * GB;

  const root = D(null, 'C:');
  const COMP = ['servicingstack', 'directx-graphics', 'edge-webview', 'onecore-coremessaging', 'shell-experiencehost', 'windowsupdate-client', 'media-foundation', 'xaml-runtime', 'wcf-framework', 'networking-mpssvc', 'ie-htmlrendering', 'printing-spooler', 'search-core', 'speech-runtime', 'tcpip', 'defender-engine'];
  const winsxsName = () => `amd64_microsoft-windows-${pick(COMP)}_31bf3856ad364e35_10.0.26100.${1000 + int(3000)}_none_${hex(16)}`;

  const win = D(root, 'Windows', { lock: 'os', lockDeep: 'os' });
  fill(D(win, 'WinSxS'), 11.8 * GB, 1900, { ext: ['dll', 'dll', 'mui', 'manifest', 'exe', 'cat'], age: [30, 700], compress: 0.62, sigma: 1.9, dirs: { n: 150, name: winsxsName } });
  const sys32 = D(win, 'System32');
  const drvs = ['nv_dispi', 'iigd_dch', 'netwtw10', 'rtux64w10', 'hdxrt', 'intcaudiobus', 'ibtusb', 'usbxhci', 'nvhda', 'iastorvd'];
  fill(D(D(sys32, 'DriverStore'), 'FileRepository'), 3.1 * GB, 420, { ext: ['dll', 'sys', 'exe', 'cat'], cat: Cat.App, age: [20, 400], dirs: { n: 48, sigma: 1.6, name: () => `${pick(drvs)}.inf_amd64_${hex(16)}` } });
  fill(sys32, 4.6 * GB, 900, { ext: ['dll', 'dll', 'dll', 'exe', 'mui', 'sys'], age: [20, 500] });
  fill(D(win, 'SysWOW64'), 1.9 * GB, 420, { ext: ['dll', 'exe', 'mui'], age: [30, 600] });
  fill(D(win, 'Installer'), 6.4 * GB, 140, { ext: ['msi', 'msp', 'msp'], age: [100, 900], name: (_i, e) => `${hex(7)}.${e}` });
  fill(D(D(win, 'SoftwareDistribution'), 'Download'), 4.2 * GB, 80, { ext: ['cab', 'esd', 'cab'], age: [3, 40], dirs: { n: 24, name: () => hex(32) } });
  fill(D(win, 'Temp'), 1.1 * GB, 260, { ext: ['tmp', 'log', 'etl'], cat: Cat.Cache, age: [0, 40] });
  fill(D(win, 'Fonts'), 0.9 * GB, 180, { ext: ['ttf'], age: [200, 800] });
  fill(D(win, 'assembly'), 1.6 * GB, 280, { ext: ['dll'], age: [100, 700], dirs: { n: 12, name: i => ['GAC_MSIL', 'GAC_64', 'GAC_32', 'NativeImages_v4.0.30319_64', 'NativeImages_v4.0.30319_32'][i % 5] + (i > 4 ? '_' + i : '') } });
  fill(D(win, 'Microsoft.NET'), 1.2 * GB, 260, { ext: ['dll', 'exe'], age: [100, 700] });
  fill(D(win, 'Logs'), 0.6 * GB, 110, { ext: ['etl', 'log', 'evtx'], age: [0, 120] });
  F(win, 'MEMORY.DMP', 2.1 * GB, { age: 46, cat: Cat.System });

  const old = D(root, 'Windows.old');
  fill(D(old, 'Windows'), 14.2 * GB, 900, { ext: ['dll', 'exe', 'mui', 'cab'], age: [700, 760], dirs: { n: 30, name: i => ['System32', 'SysWOW64', 'WinSxS', 'Installer', 'assembly', 'Fonts', 'servicing', 'Microsoft.NET'][i % 8] + (i > 7 ? '_' + i : '') } });
  fill(D(old, 'Program Files'), 4.1 * GB, 300, { ext: ['dll', 'exe', 'pak'], age: [700, 760], dirs: { n: 10, name: i => ['Microsoft Office', 'Common Files', 'Windows Defender', 'Internet Explorer', 'NVIDIA Corporation', 'Realtek', 'Intel', 'Windows Mail', 'MSBuild', 'Reference Assemblies'][i] } });
  fill(D(old, 'Users'), 6.3 * GB, 400, { ext: ['jpg', 'mp4', 'docx', 'pdf', 'zip', 'tmp'], age: [720, 1100], dirs: { n: 6, name: i => ['AppData', 'Documents', 'Downloads', 'Desktop', 'Pictures', 'Videos'][i] } });

  const pf = D(root, 'Program Files', { lock: 'os' });
  const adobe = D(pf, 'Adobe');
  fill(D(adobe, 'Adobe Photoshop 2025'), 4.6 * GB, 320, { ext: ['dll', 'exe', 'pak', 'psd'], cat: Cat.App, age: [60, 200] });
  fill(D(adobe, 'Adobe Premiere Pro 2025'), 6.9 * GB, 420, { ext: ['dll', 'exe', 'pak', 'mp4'], cat: Cat.App, age: [60, 200] });
  fill(D(adobe, 'Adobe Creative Cloud'), 1.1 * GB, 120, { ext: ['dll', 'exe'], age: [10, 90] });
  fill(D(D(D(pf, 'Microsoft Office'), 'root'), 'Office16'), 4.8 * GB, 400, { ext: ['dll', 'exe', 'mui'], age: [10, 60] });
  fill(D(pf, 'NVIDIA Corporation'), 3.1 * GB, 160, { ext: ['dll', 'exe', 'bin'], cat: Cat.App, age: [10, 80] });
  const jb = D(pf, 'JetBrains');
  fill(D(jb, 'RustRover 2025.2'), 3.0 * GB, 260, { ext: ['jar', 'dll', 'exe'], cat: Cat.App, age: [20, 70] });
  fill(D(jb, 'PyCharm 2025.2'), 3.2 * GB, 260, { ext: ['jar', 'dll', 'exe'], cat: Cat.App, age: [20, 70] });
  fill(D(D(pf, 'Docker'), 'Docker'), 2.4 * GB, 120, { ext: ['exe', 'dll', 'iso'], cat: Cat.App, age: [5, 40] });
  fill(D(D(D(D(pf, 'Unity'), 'Hub'), 'Editor'), '2022.3.48f1'), 9.2 * GB, 520, { ext: ['dll', 'exe', 'bundle', 'assets'], cat: Cat.App, age: [150, 400] });
  fill(D(pf, 'Blender Foundation'), 1.1 * GB, 140, { ext: ['dll', 'exe', 'pyd'], age: [90, 300] });
  const wapps = D(pf, 'WindowsApps', { lock: 'acl', lockDeep: 'acl' });
  fill(wapps, 7.8 * GB, 40, { ext: ['dll', 'exe'], age: [5, 200], lock: 'acl', dirs: { n: 8, name: () => `Microsoft.${pick(['Photos', 'GamingApp', 'WindowsTerminal', 'XboxApp', 'Paint', 'Clipchamp', 'MinecraftUWP', 'ScreenSketch'])}_${int(30)}.${int(9000)}.0.0_x64__8wekyb3d8bbwe` } });
  fill(D(pf, 'Git'), 0.4 * GB, 120, { ext: ['exe', 'dll'], age: [30, 120] });
  fill(D(pf, 'Common Files'), 1.2 * GB, 160, { ext: ['dll', 'exe'], age: [30, 600] });

  const pf86 = D(root, 'Program Files (x86)', { lock: 'os' });
  const steam = D(pf86, 'Steam');
  const common = D(D(steam, 'steamapps'), 'common');
  const cp = D(common, 'Cyberpunk 2077');
  fill(D(D(D(cp, 'archive'), 'pc'), 'content'), 58.6 * GB, 64, { ext: ['archive'], cat: Cat.Game, age: [90, 300], sigma: 1.1, name: i => `basegame_${i + 1}_${pick(['engine', 'gamedata', 'textures', 'audio_general', 'lang_zh', 'nightcity', 'vehicles', 'characters'])}.archive` });
  fill(D(cp, 'bin'), 1.6 * GB, 60, { ext: ['dll', 'exe'], cat: Cat.App, age: [90, 300] });
  fill(D(cp, 'r6'), 1.9 * GB, 80, { ext: ['bk2', 'cache'], cat: Cat.Game, age: [90, 300] });
  fill(D(D(D(common, 'Counter-Strike Global Offensive'), 'game'), 'csgo'), 27.0 * GB, 180, { ext: ['vpk'], cat: Cat.Game, age: [2, 30], sigma: 0.9, name: i => `pak01_${String(i).padStart(3, '0')}.vpk` });
  fill(D(common, 'Stardew Valley'), 0.6 * GB, 90, { ext: ['bundle', 'dll'], cat: Cat.Game, age: [300, 700] });
  fill(D(steam, 'appcache'), 1.2 * GB, 80, { ext: ['cache', 'tmp'], cat: Cat.Cache, age: [0, 20] });
  fill(D(steam, 'logs'), 0.2 * GB, 40, { ext: ['log'], age: [0, 30] });
  fill(D(pf86, 'Microsoft Visual Studio'), 3.4 * GB, 300, { ext: ['dll', 'exe', 'lib'], age: [60, 300] });
  fill(D(pf86, 'Common Files'), 0.9 * GB, 120, { ext: ['dll'], age: [100, 900] });

  const pd = D(root, 'ProgramData', { lock: 'os' });
  fill(D(pd, 'Package Cache'), 6.8 * GB, 140, { ext: ['msi', 'cab', 'exe'], age: [60, 900], dirs: { n: 40, name: () => `{${hex(8)}-${hex(4)}-${hex(4)}-${hex(4)}-${hex(12)}}` } });
  const pdm = D(pd, 'Microsoft');
  fill(D(pdm, 'Windows Defender'), 1.3 * GB, 60, { ext: ['dll', 'cab', 'db'], age: [0, 10] });
  fill(D(pdm, 'Search'), 2.2 * GB, 30, { ext: ['db', 'log'], cat: Cat.Cache, age: [0, 3] });
  fill(D(D(pdm, 'Windows'), 'WER'), 1.4 * GB, 40, { ext: ['dmp', 'log'], age: [5, 200] });
  fill(D(pd, 'NVIDIA Corporation'), 1.6 * GB, 80, { ext: ['exe', 'cab', 'log'], age: [10, 200] });
  fill(D(pd, 'chocolatey'), 1.1 * GB, 160, { ext: ['exe', 'zip', 'log'], age: [30, 400] });
  fill(D(pd, 'Adobe'), 2.9 * GB, 90, { ext: ['zip', 'cab', 'dll'], age: [30, 300] });

  const users = D(root, 'Users', { lock: 'os' });
  const me = D(users, 'User', { lock: 'os' });
  const appdata = D(me, 'AppData');
  const local = D(appdata, 'Local');
  F(D(D(D(local, 'Docker'), 'wsl'), 'disk'), 'docker_data.vhdx', 41.5 * GB, { age: 0 });
  const pkgs = D(local, 'Packages');
  F(D(D(pkgs, 'CanonicalGroupLimited.Ubuntu24.04LTS_79rhkp1fndgsc'), 'LocalState'), 'ext4.vhdx', 18.6 * GB, { age: 1 });
  fill(pkgs, 1.2 * GB, 120, { ext: ['dat', 'db', 'tmp'], age: [0, 200], dirs: { n: 14, name: () => `Microsoft.${pick(['WindowsTerminal', 'Windows.Photos', 'GamingApp', 'XboxApp', 'ScreenSketch', 'Paint', 'StorePurchaseApp'])}_8wekyb3d8bbwe` } });
  fill(D(local, 'Temp'), 6.8 * GB, 900, { ext: ['tmp', 'tmp', 'log', 'zip', 'exe', 'cab'], cat: Cat.Cache, age: [0, 60], dirs: { n: 40, sigma: 1.4, name: () => pick([`${hex(8)}-${hex(4)}`, `nsx${hex(4)}.tmp`, `chrome_drag${int(9999)}`, `vscode-${hex(6)}`, `msedge_url_fetcher_${int(99999)}`]) } });
  fill(D(local, 'npm-cache'), 3.4 * GB, 700, { ext: ['tmp'], cat: Cat.Cache, age: [0, 200], name: () => hex(40), dirs: { n: 1, name: () => '_cacache', next: { n: 1, name: () => 'content-v2', next: { n: 64, name: () => hex(2) } } } });
  fill(D(D(local, 'pip'), 'cache'), 2.1 * GB, 300, { ext: ['whl', 'tmp'], cat: Cat.Cache, age: [2, 300] });
  fill(D(local, 'JetBrains'), 4.0 * GB, 380, { ext: ['dat', 'cache', 'jar'], cat: Cat.Cache, age: [0, 60], dirs: { n: 3, name: i => ['RustRover2025.2', 'PyCharm2025.2', 'RustRover2025.1'][i] } });
  fill(D(D(D(local, 'Google'), 'Chrome'), 'User Data'), 3.6 * GB, 400, { ext: ['db', 'cache', 'tmp'], cat: Cat.Cache, age: [0, 30] });
  fill(D(D(D(local, 'Microsoft'), 'Edge'), 'User Data'), 1.4 * GB, 200, { ext: ['db', 'cache'], cat: Cat.Cache, age: [0, 60] });
  fill(D(D(local, 'NVIDIA'), 'DXCache'), 1.8 * GB, 300, { ext: ['cache'], cat: Cat.Cache, age: [0, 90] });
  fill(D(local, 'CrashDumps'), 2.4 * GB, 9, { ext: ['dmp'], age: [10, 300], name: (_i, e) => `${pick(['Cyberpunk2077.exe', 'Code.exe', 'Unity.exe', 'explorer.exe', 'steamwebhelper.exe'])}.${int(30000)}.${e}` });
  const progs = D(local, 'Programs');
  fill(D(progs, 'Microsoft VS Code'), 0.6 * GB, 140, { ext: ['dll', 'exe', 'js', 'node'], age: [3, 30] });
  fill(D(progs, 'Python'), 1.1 * GB, 280, { ext: ['pyd', 'dll', 'exe', 'whl'], age: [60, 300] });
  fill(D(D(local, 'Unity'), 'cache'), 2.7 * GB, 300, { ext: ['bundle', 'cache'], cat: Cat.Cache, age: [20, 300] });
  const roam = D(appdata, 'Roaming');
  fill(D(roam, 'Code'), 1.6 * GB, 300, { ext: ['json', 'db', 'cache', 'log'], age: [0, 60], dirs: { n: 4, name: i => ['User', 'CachedData', 'logs', 'CachedExtensionVSIXs'][i] } });
  fill(D(roam, 'discord'), 0.9 * GB, 140, { ext: ['cache', 'dll', 'tmp'], cat: Cat.Cache, age: [0, 40] });
  fill(D(roam, 'Adobe'), 2.1 * GB, 160, { ext: ['cache', 'db', 'psd'], age: [10, 300] });
  fill(D(roam, 'Tencent'), 1.2 * GB, 120, { ext: ['db', 'dat', 'log'], age: [1, 100] });
  fill(D(appdata, 'LocalLow'), 1.3 * GB, 160, { ext: ['dat', 'cache', 'log'], age: [3, 400] });

  const dl = D(me, 'Downloads');
  F(dl, 'Win11_24H2_Chinese_Simplified_x64.iso', 5.6 * GB, { age: 210 });
  F(dl, 'ubuntu-24.04.1-desktop-amd64.iso', 5.9 * GB, { age: 340 });
  F(dl, 'cuda_12.6.2_560.94_windows.exe', 3.2 * GB, { age: 280 });
  F(dl, 'DaVinci_Resolve_19.1_Windows.zip', 3.9 * GB, { age: 150 });
  F(dl, 'VisualStudioSetup.exe', 4.2 * MB, { age: 400 });

  const vid = D(me, 'Videos');
  fill(D(vid, 'Captures'), 8.0 * GB, 120, { ext: ['mp4'], age: [5, 700], name: () => `${pick(['Cyberpunk 2077', 'Counter-Strike 2', 'Stardew Valley'])} 2025-${pad2(1 + int(12))}-${pad2(1 + int(28))} ${pad2(int(24))}-${pad2(int(60))}-${pad2(int(60))}.mp4` });
  const obs = D(vid, 'OBS');
  for (const [d, s] of [['2025-03-02 20-11-43', 4.4], ['2025-04-19 21-36-02', 3.7], ['2025-06-07 19-02-58', 2.9], ['2025-08-14 21-03-55', 3.1], ['2025-09-30 22-47-10', 2.3], ['2025-10-26 20-15-31', 1.8]] as const)
    F(obs, `${d}.mkv`, s * GB, { age: Math.max(10, (Date.UTC(2026, 10, 5) - Date.parse(d.slice(0, 10))) / 864e5) });
  fill(vid, 5.3 * GB, 40, { ext: ['mp4', 'mov'], age: [30, 1500] });

  const pic = D(me, 'Pictures');
  fill(D(pic, 'Screenshots'), 2.1 * GB, 900, { ext: ['png'], age: [0, 700], sigma: 0.9, name: () => `屏幕截图 2025-${pad2(1 + int(12))}-${pad2(1 + int(28))} ${pad2(int(24))}${pad2(int(60))}${pad2(int(60))}.png` });
  fill(D(pic, 'Photos'), 8.6 * GB, 360, { ext: ['arw', 'arw', 'jpg'], age: [200, 1400], sigma: 0.5, name: (_i, e) => `DSC0${1000 + int(8999)}.${e.toUpperCase()}`, dirs: { n: 6, name: i => ['2021 京都', '2022 大理', '2023 冰岛', '2024 青岛', '2024 年会', '2025 上海'][i] } });
  fill(pic, 1.7 * GB, 140, { ext: ['jpg', 'png', 'psd', 'webp'], age: [10, 1200] });

  const docs = D(me, 'Documents');
  const wx = D(D(D(docs, 'WeChat Files'), `wxid_${hex(14)}`), 'FileStorage');
  const month = () => `2025-${pad2(1 + int(12))}`;
  fill(D(wx, 'Video'), 6.1 * GB, 260, { ext: ['mp4'], age: [10, 900], name: () => `${hex(32)}.mp4`, dirs: { n: 14, name: month } });
  fill(D(wx, 'File'), 3.9 * GB, 400, { ext: ['pdf', 'docx', 'xlsx', 'pptx', 'zip'], age: [10, 1200], name: (_i, e) => `${pick(['项目周报', '报价单', '合同扫描件', '会议纪要', '课程讲义', '设计稿', '发票', '论文初稿'])}${pick(['', '_v2', '_final', '(1)', '_修订'])}.${e}`, dirs: { n: 14, name: month } });
  fill(D(wx, 'Image'), 2.8 * GB, 1200, { ext: ['dat'], cat: Cat.Image, age: [5, 1300], name: () => `${hex(32)}.dat`, dirs: { n: 14, name: month } });
  fill(D(wx, 'Cache'), 1.4 * GB, 300, { ext: ['tmp', 'dat'], cat: Cat.Cache, age: [0, 60] });
  fill(D(docs, 'Tencent Files'), 1.8 * GB, 240, { ext: ['jpg', 'mp4', 'dat', 'docx'], age: [100, 1600] });
  fill(D(docs, 'My Games'), 0.6 * GB, 80, { ext: ['dat', 'json'], cat: Cat.Game, age: [2, 600] });
  fill(docs, 4.6 * GB, 260, { ext: ['pdf', 'docx', 'xlsx', 'pptx', 'zip', 'md'], age: [10, 2400], name: (_i, e) => `${pick(['季度总结', '读书笔记', '旅行计划', '简历', '租房合同', '体检报告', '毕业设计', '预算表'])}${pick(['', '_2023', '_2024', '_2025', '_草稿'])}.${e}` });

  fill(D(me, 'Music'), 4.8 * GB, 160, { ext: ['flac', 'flac', 'mp3'], age: [300, 2000], sigma: 0.6 });

  const repos = D(D(me, 'source'), 'repos');
  const tess = D(repos, 'tessera');
  fill(D(D(tess, 'target'), 'debug'), 3.4 * GB, 420, { ext: ['rlib', 'pdb', 'obj', 'exe', 'd'], cat: Cat.Dev, age: [0, 5], dirs: { n: 3, name: i => ['deps', 'incremental', 'build'][i] } });
  fill(tess, 0.02 * GB, 60, { ext: ['rs', 'md', 'json'], cat: Cat.Dev, age: [0, 5] });
  fill(D(D(repos, 'blog'), 'node_modules'), 0.9 * GB, 900, { ext: ['js', 'json', 'md'], cat: Cat.Dev, age: [30, 200], sigma: 1.2 });
  fill(D(D(repos, 'game-jam'), 'Library'), 2.8 * GB, 400, { ext: ['assets', 'bundle', 'dat'], cat: Cat.Dev, age: [200, 400] });
  fill(repos, 2.5 * GB, 400, { ext: ['rs', 'js', 'json', 'pdb', 'rlib', 'zip'], cat: Cat.Dev, age: [5, 800], dirs: { n: 7, name: i => ['leetcode', 'dotfiles', 'pixel-editor', 'crawler', 'thesis', 'tauri-playground', 'wasm-demo'][i] } });
  fill(D(D(me, '.cargo'), 'registry'), 2.9 * GB, 500, { ext: ['crate', 'rs'], cat: Cat.Dev, age: [0, 300], dirs: { n: 2, name: i => ['cache', 'src'][i] } });
  fill(D(D(me, '.rustup'), 'toolchains'), 3.1 * GB, 300, { ext: ['rlib', 'dll', 'exe', 'lib'], cat: Cat.Dev, age: [10, 120], dirs: { n: 2, name: i => ['stable-x86_64-pc-windows-msvc', 'nightly-x86_64-pc-windows-msvc'][i] } });
  fill(D(D(me, '.nuget'), 'packages'), 2.4 * GB, 300, { ext: ['dll', 'lib'], cat: Cat.Dev, age: [30, 700] });
  fill(D(me, '.gradle'), 1.6 * GB, 200, { ext: ['jar', 'lib'], cat: Cat.Dev, age: [60, 500] });
  fill(D(D(me, '.vscode'), 'extensions'), 0.9 * GB, 200, { ext: ['js', 'json', 'node'], cat: Cat.Dev, age: [3, 200] });
  fill(D(me, 'OneDrive'), 7.8 * GB, 320, { ext: ['jpg', 'pdf', 'docx', 'mp4', 'xlsx'], age: [30, 1500], cloud: true, dirs: { n: 5, name: i => ['文档', '图片', '桌面', '备份', '共享'][i] } });
  fill(D(me, 'Desktop'), 1.6 * GB, 60, { ext: ['zip', 'pdf', 'png', 'exe', 'docx'], age: [1, 300] });
  fill(D(users, 'Public'), 0.4 * GB, 40, { ext: ['jpg', 'mp4', 'lnk'], age: [300, 900] });

  F(root, 'pagefile.sys', 16.0 * GB, { lock: 'sys', age: 0, cat: Cat.System });
  F(root, 'hiberfil.sys', 12.7 * GB, { lock: 'sys', age: 0, cat: Cat.System });
  F(root, 'swapfile.sys', 0.25 * GB, { lock: 'sys', age: 0, cat: Cat.System });
  const svi = D(root, 'System Volume Information', { lock: 'acl', lockDeep: 'acl' });
  F(svi, '（无权限，大小由卷信息推算）', 9.2 * GB, { lock: 'acl', age: 3, cat: Cat.System });
  const bin = D(root, '$Recycle.Bin');
  const binMe = D(bin, 'S-1-5-21-3623811015-3361044348-30300820-1001');
  fill(binMe, 3.4 * GB, 90, { ext: ['zip', 'mp4', 'exe', 'png', 'tmp'], age: [1, 40], name: (_i, e) => `$R${hex(6).toUpperCase()}.${e}` });
  const rec = D(root, 'Recovery', { lock: 'acl', lockDeep: 'acl' });
  F(rec, 'WindowsRE', 0.6 * GB, { lock: 'acl', age: 500, cat: Cat.System });
  fill(D(root, 'msys64'), 2.2 * GB, 600, { ext: ['exe', 'dll', 'a', 'h'], cat: Cat.Dev, age: [100, 500] });

  // balance the Downloads filler so the total lands on the volume's used size
  let tot = 0;
  for (const n of b.N) if (!n.kids) tot += n.disk;
  fill(dl, Math.max(1 * GB, USED_TARGET - tot), 260, {
    ext: ['zip', 'exe', 'msi', 'pdf', 'png', 'mp4', '7z', 'iso'],
    age: [1, 900],
    name: (_i, e) => `${pick(['setup', 'installer', 'portable', 'release', 'driver', 'firmware', 'wallpaper', 'dataset', 'backup', 'sample', 'font', 'patch'])}${pick(['-x64', '_win64', '-v2', '_2025', '-final', ''])}${b.rnd() < 0.4 ? '-' + (1 + int(30)) : ''}.${e}`,
  });

  return finish(b, { root: 'C:\\', label: '本地磁盘', fs: 'NTFS', capacity: CAPACITY, usedTarget: USED_TARGET, binId: bin.id, binUserId: binMe.id, downloadsId: dl.id });
}

export function buildD(): Drive {
  const b = new Builder(931);
  const { D, pick, int, hex } = b;
  const F = b.F.bind(b);
  const fill = b.fill.bind(b);
  const pad2 = (n: number) => String(n).padStart(2, '0');
  const CAPACITY = 931.5 * GB;
  const USED_TARGET = 612.4 * GB;
  const root = D(null, 'D:');

  const lib = D(D(root, 'SteamLibrary'), 'steamapps');
  const common = D(lib, 'common');
  const games: [string, number, string[]][] = [
    ['Baldurs Gate 3', 128, ['pak']],
    ['Black Myth Wukong', 131, ['pak', 'utoc', 'ucas']],
    ['Elden Ring', 58, ['bdt', 'bhd']],
    ['Red Dead Redemption 2', 119, ['rpf']],
    ['Forza Horizon 5', 103, ['zip', 'pak']],
  ];
  for (const [g, s, ext] of games) fill(D(common, g), s * GB, 90, { ext, cat: Cat.Game, age: [20, 600], sigma: 1.2 });
  fill(D(lib, 'shadercache'), 4.2 * GB, 120, { ext: ['cache'], cat: Cat.Cache, age: [0, 60] });
  fill(D(lib, 'downloading'), 2.1 * GB, 30, { ext: ['tmp'], cat: Cat.Cache, age: [0, 2] });

  const media = D(root, '影视');
  fill(media, 46 * GB, 60, { ext: ['mkv', 'mp4'], age: [100, 1500], sigma: 0.7, dirs: { n: 6, name: i => ['纪录片', '电影', '动画', '演唱会', '课程录像', '家庭录像'][i] } });

  const photos = D(root, '照片');
  fill(photos, 38 * GB, 900, { ext: ['arw', 'jpg', 'heic', 'mp4'], age: [200, 2400], sigma: 0.8, name: (_i, e) => `IMG_${1000 + int(8999)}.${e.toUpperCase()}`, dirs: { n: 9, name: i => `${2017 + i}` } });

  const bak = D(root, '备份');
  F(bak, 'WindowsImageBackup_2025-06.vhdx', 21.4 * GB, { age: 150 });
  fill(D(bak, '手机备份'), 18.6 * GB, 120, { ext: ['zip', 'db', 'jpg', 'mp4'], age: [40, 900] });
  fill(D(bak, '旧电脑'), 9.8 * GB, 300, { ext: ['zip', 'docx', 'pdf', 'jpg'], age: [700, 2000] });

  const vm = D(root, 'VMs');
  F(D(vm, 'Win11-dev'), 'Win11-dev.vhdx', 42.0 * GB, { age: 12 });
  F(D(vm, 'ubuntu-server'), 'ubuntu-server.vhdx', 14.5 * GB, { age: 80 });

  const dev = D(root, 'dev');
  fill(dev, 6.4 * GB, 1200, { ext: ['rs', 'js', 'json', 'rlib', 'pdb', 'obj', 'zip'], cat: Cat.Dev, age: [0, 700], dirs: { n: 6, name: i => ['android-sdk', 'datasets', 'llama-models', 'unity-projects', 'scratch', 'archive'][i] } });
  fill(D(root, '下载'), 11.2 * GB, 140, { ext: ['zip', 'exe', 'iso', 'pdf', '7z'], age: [3, 900], name: (_i, e) => `${pick(['setup', 'driver', 'dataset', 'release', 'backup'])}_${hex(4)}_${2024 + int(2)}${pad2(1 + int(12))}.${e}` });
  const bin = D(root, '$RECYCLE.BIN');
  const binMe = D(bin, 'S-1-5-21-3623811015-3361044348-30300820-1001');
  fill(binMe, 1.1 * GB, 20, { ext: ['zip', 'mp4'], age: [1, 30], name: (_i, e) => `$R${hex(6).toUpperCase()}.${e}` });
  const svi = D(root, 'System Volume Information', { lock: 'acl', lockDeep: 'acl' });
  F(svi, '（无权限，大小由卷信息推算）', 2.3 * GB, { lock: 'acl', age: 3, cat: Cat.System });

  let tot = 0;
  for (const n of b.N) if (!n.kids) tot += n.disk;
  fill(D(root, '杂项'), Math.max(1 * GB, USED_TARGET - tot), 200, { ext: ['zip', 'mp4', 'jpg', 'pdf', 'psd'], age: [10, 1500] });

  return finish(b, { root: 'D:\\', label: '数据', fs: 'NTFS', capacity: CAPACITY, usedTarget: USED_TARGET, binId: bin.id, binUserId: binMe.id, downloadsId: -1 });
}
