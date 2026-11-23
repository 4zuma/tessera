//! Squarified treemap layout, encoded as the binary layout buffer of the
//! IPC contract (v1).
//!
//! The full tree stays in Rust; [`LayoutEngine::layout`] turns the part of
//! it that is visible in one view into a compact buffer the WebView draws
//! with Canvas 2D. The engine is stateful: it remembers the size each node
//! was ranked at (for the rank hysteresis below) and caches file
//! categories, so keep one engine per tree and call
//! [`LayoutEngine::reset`] when a new scan starts.
//!
//! # Rules
//!
//! 1. Children of size 0, and deleted nodes, are skipped. Siblings are
//!    ordered by size with 25 % hysteresis: each node remembers the size it
//!    was ranked at and is only re-ranked when its size moved by more than
//!    25 %, so a growing scan does not reshuffle the map on every update.
//!    Ties are broken by node id, so the order is deterministic.
//! 2. Siblings whose area would be below `minArea`, or that rank beyond the
//!    first `maxPerDir`, are merged into one GROUP tile, placed after the
//!    other siblings; its `cat` is the category with the most bytes among
//!    its files. A single small sibling is shown as itself rather than as a
//!    group of one.
//! 3. Squarified treemap (Bruls, Huijsing and van Wijk, "Squarified
//!    Treemaps", 2000) into the parent's inner box: siblings in rank order
//!    are added to a row along the shorter side of the free space while
//!    that does not worsen the row's worst aspect ratio. Every cell is then
//!    inset by `gap / 2`; rects smaller than 1 px in either direction are
//!    dropped (with everything inside them).
//! 4. Folded chains: a folder whose only visible child is a folder is
//!    merged with it into one tile named `a › b › c`, whose `id` is the top
//!    folder and whose `target` is the deepest one; the deepest folder's
//!    children fill the tile and name the tile as their `parent`.
//! 5. A folder gets a header band of `header` px when it is at least
//!    `header * 2.2` tall and `minHeaderW` wide (this applies to DIR_LEAF
//!    tiles too). It opens (kind DIR, its children follow) only when all of
//!    these hold:
//!    * its depth is below `maxDepth`;
//!    * it has a header band (unless `header` is 0), so an open folder is
//!      always named;
//!    * its area is above `minArea * recurse` px², so an open folder has
//!      room for a few labelled tiles;
//!    * its area is above `openShare` of the map's area, so a very large
//!      window does not open more levels than a normal one.
//!
//!    The inner box is the tile inset by `pad`, by `header` at the top when
//!    it has a header. If the inner box is smaller than 3 x 3 px, or no
//!    child ends up visible, the folder is a DIR_LEAF (drawn solid).
//! 6. A DIR_LEAF's `cat` is the category with the most bytes among all the
//!    files below it (on a tie, the lower category), or 255 when it has
//!    none, so a closed folder can be tinted by what fills it. The engine
//!    walks a closed folder's subtree once and remembers the answer with
//!    the folder's size; it walks it again only when that size has moved
//!    by more than 1/8, so steady layouts cost nothing extra.
//! 7. When the view is the tree root, two pseudo tiles can be added from
//!    numbers the caller passes in [`RootExtras`]: UNSCANNED (`used -
//!    scanned` while scanning, only above 64 MiB), ranked first, and FREE
//!    (the volume's free bytes), ranked last.
//!
//! Folders are opened largest first, so if the internal safety cap of
//! [`MAX_RECORDS`] records is ever reached, the big structure is complete
//! and only fine detail is missing.
//!
//! # Buffer
//!
//! Little-endian. A 32-byte header, `count` records of 52 bytes, then the
//! UTF-8 name pool.
//!
//! ```text
//! header (32 bytes)
//!   0  u32  magic       0x4C525354 ("TSRL")
//!   4  u16  version     1
//!   6  u16  headerSize  32
//!   8  u32  count       number of records
//!  12  u32  recordSize  52
//!  16  u32  namesLen    bytes in the name pool
//!  20  u32  view        echo of the request
//!  24  u32  rev         tree revision this layout was computed from
//!  28  f32  bytesPerPx  bytes represented by one px² at this view
//!
//! record (52 bytes)
//!   0  u32  id          node id, or ids::FREE / ids::UNSCANNED, or
//!                       ids::GROUP_BIT | folder id for a group
//!   4  u32  parent      id of the enclosing rect, ids::NONE at the top level
//!   8  u32  target      what a double-click zooms into: the deepest folder
//!                       of a folded chain, the folder of a group, or the id
//!  12  u32  count       GROUP: how many items were merged; otherwise 0
//!  16  f32  x           CSS px, relative to the canvas
//!  20  f32  y
//!  24  f32  w
//!  28  f32  h
//!  32  u32  sizeLo      size in the requested mode
//!  36  u32  sizeHi
//!  40  u32  nameOff     offset into the name pool
//!  44  u16  nameLen     0 = no name sent
//!  46  u8   depth       1 = direct child of the view
//!  47  u8   kind        see [`kind`]
//!  48  u8   cat         see [`crate::category`]; for DIR_LEAF and GROUP the
//!                       category with the most bytes (rules 2, 6); 255 for
//!                       open folders, pseudo tiles and anything without files
//!  49  u8   flags       see [`flags`]
//!  50  u8   header      header band height in px, 0 = none
//!  51  u8   reserved    0
//! ```
//!
//! Records are in paint order: a folder always comes before its children.
//! Names are sent for files and folders at least `labelMinW` x `labelMinH`
//! big and for every folder with a header; WTF-8 names that are not valid
//! UTF-8 are converted lossily. `bytesPerPx` is the view's total size
//! (including pseudo tiles) divided by the area of the map, the canvas
//! minus the `outer` margin.

use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::{BinaryHeap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};

use crate::tree::Node;
use crate::{Error, NodeFlags, NodeId, Result, Tree, category, wtf8};

/// `"TSRL"` read as a little-endian `u32`.
pub const MAGIC: u32 = 0x4C52_5354;
pub const VERSION: u16 = 1;
pub const HEADER_SIZE: usize = 32;
pub const RECORD_SIZE: usize = 52;
/// Safety cap on the number of records in one layout. Never reached with
/// sane styles (a 4K canvas holds at most ~75,000 tiles of 110 px²).
/// `maxPerDir` is clamped to `MAX_RECORDS - 3` so the top level fits too.
pub const MAX_RECORDS: usize = 100_000;
/// Pseudo tile threshold for UNSCANNED (rule 7).
pub const UNSCANNED_MIN: u64 = 64 << 20;

/// Special ids in records.
pub mod ids {
    pub const NONE: u32 = 0xFFFF_FFFF;
    pub const FREE: u32 = 0xFFFF_FFFE;
    pub const UNSCANNED: u32 = 0xFFFF_FFFD;
    /// A group's id is `GROUP_BIT | folder id`.
    pub const GROUP_BIT: u32 = 0x8000_0000;
}

/// Record kinds.
pub mod kind {
    pub const FILE: u8 = 0;
    /// An open folder: its children follow.
    pub const DIR: u8 = 1;
    /// A folder drawn solid.
    pub const DIR_LEAF: u8 = 2;
    pub const GROUP: u8 = 3;
    pub const FREE: u8 = 4;
    pub const UNSCANNED: u8 = 5;
}

/// Record flag bits.
pub mod flags {
    /// Deleting it is refused ([`crate::delete::lock_reason`]).
    pub const PROTECTED: u8 = 1 << 0;
    /// Could not be read.
    pub const NO_ACCESS: u8 = 1 << 1;
    /// Open by another process. Not detected yet; always 0.
    pub const IN_USE: u8 = 1 << 2;
    /// Cloud placeholder (only local bytes counted).
    pub const CLOUD: u8 = 1 << 3;
    /// An additional hard link (its bytes are counted elsewhere).
    pub const HARDLINK: u8 = 1 << 4;
    /// The `$Recycle.Bin` folder directly under a volume root.
    pub const RECYCLE_BIN: u8 = 1 << 5;
    /// Junction, symbolic link or other reparse point.
    pub const REPARSE: u8 = 1 << 6;
    /// Still being scanned. Not tracked yet; always 0.
    pub const SCANNING: u8 = 1 << 7;
}

/// Which size the areas are proportional to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SizeMode {
    /// Allocated bytes on disk ([`Node::size`]).
    #[default]
    Disk,
    /// File length ([`Node::logical`]).
    Logical,
}

impl SizeMode {
    #[inline]
    pub fn of(self, node: &Node) -> u64 {
        match self {
            SizeMode::Disk => node.size(),
            SizeMode::Logical => node.logical(),
        }
    }
}

/// `LayoutRequest.style`; [`Default`] gives the contract's defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutStyle {
    /// Margin around the whole map.
    pub outer: f32,
    /// Inner padding of a folder.
    pub pad: f32,
    /// Gap between siblings; every rect is inset by `gap / 2`.
    pub gap: f32,
    /// Folder header band height.
    pub header: f32,
    /// A folder narrower than this gets no header.
    pub min_header_w: f32,
    /// Siblings smaller than this (px²) are merged into a group.
    pub min_area: f32,
    /// A folder opens only if its area is above `min_area * recurse`.
    pub recurse: f32,
    /// A folder opens only if its area is above this share (0 to 1) of the
    /// map's area; 0 turns the rule off.
    pub open_share: f32,
    pub max_depth: u8,
    /// At most this many tiles per folder; the rest are grouped.
    pub max_per_dir: u32,
    /// Names are sent only for rects at least this big (and for every
    /// folder with a header).
    pub label_min_w: f32,
    pub label_min_h: f32,
}

/// Default `recurse`: with `min_area` 110, a folder must be over 6,600 px²
/// (about 80 x 80 px) to open, so that an open folder has room for a few
/// labelled tiles under its header.
pub const RECURSE: f32 = 60.0;
/// Default `open_share`: a folder must also be over 0.25 % of the map to
/// open. This only matters on maps over 2.6 million px² (about
/// 2000 x 1300), where it keeps a very large window from opening more
/// levels than a normal one.
pub const OPEN_SHARE: f32 = 0.0025;

impl Default for LayoutStyle {
    fn default() -> Self {
        LayoutStyle {
            outer: 6.0,
            pad: 3.0,
            gap: 2.0,
            header: 21.0,
            min_header_w: 60.0,
            min_area: 110.0,
            recurse: RECURSE,
            open_share: OPEN_SHARE,
            max_depth: 14,
            max_per_dir: 400,
            label_min_w: 44.0,
            label_min_h: 20.0,
        }
    }
}

/// The `layout` command's request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutRequest {
    /// The folder shown as the root of the map.
    pub view: NodeId,
    /// Canvas size in CSS px.
    pub width: f32,
    pub height: f32,
    pub size_mode: SizeMode,
    pub style: LayoutStyle,
}

impl LayoutRequest {
    /// A request for `view` on a `width` x `height` canvas with the default
    /// style and on-disk sizes.
    pub fn new(view: NodeId, width: f32, height: f32) -> Self {
        LayoutRequest {
            view,
            width,
            height,
            size_mode: SizeMode::Disk,
            style: LayoutStyle::default(),
        }
    }
}

/// Pseudo tiles for the volume root (rule 7), used only when the view is
/// the tree root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RootExtras {
    /// Free bytes on the volume.
    pub free: Option<u64>,
    /// `used - scanned` while a scan runs; shown only above 64 MiB.
    pub unscanned: Option<u64>,
}

/// Computes layouts; keeps the rank hysteresis state between them.
#[derive(Default)]
pub struct LayoutEngine {
    ranks: HashMap<u32, Rank, BuildHasherDefault<IdHasher>>,
    /// Main category of closed folders (rule 6), by folder id.
    tints: HashMap<u32, Tint, BuildHasherDefault<IdHasher>>,
    generation: u32,
    /// Protected node ids (sorted) and the tree length they were found at.
    protected: Option<(usize, Vec<u32>)>,
    scratch: Scratch,
}

/// Rank entries unused for this many layouts may be dropped.
const RANK_TTL: u32 = 64;

#[derive(Clone, Copy)]
struct Rank {
    size: u64,
    used: u32,
}

/// A closed folder's main category, and the size it was found at.
#[derive(Clone, Copy)]
struct Tint {
    size: u64,
    mode: SizeMode,
    cat: u8,
    used: u32,
}

impl LayoutEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets all state; call when a new scan starts.
    pub fn reset(&mut self) {
        self.ranks.clear();
        self.tints.clear();
        self.protected = None;
    }

    /// Lays out `req.view` and returns the encoded buffer (see the module
    /// docs). An invalid, deleted or non-folder view, or an empty canvas,
    /// gives a buffer with no records. `rev` is echoed in the header.
    pub fn layout(
        &mut self,
        tree: &Tree,
        req: &LayoutRequest,
        extras: RootExtras,
        rev: u32,
    ) -> Vec<u8> {
        self.generation = self.generation.wrapping_add(1);
        let protected = match self.protected.take() {
            Some((len, ids)) if len == tree.len() => ids,
            _ => crate::delete::protected_nodes(tree),
        };
        let mut run = Run {
            tree,
            mode: req.size_mode,
            st: Style::new(&req.style),
            ranks: &mut self.ranks,
            tints: &mut self.tints,
            generation: self.generation,
            protected: &protected,
            recycle_bin: recycle_bin(tree),
            s: std::mem::take(&mut self.scratch),
            touched: 0,
        };
        let bytes_per_px = run.run(req, extras);
        let buf = run.encode(req, rev, bytes_per_px);
        let touched = run.touched;
        self.scratch = run.s;
        self.protected = Some((tree.len(), protected));
        // Keep the hysteresis state bounded by what is actually laid out.
        let g = self.generation;
        if self.ranks.len() > 4 * touched + 4096 {
            self.ranks.retain(|_, r| g.wrapping_sub(r.used) < RANK_TTL);
        }
        if self.tints.len() > 4 * touched + 4096 {
            self.tints.retain(|_, t| g.wrapping_sub(t.used) < RANK_TTL);
        }
        buf
    }
}

/// The `$Recycle.Bin` folder, if the tree root is a volume root.
fn recycle_bin(tree: &Tree) -> Option<NodeId> {
    if !crate::delete::is_root_name(&tree.name(NodeId::ROOT)) {
        return None;
    }
    tree.find_child(NodeId::ROOT, b"$Recycle.Bin")
}

/// Multiplicative hashing for `u32` node ids.
#[derive(Default, Clone, Copy)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(8) ^ b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }

    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.0 = (n as u64 ^ 0x5555_5555).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// The style in f64, sanitised.
struct Style {
    outer: f64,
    pad: f64,
    half_gap: f64,
    header: f64,
    min_header_w: f64,
    min_area: f64,
    /// `min_area * recurse`.
    open_min: f64,
    open_share: f64,
    /// What a folder's area must be above to open: the larger of `open_min`
    /// and `open_share` of the map, set once the map is known.
    open_area: f64,
    max_depth: u8,
    max_per_dir: usize,
    label_w: f64,
    label_h: f64,
}

impl Style {
    fn new(s: &LayoutStyle) -> Style {
        let v = |x: f32| {
            if x.is_finite() && x > 0.0 {
                x as f64
            } else {
                0.0
            }
        };
        let min_area = v(s.min_area).max(1.0);
        Style {
            outer: v(s.outer),
            pad: v(s.pad),
            half_gap: v(s.gap) / 2.0,
            header: v(s.header).min(255.0),
            min_header_w: v(s.min_header_w),
            min_area,
            open_min: min_area * v(s.recurse),
            open_share: v(s.open_share).min(1.0),
            open_area: 0.0,
            max_depth: s.max_depth,
            // Room for UNSCANNED, the group and FREE: the top level never
            // goes over the cap.
            max_per_dir: (s.max_per_dir as usize).clamp(1, MAX_RECORDS - 3),
            label_w: v(s.label_min_w),
            label_h: v(s.label_min_h),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct BoxF {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Clone, Copy)]
struct Kid {
    size: u64,
    rank: u64,
    node: NodeId,
}

/// Rank descending, then id ascending: a total order.
#[inline]
fn by_rank(a: &Kid, b: &Kid) -> Ordering {
    b.rank.cmp(&a.rank).then(a.node.cmp(&b.node))
}

#[derive(Clone, Copy)]
enum ItemKind {
    Node,
    /// `node` is the folder whose children were merged.
    Group {
        count: u32,
        cat: u8,
    },
    Free,
    Unscanned,
}

#[derive(Clone, Copy)]
struct Item {
    kind: ItemKind,
    node: NodeId,
    size: u64,
}

/// One record before encoding.
struct Rec {
    id: u32,
    parent: u32,
    target: u32,
    count: u32,
    b: BoxF,
    size: u64,
    depth: u8,
    kind: u8,
    cat: u8,
    flags: u8,
    header: u8,
}

/// A folder tile waiting to be opened.
struct Pending {
    area: f64,
    node: NodeId,
    rec: usize,
    depth: u8,
    inner: BoxF,
}

impl Ord for Pending {
    fn cmp(&self, other: &Self) -> Ordering {
        self.area
            .total_cmp(&other.area)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl PartialOrd for Pending {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Pending {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Pending {}

/// Buffers reused between layouts.
#[derive(Default)]
struct Scratch {
    kids: Vec<Kid>,
    items: Vec<Item>,
    areas: Vec<f64>,
    boxes: Vec<BoxF>,
    cells: Vec<(Item, BoxF)>,
    recs: Vec<Rec>,
    heap: BinaryHeap<Pending>,
    stack: Vec<NodeId>,
}

/// One layout in progress.
struct Run<'a> {
    tree: &'a Tree,
    mode: SizeMode,
    st: Style,
    ranks: &'a mut HashMap<u32, Rank, BuildHasherDefault<IdHasher>>,
    tints: &'a mut HashMap<u32, Tint, BuildHasherDefault<IdHasher>>,
    generation: u32,
    protected: &'a [u32],
    recycle_bin: Option<NodeId>,
    s: Scratch,
    touched: usize,
}

impl Run<'_> {
    /// Fills `s.recs`; returns bytes per px².
    fn run(&mut self, req: &LayoutRequest, extras: RootExtras) -> f64 {
        self.s.recs.clear();
        self.s.heap.clear();
        let tree = self.tree;
        let view = req.view;
        let ok = req.width.is_finite()
            && req.height.is_finite()
            && view.index() < tree.len()
            && view.0 & ids::GROUP_BIT == 0
            && tree.node(view).is_dir()
            && !tree.is_deleted(view);
        if !ok {
            return 0.0;
        }
        let o = self.st.outer;
        let map = BoxF {
            x: o,
            y: o,
            w: req.width as f64 - 2.0 * o,
            h: req.height as f64 - 2.0 * o,
        };
        if !(map.w >= 1.0 && map.h >= 1.0) {
            return 0.0;
        }
        self.st.open_area = self.st.open_min.max(self.st.open_share * map.w * map.h);
        let (unscanned, free) = if view == NodeId::ROOT {
            (
                extras.unscanned.filter(|&u| u > UNSCANNED_MIN).unwrap_or(0),
                extras.free.unwrap_or(0),
            )
        } else {
            (0, 0)
        };
        let area = map.w * map.h;
        let total = self.collect(view, area, unscanned, free);
        self.place(map);
        let cells = std::mem::take(&mut self.s.cells);
        self.emit(&cells, ids::NONE, 1);
        self.s.cells = cells;

        while let Some(p) = self.s.heap.pop() {
            if !self.open(p) {
                // Over the cap: everything still waiting stays solid.
                for q in self.s.heap.drain() {
                    self.s.recs[q.rec].kind = kind::DIR_LEAF;
                }
                break;
            }
        }
        // Rule 6. Closed folders never contain each other, so this walks
        // each node at most once.
        for i in 0..self.s.recs.len() {
            let r = &self.s.recs[i];
            if r.kind == kind::DIR_LEAF {
                let (id, size) = (NodeId(r.id), r.size);
                self.s.recs[i].cat = self.tint(id, size);
            }
        }
        total as f64 / area
    }

    /// Lays out the children of a folder tile. Returns false at the cap.
    fn open(&mut self, p: Pending) -> bool {
        let tree = self.tree;
        let area = p.inner.w * p.inner.h;
        let mut folder = p.node;
        loop {
            self.collect(folder, area, 0, 0);
            self.place(p.inner);
            match self.s.cells.as_slice() {
                [(it, _)] if matches!(it.kind, ItemKind::Node) && tree.node(it.node).is_dir() => {
                    folder = it.node; // fold the chain (rule 4)
                }
                _ => break,
            }
        }
        let rec = &mut self.s.recs[p.rec];
        rec.target = folder.0;
        if self.s.cells.is_empty() {
            rec.kind = kind::DIR_LEAF;
            return true;
        }
        if self.s.recs.len() + self.s.cells.len() > MAX_RECORDS {
            self.s.recs[p.rec].kind = kind::DIR_LEAF;
            return false;
        }
        let cells = std::mem::take(&mut self.s.cells);
        self.emit(&cells, p.node.0, p.depth.saturating_add(1));
        self.s.cells = cells;
        true
    }

    /// The rank size of a node, with 25 % hysteresis (rule 1).
    #[inline]
    fn rank(&mut self, id: NodeId, size: u64) -> u64 {
        self.touched += 1;
        match self.ranks.entry(id.0) {
            Entry::Occupied(mut e) => {
                let r = e.get_mut();
                if size.abs_diff(r.size) > r.size / 4 {
                    r.size = size;
                }
                r.used = self.generation;
                r.size
            }
            Entry::Vacant(e) => {
                e.insert(Rank {
                    size,
                    used: self.generation,
                });
                size
            }
        }
    }

    /// The category with the most bytes below a closed folder of `size`
    /// (rule 6), from the cache while the size stays within 1/8 of the size
    /// it was found at.
    fn tint(&mut self, dir: NodeId, size: u64) -> u8 {
        let (mode, generation) = (self.mode, self.generation);
        if let Some(t) = self.tints.get_mut(&dir.0)
            && t.mode == mode
            && size.abs_diff(t.size) <= t.size / 8
        {
            t.used = generation;
            return t.cat;
        }
        let tree = self.tree;
        let mut tally = [0u64; category::COUNT];
        let stack = &mut self.s.stack;
        stack.clear();
        stack.push(dir);
        while let Some(d) = stack.pop() {
            let mut c = tree.node(d).first_child;
            while !c.is_none() {
                let n = tree.node(c);
                if !n.is_deleted() {
                    if n.is_dir() {
                        stack.push(c);
                    } else {
                        let cat = category::of(&n);
                        if let Some(t) = tally.get_mut(cat as usize) {
                            *t += mode.of(&n);
                        }
                    }
                }
                c = n.next_sibling;
            }
        }
        let cat = main_category(&tally);
        self.tints.insert(
            dir.0,
            Tint {
                size,
                mode,
                cat,
                used: generation,
            },
        );
        cat
    }

    /// Fills `s.items` with what `folder` shows in a box of `area` px², in
    /// paint order (rules 1, 2, 7). Returns the total size.
    fn collect(&mut self, folder: NodeId, area: f64, unscanned: u64, free: u64) -> u64 {
        let tree = self.tree;
        let mode = self.mode;
        let dir = tree.node(folder);
        // The threshold comes from the folder's own size (the sum over its
        // children), so one pass over the siblings sorts them out; the
        // areas use the sum actually seen.
        let expect = mode.of(&dir).saturating_add(unscanned).saturating_add(free);
        let threshold = (self.st.min_area * expect as f64 / area).ceil() as u64;
        self.s.kids.clear();
        self.s.items.clear();
        let mut total = unscanned.saturating_add(free);
        let (mut small_n, mut small_sum) = (0u32, 0u64);
        let mut lone = None;
        let mut tally = [0u64; category::COUNT];
        let mut c = dir.first_child;
        while !c.is_none() {
            // Siblings added in one batch have consecutive ids; stepping
            // by index lets the CPU run ahead of the sibling links.
            loop {
                let n = tree.node(c);
                let size = mode.of(&n);
                if size > 0 && !n.is_deleted() {
                    total = total.saturating_add(size);
                    if size >= threshold {
                        self.s.kids.push(Kid {
                            size,
                            rank: 0,
                            node: c,
                        });
                    } else {
                        small_n += 1;
                        small_sum = small_sum.saturating_add(size);
                        lone = Some(Kid {
                            size,
                            rank: 0,
                            node: c,
                        });
                        let cat = category::of(&n);
                        if (cat as usize) < category::COUNT {
                            tally[cat as usize] += size;
                        }
                    }
                }
                let next = n.next_sibling;
                if next.0 != c.0.wrapping_add(1) {
                    c = next;
                    break;
                }
                c = NodeId(c.0 + 1);
            }
        }
        if small_n == 1 {
            // A group of one would hide a name for nothing.
            self.s.kids.extend(lone);
            (small_n, small_sum, tally) = (0, 0, [0; category::COUNT]);
        }
        let mut kids = std::mem::take(&mut self.s.kids);
        for k in kids.iter_mut() {
            k.rank = self.rank(k.node, k.size);
        }
        if kids.len() > self.st.max_per_dir {
            let keep = self.st.max_per_dir;
            kids.select_nth_unstable_by(keep, by_rank);
            for k in &kids[keep..] {
                small_n += 1;
                small_sum = small_sum.saturating_add(k.size);
                let cat = category::of(&tree.node(k.node));
                if (cat as usize) < category::COUNT {
                    tally[cat as usize] += k.size;
                }
            }
            kids.truncate(keep);
        }
        kids.sort_unstable_by(by_rank);

        let items = &mut self.s.items;
        if unscanned > 0 {
            items.push(Item {
                kind: ItemKind::Unscanned,
                node: folder,
                size: unscanned,
            });
        }
        items.extend(kids.iter().map(|k| Item {
            kind: ItemKind::Node,
            node: k.node,
            size: k.size,
        }));
        if small_n > 0 {
            items.push(Item {
                kind: ItemKind::Group {
                    count: small_n,
                    cat: main_category(&tally),
                },
                node: folder,
                size: small_sum,
            });
        }
        if free > 0 {
            items.push(Item {
                kind: ItemKind::Free,
                node: folder,
                size: free,
            });
        }
        self.s.kids = kids;
        total
    }

    /// Squarifies `s.items` into `b`, insets every cell by `gap / 2` and
    /// keeps those at least 1 px wide and tall in `s.cells` (rule 3).
    fn place(&mut self, b: BoxF) {
        let s = &mut self.s;
        s.cells.clear();
        let total: f64 = s.items.iter().map(|i| i.size as f64).sum();
        if total <= 0.0 {
            return;
        }
        let scale = b.w * b.h / total;
        s.areas.clear();
        s.areas
            .extend(s.items.iter().map(|i| i.size as f64 * scale));
        squarify(&s.areas, b, &mut s.boxes);
        let g = self.st.half_gap;
        for (it, c) in s.items.iter().zip(&s.boxes) {
            let r = BoxF {
                x: c.x + g,
                y: c.y + g,
                w: c.w - 2.0 * g,
                h: c.h - 2.0 * g,
            };
            if r.w >= 1.0 && r.h >= 1.0 {
                s.cells.push((*it, r));
            }
        }
    }

    /// Appends records for `cells` and queues the folders that open.
    fn emit(&mut self, cells: &[(Item, BoxF)], parent: u32, depth: u8) {
        let tree = self.tree;
        for &(it, b) in cells {
            let mut rec = Rec {
                id: it.node.0,
                parent,
                target: it.node.0,
                count: 0,
                b,
                size: it.size,
                depth,
                kind: kind::FILE,
                cat: category::NONE,
                flags: 0,
                header: 0,
            };
            match it.kind {
                ItemKind::Free => {
                    (rec.id, rec.target, rec.kind) = (ids::FREE, ids::FREE, kind::FREE);
                }
                ItemKind::Unscanned => {
                    (rec.id, rec.target, rec.kind) =
                        (ids::UNSCANNED, ids::UNSCANNED, kind::UNSCANNED);
                }
                ItemKind::Group { count, cat } => {
                    rec.id = ids::GROUP_BIT | it.node.0;
                    (rec.count, rec.cat, rec.kind) = (count, cat, kind::GROUP);
                }
                ItemKind::Node => {
                    let n = tree.node(it.node);
                    rec.flags = self.flags(it.node, &n);
                    if n.is_dir() {
                        let st = &self.st;
                        let header =
                            st.header > 0.0 && b.h >= st.header * 2.2 && b.w >= st.min_header_w;
                        let top = if header { st.header } else { st.pad };
                        rec.header = if header { st.header.round() as u8 } else { 0 };
                        let inner = BoxF {
                            x: b.x + st.pad,
                            y: b.y + top,
                            w: b.w - 2.0 * st.pad,
                            h: b.h - top - st.pad,
                        };
                        let opens = depth < st.max_depth
                            && (header || st.header <= 0.0)
                            && b.w * b.h > st.open_area
                            && inner.w >= 3.0
                            && inner.h >= 3.0;
                        if opens {
                            rec.kind = kind::DIR;
                            self.s.heap.push(Pending {
                                area: b.w * b.h,
                                node: it.node,
                                rec: self.s.recs.len(),
                                depth,
                                inner,
                            });
                        } else {
                            rec.kind = kind::DIR_LEAF;
                        }
                    } else {
                        rec.cat = category::of(&n);
                    }
                }
            }
            self.s.recs.push(rec);
        }
    }

    fn flags(&self, id: NodeId, n: &Node) -> u8 {
        const MAP: [(NodeFlags, u8); 4] = [
            (NodeFlags::INACCESSIBLE, flags::NO_ACCESS),
            (NodeFlags::CLOUD, flags::CLOUD),
            (NodeFlags::HARDLINK, flags::HARDLINK),
            (NodeFlags::REPARSE, flags::REPARSE),
        ];
        let mut f = 0;
        for (nf, bit) in MAP {
            if n.flags.intersects(nf) {
                f |= bit;
            }
        }
        if self.protected.binary_search(&id.0).is_ok() {
            f |= flags::PROTECTED;
        }
        if self.recycle_bin == Some(id) {
            f |= flags::RECYCLE_BIN;
        }
        f
    }

    /// Writes the buffer, adding names to the pool.
    fn encode(&self, req: &LayoutRequest, rev: u32, bytes_per_px: f64) -> Vec<u8> {
        let tree = self.tree;
        let recs = &self.s.recs;
        let mut names: Vec<u8> = Vec::new();
        let mut out = Vec::with_capacity(HEADER_SIZE + (RECORD_SIZE + 16) * recs.len());
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(HEADER_SIZE as u16).to_le_bytes());
        out.extend_from_slice(&(recs.len() as u32).to_le_bytes());
        out.extend_from_slice(&(RECORD_SIZE as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // namesLen, patched below
        out.extend_from_slice(&req.view.0.to_le_bytes());
        out.extend_from_slice(&rev.to_le_bytes());
        out.extend_from_slice(&(bytes_per_px as f32).to_le_bytes());
        for r in recs {
            let named = matches!(r.kind, kind::FILE | kind::DIR | kind::DIR_LEAF)
                && (r.header > 0 || (r.b.w >= self.st.label_w && r.b.h >= self.st.label_h));
            let (off, len) = if named {
                let start = names.len();
                push_name(&mut names, tree, NodeId(r.id), NodeId(r.target));
                (start as u32, (names.len() - start) as u16)
            } else {
                (0, 0)
            };
            out.extend_from_slice(&r.id.to_le_bytes());
            out.extend_from_slice(&r.parent.to_le_bytes());
            out.extend_from_slice(&r.target.to_le_bytes());
            out.extend_from_slice(&r.count.to_le_bytes());
            for v in [r.b.x, r.b.y, r.b.w, r.b.h] {
                out.extend_from_slice(&(v as f32).to_le_bytes());
            }
            out.extend_from_slice(&r.size.to_le_bytes());
            out.extend_from_slice(&off.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&[r.depth, r.kind, r.cat, r.flags, r.header, 0]);
        }
        out[16..20].copy_from_slice(&(names.len() as u32).to_le_bytes());
        out.extend_from_slice(&names);
        out
    }
}

/// The category with the most bytes in `tally`; on a tie, the lower
/// category; [`category::NONE`] when it is all zero.
fn main_category(tally: &[u64; category::COUNT]) -> u8 {
    let (best, bytes) =
        tally.iter().enumerate().fold(
            (0, 0u64),
            |acc, (i, &b)| if b > acc.1 { (i, b) } else { acc },
        );
    if bytes > 0 {
        best as u8
    } else {
        category::NONE
    }
}

/// Appends the UTF-8 name of a tile: `top`'s name, or for a folded chain
/// the names from `top` down to `bottom` joined by ` › `. At most 65,535
/// bytes, cut at a character boundary.
fn push_name(out: &mut Vec<u8>, tree: &Tree, top: NodeId, bottom: NodeId) {
    let start = out.len();
    let mut chain = vec![bottom];
    if top != bottom {
        for a in tree.ancestors(bottom) {
            chain.push(a);
            if a == top {
                break;
            }
        }
    }
    for (i, &n) in chain.iter().rev().enumerate() {
        if i > 0 {
            out.extend_from_slice(" \u{203A} ".as_bytes());
        }
        let raw = tree.name(n);
        match std::str::from_utf8(&raw) {
            Ok(s) => out.extend_from_slice(s.as_bytes()),
            Err(_) => out.extend_from_slice(wtf8::to_string_lossy(&raw).as_bytes()),
        }
    }
    if out.len() - start > u16::MAX as usize {
        let mut end = start + u16::MAX as usize;
        while end > start && (out[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        out.truncate(end);
    }
}

/// The worst aspect ratio in a row of total area `sum` laid along a side
/// of length `side`, given its smallest and largest areas.
#[inline]
fn worst(sum: f64, lo: f64, hi: f64, side: f64) -> f64 {
    let (s2, w2) = (sum * sum, side * side);
    (w2 * hi / s2).max(s2 / (w2 * lo))
}

/// Squarified layout of `areas` (which sum to the area of `b`) into `b`.
/// Writes one box per area, in order, to `out`.
fn squarify(areas: &[f64], b: BoxF, out: &mut Vec<BoxF>) {
    out.clear();
    let BoxF {
        mut x,
        mut y,
        mut w,
        mut h,
    } = b;
    let n = areas.len();
    let mut i = 0;
    while i < n {
        let side = w.min(h);
        let mut j = i + 1;
        let mut sum = areas[i];
        if side > 0.0 && sum > 0.0 {
            let (mut lo, mut hi) = (sum, sum);
            let mut cur = worst(sum, lo, hi, side);
            while j < n {
                let a = areas[j];
                let next = worst(sum + a, lo.min(a), hi.max(a), side);
                if next > cur {
                    break;
                }
                (sum, lo, hi, cur) = (sum + a, lo.min(a), hi.max(a), next);
                j += 1;
            }
        } else {
            // Degenerate space: put everything left in one row.
            sum = areas[i..].iter().sum();
            j = n;
        }
        let last = j == n;
        let row = &areas[i..j];
        if w >= h {
            // A column on the left, items top to bottom.
            let t = if last || h <= 0.0 {
                w
            } else {
                (sum / h).min(w)
            };
            place_row(
                row,
                sum,
                h,
                |s, e| BoxF {
                    x,
                    y: y + s,
                    w: t,
                    h: e - s,
                },
                out,
            );
            x += t;
            w -= t;
        } else {
            // A row at the top, items left to right.
            let t = if last || w <= 0.0 {
                h
            } else {
                (sum / w).min(h)
            };
            place_row(
                row,
                sum,
                w,
                |s, e| BoxF {
                    x: x + s,
                    y,
                    w: e - s,
                    h: t,
                },
                out,
            );
            y += t;
            h -= t;
        }
        i = j;
    }
}

/// Splits a strip of length `len` among `row` in proportion to the areas;
/// the last item ends exactly at `len`, so rounding leaves no gap.
fn place_row(row: &[f64], sum: f64, len: f64, mk: impl Fn(f64, f64) -> BoxF, out: &mut Vec<BoxF>) {
    let mut acc = 0.0;
    let mut start = 0.0;
    for (k, &a) in row.iter().enumerate() {
        acc += a;
        let end = if k + 1 == row.len() || sum <= 0.0 {
            len
        } else {
            len * acc / sum
        };
        out.push(mk(start, end));
        start = end;
    }
}

// ---------------------------------------------------------------------------
// Decoding (tests, tools)

/// A decoded layout buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub view: u32,
    pub rev: u32,
    pub bytes_per_px: f32,
    pub records: Vec<Record>,
}

/// A decoded record.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub id: u32,
    pub parent: u32,
    pub target: u32,
    pub count: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub size: u64,
    /// `None` when no name was sent.
    pub name: Option<String>,
    pub depth: u8,
    pub kind: u8,
    pub cat: u8,
    pub flags: u8,
    pub header: u8,
}

/// Parses and validates a layout buffer.
pub fn decode(buf: &[u8]) -> Result<Decoded> {
    let bad = |what: &str| Error::Corrupt(format!("layout buffer: {what}"));
    if buf.len() < HEADER_SIZE {
        return Err(bad("shorter than the header"));
    }
    let u16_at = |o: usize| u16::from_le_bytes([buf[o], buf[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    let f32_at = |o: usize| f32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    if u32_at(0) != MAGIC || u16_at(4) != VERSION {
        return Err(bad("bad magic or version"));
    }
    let hdr = u16_at(6) as usize;
    let count = u32_at(8) as usize;
    let rec = u32_at(12) as usize;
    let names_len = u32_at(16) as usize;
    if hdr < HEADER_SIZE || rec < RECORD_SIZE {
        return Err(bad("header or record too short"));
    }
    let names_at = count
        .checked_mul(rec)
        .and_then(|n| n.checked_add(hdr))
        .ok_or_else(|| bad("count overflows"))?;
    if names_at.checked_add(names_len) != Some(buf.len()) {
        return Err(bad("length does not match count and namesLen"));
    }
    let names = &buf[names_at..];
    let mut records = Vec::with_capacity(count);
    for i in 0..count {
        let o = hdr + i * rec;
        let (off, len) = (u32_at(o + 40) as usize, u16_at(o + 44) as usize);
        let name = if len == 0 {
            None
        } else {
            let bytes = names
                .get(off..off + len)
                .ok_or_else(|| bad("name outside the pool"))?;
            Some(String::from_utf8(bytes.to_vec()).map_err(|_| bad("name is not UTF-8"))?)
        };
        records.push(Record {
            id: u32_at(o),
            parent: u32_at(o + 4),
            target: u32_at(o + 8),
            count: u32_at(o + 12),
            x: f32_at(o + 16),
            y: f32_at(o + 20),
            w: f32_at(o + 24),
            h: f32_at(o + 28),
            size: u32_at(o + 32) as u64 | ((u32_at(o + 36) as u64) << 32),
            name,
            depth: buf[o + 46],
            kind: buf[o + 47],
            cat: buf[o + 48],
            flags: buf[o + 49],
            header: buf[o + 50],
        });
    }
    Ok(Decoded {
        view: u32_at(20),
        rev: u32_at(24),
        bytes_per_px: f32_at(28),
        records,
    })
}

/// The `bytesPerPx` field of an encoded layout's header: bytes per px² at
/// that view. `None` if `buf` is too short to be a layout.
pub fn header_bytes_per_px(buf: &[u8]) -> Option<f32> {
    buf.get(28..32)
        .map(|b| f32::from_le_bytes(b.try_into().expect("4 bytes")))
}

/// True when two encoded layouts draw the same picture: the same rects in
/// the same order, with the same ids, names, kinds, categories, flags,
/// headers and item counts, the same size labels (see [`size_label`]), and
/// no edge more than `tolerance` CSS px from where it was. The revision and
/// `bytesPerPx` are not compared. Live updates use it to skip redraws that
/// would not change a single visible pixel by more than `tolerance`.
/// Buffers that do not decode are never the same.
pub fn same_picture(a: &[u8], b: &[u8], tolerance: f32) -> bool {
    let (Ok(a), Ok(b)) = (decode(a), decode(b)) else {
        return false;
    };
    let near = |p: f32, q: f32| (p - q).abs() <= tolerance;
    a.view == b.view
        && a.records.len() == b.records.len()
        && a.records.iter().zip(&b.records).all(|(r, s)| {
            r.id == s.id
                && r.parent == s.parent
                && r.target == s.target
                && r.count == s.count
                && r.depth == s.depth
                && r.kind == s.kind
                && r.cat == s.cat
                && r.flags == s.flags
                && r.header == s.header
                && r.name == s.name
                // only named rects show their size
                && (r.name.is_none() || size_label(r.size) == size_label(s.size))
                && near(r.x, s.x)
                && near(r.y, s.y)
                && near(r.x + r.w, s.x + s.w)
                && near(r.y + r.h, s.y + s.h)
        })
}

/// What the UI's size label shows for `bytes`, as (unit and decimals,
/// rounded value): two sizes with the same key read the same. Mirrors
/// `fmtBytes` in `app/src/lib/format.ts`; keep the two in step.
pub fn size_label(bytes: u64) -> (u8, u64) {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;
    let b = bytes as f64;
    let r = |v: f64| v.round() as u64;
    if b >= TB {
        (0, r(b / TB * 100.0))
    } else if b >= 100.0 * GB {
        (1, r(b / GB))
    } else if b >= GB {
        (2, r(b / GB * 10.0))
    } else if b >= 10.0 * MB {
        (3, r(b / MB))
    } else if b >= MB {
        (4, r(b / MB * 10.0))
    } else if b >= KB {
        (5, r(b / KB))
    } else {
        (6, bytes)
    }
}

// ---------------------------------------------------------------------------
// Navigation

/// Breadcrumbs for the `nav` command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nav {
    /// From the tree root down to the node, without the folders that
    /// would show as a single folded tile (rule 4). Names are UTF-8.
    pub crumbs: Vec<(NodeId, String)>,
    /// Where "back" goes: the nearest ancestor that is not such a folder,
    /// or `None` at the root.
    pub up: Option<NodeId>,
}

/// True if a folder's only child with a size is a folder, so that viewing
/// it would show a single tile.
fn single_folder_child(tree: &Tree, dir: NodeId, mode: SizeMode) -> bool {
    let mut it = tree.children(dir).filter(|&c| mode.of(&tree.node(c)) > 0);
    matches!((it.next(), it.next()), (Some(c), None) if tree.node(c).is_dir())
}

/// Breadcrumbs and the "up" target of `id` (rule 4). Costs one pass over
/// the children of each ancestor, stopping at the second non-empty one.
pub fn nav(tree: &Tree, id: NodeId, mode: SizeMode) -> Nav {
    if id.index() >= tree.len() {
        return Nav {
            crumbs: Vec::new(),
            up: None,
        };
    }
    let mut chain: Vec<NodeId> = std::iter::once(id).chain(tree.ancestors(id)).collect();
    chain.reverse();
    let skip = |n: NodeId| n != NodeId::ROOT && n != id && single_folder_child(tree, n, mode);
    let crumbs = chain
        .iter()
        .filter(|&&n| !skip(n))
        .map(|&n| (n, wtf8::to_string_lossy(&tree.name(n))))
        .collect();
    let up = tree.ancestors(id).find(|&a| !skip(a));
    Nav { crumbs, up }
}

#[cfg(test)]
mod tests;
