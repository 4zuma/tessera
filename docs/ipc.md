# Tessera IPC contract (v1)

The full file tree lives in Rust. The WebView only ever receives

1. **layout buffers**: the rectangles that are visible right now, as one binary blob, and
2. **small JSON records** about a single node (details, breadcrumbs, delete results).

Everything here is shaped as Tauri 2 commands and events. The frontend has a mock
implementation of the same contract (`app/src/lib/backend/mock/`), so the UI can be built
and tested without the scan core, and the real core can replace it 1:1.

All sizes are bytes. All ids are `u32` node indices into the core's arena unless noted.

## Commands

| Command | Args | Returns |
| --- | --- | --- |
| `platform_info` | – | `{ elevated: bool, backdrop: "mica" \| "none", titleBar: "custom" \| "native", accent: { light, dark } \| null, version: string, core: bool, lang: "en" \| "zh-CN", locale: string }` – `lang` is the UI language, `locale` the BCP 47 tag for dates and numbers (see below) |
| `list_volumes` | – | `Volume[]` |
| `scan_start` | `{ root: string }` | `{ scanId: u32, rootId: u32 }` – cancels any running scan |
| `scan_cancel` | – | `()` |
| `layout` | `{ req: LayoutRequest }` | **raw bytes** (`tauri::ipc::Response`), see below |
| `details` | `{ id: u32, view: u32, sizeMode }` | `Details \| null` (null when the id no longer exists) |
| `nav` | `{ id: u32 }` | `{ crumbs: {id, name}[], up: u32 \| null }` |
| `cat_stats` | `{ view: u32, sizeMode }` | `{ cat: u8, bytes: u64 }[]`, largest first |
| `recycle` | `{ ids: u32[] }` | `DeleteResult` (with `undo` token) |
| `restore` | `{ token: u32 }` | `DeleteResult` – undo of a `recycle` |
| `purge` | `{ ids: u32[] }` | `DeleteResult` |
| `empty_recycle_bin` | `{ root: string }` | `DeleteResult` |
| `reveal` | `{ id: u32 }` | `()` – `explorer.exe /select,"<path>"` |
| `shell_menu` | `{ ids: u32[], x: f64, y: f64, extended: bool }` | `"none" \| "invoked" \| "recycle" \| "purge"` – Windows' classic context menu for `ids` (all in one folder, else an error) at client point `x`, `y` (CSS px), with Explorer's Shift-only verbs if `extended`; resolves when the menu closes. "Delete" is not run by the shell: `"recycle"` (`"purge"` with Shift held) tells the UI to delete through `recycle` / `purge` itself |
| `relaunch_elevated` | – | `()` – restarts with `runas`, keeps the window position |
| `search` | `{ query: string, sizeMode }` | `{ id: u32, count: u64, bytes: u64, ms: u32 } \| null` – runs the search box's query (see [Search](#search)) on a worker thread; from then on every layout carries its hits, with `id` in the header. `count` and `bytes` are the matches (a matching folder counts once, not what is in it). `null` when the query is empty (which clears the search) or a newer `search` / `search_clear` came in meanwhile. Rejects with a user-facing message for a query it can't read (`>1XB`); the hits stay as they were |
| `search_clear` | – | `()` – stops a running search and drops the hits |
| `caption` | `{ action: "min" \| "max" \| "close" }` | `()` – the custom title bar's buttons (`"max"` maximizes or restores) |
| `caption_button` | `{ right, top, width, height: f64 }` | `()` – where the custom title bar's maximize button is, in CSS px (`right` = distance from the window's right edge), so Windows 11 can open its snap layouts there |
| `caption_state` | – | `CaptionState` |

```ts
type SizeMode = "disk" | "logical";        // allocated size (default) | file length

interface Volume { root: string; label: string; fs: string; capacity: number; free: number; }

interface LayoutRequest {
  view: u32;            // node id to show as the root of the map
  width: f32; height: f32;   // CSS px of the canvas
  sizeMode: SizeMode;
  style: {
    outer: f32;         // margin around the whole map (6)
    pad: f32;           // inner padding of a folder (3)
    gap: f32;           // gap between siblings (2), each rect is inset by gap/2
    header: f32;        // folder header band height (21)
    minHeaderW: f32;    // a folder narrower than this gets no header (60)
    minArea: f32;       // px²; smaller siblings are merged into one group (110)
    minSide: f32;       // no tile narrower or shorter than this, gap not counted (24); 0 = off
    minWidth: f32;      // no tile narrower than this, gap not counted (44); 0 = off
    recurse: f32;       // a folder opens only if area > minArea * recurse (60, so 6,600 px²)
    openShare: f32;     // ... and only if area > openShare * the map's area (0.0025); 0 = off
    maxDepth: u8;       // (14)
    maxPerDir: u32;     // at most this many tiles per folder, the rest are grouped (400)
    labelMinW: f32;     // names are sent only for rects at least this big (44 × 20),
    labelMinH: f32;     //   and for every folder that gets a header
  };
}

interface Details {
  id: u32; kind: "file" | "dir" | "group" | "free" | "unscanned";
  name: string; path: string;
  size: u64;            // in the requested size mode
  disk: u64; logical: u64;
  share: f64;           // size / parent size
  files: u32; dirs: u32;  // for folders, recursive
  modified: f64 | null; // unix ms, read on demand (one GetFileAttributesExW)
  cat: u8; flags: u8;   // same as in the layout record
  lockReason: string | null;  // why delete is disabled, user-facing text
  hint: string | null;        // e.g. "powercfg /h off" for hiberfil.sys
}

interface DeleteResult {
  ok: u32[];                          // ids that were removed from the tree
  failed: { id: u32; reason: string }[];
  bytes: u64;                         // bytes moved / freed
  undo: u32 | null;                   // token for `restore`, recycle only
  rev: u32;                           // tree revision after the change
}
```

## Language

There is no language setting. At startup Rust reads the user's "Preferred languages" from
Windows (`GlobalizationPreferences.Languages`, the ordered list in Settings › Time &
language › Language & region; `GetUserPreferredUILanguages` if WinRT is unavailable) and
takes the first one Tessera has a translation for: Simplified Chinese (`zh-Hans-*`, `zh`,
`zh-CN`, `zh-SG`…) or any English. When none matches, the UI is English. Traditional Chinese
has no translation yet, so it falls through to the next language in the list. The
environment variable `TESSERA_LANG` (e.g. `en`) overrides the list, for testing.

All user-facing text in commands' results (`lockReason`, `hint`, `failed[].reason`,
volume labels, the names of the free, unscanned and grouped tiles, error strings) is in
that language (`app/src-tauri/src/i18n.rs`); the WebView's own text is in
`app/src/lib/locales/`. Text that comes from Windows itself (shell errors) is passed
through as Windows worded it.

## Events (Rust → WebView)

| Event | Payload | When |
| --- | --- | --- |
| `scan-progress` | `{ scanId, files, dirs, scanned, used, elapsedMs, done, rev, errors }` | every 100 ms while scanning, and once with `done: true` |
| `tree-changed` | `{ rev, bytesDelta }` | live updates from the watcher that can show: at most every 500 ms (2 s while the window is in the background), never while it is minimized (one on restore) |
| `caption` | `{ maximized, hover, pressed }` | custom title bar only: the window was maximized or restored, or the pointer moved onto, off, or pressed the maximize button (Windows reports that spot to Rust, not to the page; see `app/src-tauri/src/titlebar.rs`) |

`scanned` is bytes found so far, `used` is the volume's used bytes (known up front from
`GetDiskFreeSpaceExW`), so `scanned / used` is the progress and `used - scanned` is the
"not scanned yet" tile. The frontend asks for a new layout after each event, with at most
one `layout` call in flight; there is no polling and no timer when nothing changes.

`tree-changed` is held back while a change cannot move anything on screen: the backend keeps
the last layout it sent and how many bytes changed since (`Tree::changed_bytes`). Below one
px² of the shown view it stays silent; past that it lays the view out again and sends the
event only when some edge moved by more than 0.5 px or a tile, colour or size label changed
(`layout::same_picture`). Changes Windows keeps making on a system drive (logs, caches) thus
cost no IPC, no layout in the WebView and no frame. While a search is shown, a change is
sent anyway when it touches what the search found or would find now (`Hits::touched_by`: a
folder that matched, a child that was found, or a child that matches now), so the count and
the bytes follow small changes too. `bytesDelta` covers everything since the last event.

## Layout buffer

Little-endian. A 36-byte header, `count` records of 52 bytes, then the UTF-8 name pool.
Readers take the header and record sizes from the header, so fields can be added at the end.

```
header (36 bytes)
  0  u32  magic       0x4C525354 ("TSRL")
  4  u16  version     1
  6  u16  headerSize  36
  8  u32  count       number of records
 12  u32  recordSize  52
 16  u32  namesLen    bytes in the name pool
 20  u32  view        echo of the request
 24  u32  rev         tree revision this layout was computed from
 28  f32  bytesPerPx  bytes represented by one px² at this view
 32  u32  search      id of the search whose hits are in `hit`, 0 = no search

record (52 bytes)
  0  u32  id          node id, or a special id (below)
  4  u32  parent      id of the enclosing rect, NONE at the top level
  8  u32  target      what a double-click zooms into: the deepest folder of a folded
                      chain, the folder of a group, or the id itself
 12  u32  count       GROUP: how many items were merged; otherwise 0
 16  f32  x           CSS px, relative to the canvas
 20  f32  y
 24  f32  w
 28  f32  h
 32  u32  sizeLo      size in the requested mode
 36  u32  sizeHi
 40  u32  nameOff     offset into the name pool
 44  u16  nameLen     0 = no name sent (ask `details` on hover)
 46  u8   depth       1 = direct child of the view
 47  u8   kind        0 FILE, 1 DIR (open: its children follow), 2 DIR_LEAF (folder drawn
                      solid), 3 GROUP, 4 FREE, 5 UNSCANNED
 48  u8   cat         file category, 255 = none. FILE: its own; GROUP and DIR_LEAF: the
                      category with the most bytes in it (rules 2, 6); 255 for open
                      folders, pseudo tiles, and groups or closed folders without files
 49  u8   flags       bit0 PROTECTED, bit1 NO_ACCESS, bit2 IN_USE, bit3 CLOUD,
                      bit4 HARDLINK, bit5 RECYCLE_BIN, bit6 REPARSE, bit7 SCANNING
 50  u8   header      header band height in px, 0 = none
 51  u8   hit         what the search found in it, 0 when there is no search: 255 = all
                      of it (a match, or inside a matching folder), 0 = nothing, else
                      1..254 = that share of its bytes (folders and groups)
```

Records are in paint order: a folder always comes before its children.

Special ids: `NONE = 0xFFFFFFFF`, `FREE = 0xFFFFFFFE`, `UNSCANNED = 0xFFFFFFFD`.
A group's id is `0x80000000 | folderId`, so it stays stable between layouts and the frontend
can tween it. Real node ids must therefore stay below `0x80000000`.

### Categories (`cat`)

`0 other, 1 video, 2 image, 3 audio, 4 archive, 5 vdisk, 6 app, 7 game, 8 doc, 9 dev,
10 cache, 11 system`. The core maps file extensions with the table in
`crates/tessera-core/src/category.rs`; `app/src/lib/backend/categories.ts` is a copy of it
for the mock. A `.ts` file under 1 MiB is `dev` (TypeScript), otherwise `video`.

### Layout rules the core implements

These are what make the map calm while it changes, and they were tuned in the prototypes.

1. Children with size 0 are skipped. Siblings are sorted by size, with **25 % hysteresis**:
   each node remembers the size it was ranked at, and only re-ranks when its size moved by
   more than 25 %. A growing scan therefore does not reshuffle the map on every update.
2. Siblings whose area would be below `minArea`, or beyond the first `maxPerDir`, are merged
   into one GROUP tile; its `cat` is the category with the most bytes among them. A single
   small sibling is shown as itself, not as a group of one.
   With `minSide`, every tile is at least that wide and tall, since a thin sliver hardly
   reads as a tile (24 px is a little more than Windows' 13 × 20 px arrow pointer); with
   `minWidth`, at least that wide, since names are written across and a tall narrow tile
   shows only a letter or two (44 px is `labelMinW`, so every tile gets its name). Siblings whose
   area would be below `(minSide + gap) × (max(minSide, minWidth) + gap)` go to the group at
   once, and the group takes its place among the others by its size instead of coming
   last. After squarifying, the first sibling (in rank order) that came out too small and
   every one after it join the group, and the folder is laid out again. A group (or a single
   left-over sibling), UNSCANNED or FREE tile that is still too small is left out and the
   others take its room. This repeats at most 12 times; a tile still too small then is not
   drawn and its room stays empty.
3. Squarified treemap (Bruls et al. 2000) into the parent's inner box. Every rect is inset
   by `gap / 2`; rects smaller than 1 px in either direction are dropped.
4. **Folded chains**: a folder whose only visible child is a folder is merged with it into
   one tile named `a › b › c`; `target` is the deepest folder. `nav().up` skips such
   folders too, so "back" never lands on a view with a single tile. The UI copies the
   path of `target`, shows `target` in Explorer and shows its path in the details card,
   since that is where the files are; delete and Windows' own menu take the whole tile
   (`id`).
5. A folder gets a header band of `header` px when it is at least `header * 2.2` tall and
   `minHeaderW` wide. It opens (kind DIR) only when `depth < maxDepth`, it has a header
   band (unless `header` is 0), and its area is above both `minArea * recurse` and
   `openShare` of the map's area. So an open folder is always named and has room for a
   few labelled tiles, and a very large window does not open more levels than a normal
   one; smaller folders stay DIR_LEAF tiles that show their name and size. The inner box
   is inset by `pad` (by `header` at the top). If the inner box is smaller than 3 × 3 px
   it stays DIR_LEAF.
6. A DIR_LEAF's `cat` is the category with the most bytes among all files below it (lower
   category on a tie), so the UI can tint closed folders by what fills them. The core
   caches it per folder and walks the folder again only when its size moved by over 1/8.
7. At the volume root, two pseudo children are added: UNSCANNED (`used - scanned`, while
   scanning and above 64 MiB) ranked first, and FREE (the volume's free bytes) ranked last.
8. With a search set, every record's `hit` says how much of it the search found. A GROUP
   gets the share of the merged items' bytes that matched, so it stays findable when a
   match is too small for its own tile.

## Search

The search box's query, parsed by `crates/tessera-core/src/search.rs` (the mock repeats it
in `app/src/lib/backend/mock/search.ts`). Words are separated by spaces and all must hold:

| Word | Matches |
| --- | --- |
| `report` | a file or folder whose name contains it, ignoring case as Windows does |
| `"tax 2024"` | the same, with spaces in it |
| `.iso`, `*.iso` | files ending in `.iso`; with several extension words, any of them |
| `>1GB`, `>=1.5g`, `<500MB`, `<=4k` | a size bound in the searched size mode; binary units (`k` = 1024), `b k kb kib m mb mib g gb gib t tb tib`, any case, decimals allowed. A word that starts with `<` or `>` and is not a size rejects the query |

A folder matches only through name words, and not when the query has an extension word
(`.iso` finds files only). Everything inside a matching folder counts as found and is lit,
but only the folder is counted. Deleted nodes and nodes that are not attached yet are
skipped. A search over 3.3 M nodes takes about 70 ms on 4 cores: one parallel pass marks
the matches (a node's name is folded once per distinct name), a second pass walks the tree
for the folder shares, the count, and the bytes. The hits hold one byte per node.

The UI searches 150 ms after the last key press (Enter: at once), and again when the size
mode changes, after a delete or an undo, when a scan ends, and at most every 1.5 s while a
scan or the watcher changes the tree. What the search did not find is dimmed; a folder or
group that holds some matches is dimmed less the more of it matched.
