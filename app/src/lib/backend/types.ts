/**
 * The frontend's view of the Rust core. Mirrors docs/ipc.md; the mock backend and
 * the Tauri backend both implement `Backend`.
 */

export type SizeMode = 'disk' | 'logical';

export interface PlatformInfo {
  /** true when the process runs elevated (MFT path available) */
  elevated: boolean;
  /** the window backdrop that Rust managed to apply */
  backdrop: 'mica' | 'none';
  /** 'custom': the page draws the title bar (Windows 11); 'native': Windows does */
  titleBar: 'custom' | 'native';
  /** system accent colour, tuned per theme as Windows does (AccentDark1 / AccentLight2) */
  accent: { light: string; dark: string } | null;
  version: string;
  /** true when tessera-core serves the commands; false means the mock tree is used */
  core: boolean;
  /** the UI runs inside the Tauri window (not a plain browser preview) */
  native: boolean;
  /** UI language, picked from the Windows preferred languages (see lib/i18n.svelte.ts) */
  lang: string;
  /** BCP 47 tag for dates and numbers, e.g. en-GB */
  locale: string;
}

export interface Volume {
  root: string;
  label: string;
  fs: string;
  capacity: number;
  free: number;
}

export interface LayoutStyle {
  outer: number;
  pad: number;
  gap: number;
  header: number;
  minHeaderW: number;
  minArea: number;
  /** no tile narrower or shorter than this (CSS px, gap not counted); 0 = off */
  minSide: number;
  /** no folder tile narrower than this (CSS px, gap not counted), so its name fits; 0 = off */
  minWidth: number;
  /** no folder tile shorter than this (CSS px, gap not counted); 0 or absent = off (the band look's design canvas only, for now) */
  minHeight?: number;
  /** a folder tile holds its size label on one line (rule 2); absent = off (the band look's design canvas only, for now) */
  sizeFit?: SizeFit;
  recurse: number;
  openShare: number;
  maxDepth: number;
  maxPerDir: number;
  labelMinW: number;
  labelMinH: number;
}

/**
 * The room a folder's size label ("8.6 GB") takes at the UI's font, so the layout can give
 * every folder tile room for its own label (rule 2). With it, a folder's minimum is no longer
 * minWidth x minHeight but the label on one line: label width + inset wide (never narrower
 * than minWidth) and `height` tall. CSS px, gap not counted.
 */
export interface SizeFit {
  /** the widest digit */
  digit: number;
  /** '.' (and any digit grouping mark) */
  dot: number;
  /** the space between number and unit */
  space: number;
  /** the unit: bytes (the UI's word), KB, MB, GB, TB */
  units: [number, number, number, number, number];
  /** room left and right of the label, together */
  inset: number;
  /** least folder height with the label in it */
  height: number;
}

export interface LayoutRequest {
  view: number;
  width: number;
  height: number;
  sizeMode: SizeMode;
  style: LayoutStyle;
  /** a level of `view` (docs/ipc.md rule 9): the children it leaves out */
  hide?: number[];
}

/** 'level' is the UI's own card for a level of the view (docs/ipc.md rule 9); the core never sends it */
export type DetailsKind = 'file' | 'dir' | 'group' | 'free' | 'unscanned' | 'level';

export interface Details {
  id: number;
  kind: DetailsKind;
  name: string;
  path: string;
  size: number;
  disk: number;
  logical: number;
  share: number;
  files: number;
  dirs: number;
  modified: number | null;
  cat: number;
  flags: number;
  lockReason: string | null;
  hint: string | null;
}

export interface Crumb {
  id: number;
  name: string;
}

export interface NavInfo {
  crumbs: Crumb[];
  up: number | null;
}

export interface CatStat {
  cat: number;
  bytes: number;
}

/**
 * What became of Windows' menu: closed, a command ran, or "Delete" was chosen (with Shift
 * held: "purge"), which Tessera then does itself.
 */
export type ShellAction = 'none' | 'invoked' | 'recycle' | 'purge';

export interface DeleteResult {
  ok: number[];
  failed: { id: number; reason: string }[];
  bytes: number;
  undo: number | null;
  rev: number;
}

export interface ScanStarted {
  scanId: number;
  rootId: number;
}

export interface ScanProgress {
  scanId: number;
  files: number;
  dirs: number;
  /** bytes found so far */
  scanned: number;
  /** the volume's used bytes, known before the scan starts */
  used: number;
  elapsedMs: number;
  done: boolean;
  rev: number;
  errors: number;
}

/** what the page's caption buttons show (custom title bar only) */
export interface CaptionState {
  maximized: boolean;
  /** the pointer is on the maximize button: Windows tells Rust, not the page (snap layouts) */
  hover: boolean;
  pressed: boolean;
}

/** what a search found (see docs/ipc.md › search) */
export interface SearchResult {
  /** echoed in the header of layouts that show these hits */
  id: number;
  /** matching files and folders, not counting what is inside a matching folder */
  count: number;
  /** their bytes, in the size mode searched */
  bytes: number;
  ms: number;
}

export interface TreeChanged {
  rev: number;
  bytesDelta: number;
}

export interface BackendEvents {
  'scan-progress': ScanProgress;
  'tree-changed': TreeChanged;
  caption: CaptionState;
}

export type Unlisten = () => void;

export interface Backend {
  readonly platform: PlatformInfo;
  listVolumes(): Promise<Volume[]>;
  scanStart(root: string): Promise<ScanStarted>;
  scanCancel(): Promise<void>;
  /** raw layout buffer, decode with `decodeLayout` */
  layout(req: LayoutRequest): Promise<ArrayBuffer>;
  details(id: number, view: number, sizeMode: SizeMode): Promise<Details | null>;
  nav(id: number): Promise<NavInfo>;
  /** the legend by type; with `hide`, of a level of `view` (what it does not leave out) */
  catStats(view: number, sizeMode: SizeMode, hide?: number[]): Promise<CatStat[]>;
  /**
   * the legend by age: bytes of the files in `view` (with `hide`, in a level of it) per age
   * bin (AGE_BINS entries, 0 = date not known; see ageBin in lib/age.ts)
   */
  ageStats(view: number, sizeMode: SizeMode, hide?: number[]): Promise<number[]>;
  recycle(ids: number[]): Promise<DeleteResult>;
  restore(token: number): Promise<DeleteResult>;
  purge(ids: number[]): Promise<DeleteResult>;
  emptyRecycleBin(root: string): Promise<DeleteResult>;
  reveal(id: number): Promise<void>;
  /** Windows' own context menu for `ids` (one folder) at client point x, y; resolves when it closes */
  shellMenu(ids: number[], x: number, y: number, extended: boolean): Promise<ShellAction>;
  relaunchElevated(): Promise<void>;
  /**
   * runs the search box's query; layouts carry its hits from then on. null when a newer
   * search or a clear came in meanwhile, or the query is empty (which clears). Rejects with a
   * user-facing message for a query it can't read (">1XB").
   */
  search(query: string, sizeMode: SizeMode): Promise<SearchResult | null>;
  /** stops a running search and drops the hits */
  searchClear(): Promise<void>;
  /** the custom title bar's buttons */
  caption(action: 'min' | 'max' | 'close'): Promise<void>;
  /** where the maximize button is (CSS px, from the window's right edge) */
  captionButton(rect: { right: number; top: number; width: number; height: number }): Promise<void>;
  captionState(): Promise<CaptionState>;
  /** the view options saved in the registry (see lib/settings.ts) */
  loadSettings(): Promise<Record<string, number | string>>;
  /** saves one value; `null` deletes it */
  saveSetting(name: string, value: number | string | null): Promise<void>;
  /** deletes everything Tessera saved, the window's place and size too */
  clearSettings(): Promise<void>;
  on<K extends keyof BackendEvents>(event: K, cb: (payload: BackendEvents[K]) => void): Promise<Unlisten>;
  /** preview-only helpers (live-update demo); absent on the real backend */
  demo?: { startDownload(): boolean };
}
