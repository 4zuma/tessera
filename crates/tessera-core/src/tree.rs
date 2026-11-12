//! The compact in-memory file tree.
//!
//! Every file and directory is one 20-byte slot in a chunked arena, and
//! nodes refer to each other by 32-bit [`NodeId`] instead of pointers.
//! Directories, and the few files whose sizes do not fit a slot, have a
//! 16-byte record more in a second arena. Names live in a separate,
//! compressed [`NamePool`]. Nothing is allocated per node, and the arenas
//! grow in fixed chunks, so growing them never copies the existing nodes
//! and never over-allocates by more than one chunk.
//!
//! Invariants (checked by [`Tree::check_invariants`] in tests):
//! * node 0 is the root directory; it has no parent;
//! * a directory's `size` / `logical` are the sums over its live children;
//! * every directory is stored wide (it has a record in the second arena);
//! * deleted nodes stay in their parent's sibling list but are skipped by
//!   [`Tree::children`]; their descendants are implicitly deleted.

use std::ops::Range;
use std::sync::{Arc, RwLock};

pub use crate::names::{Encoder, Name, NamePool};
use crate::wtf8;

/// Index of a node in the [`Tree`] arena.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct NodeId(pub u32);

impl NodeId {
    /// The root of every tree.
    pub const ROOT: NodeId = NodeId(0);
    /// "No node" (end of a sibling list, parent of the root).
    pub const NONE: NodeId = NodeId(u32::MAX);

    #[inline]
    pub fn is_none(self) -> bool {
        self == Self::NONE
    }

    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Per-node flag bits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(transparent)]
pub struct NodeFlags(pub u16);

impl NodeFlags {
    pub const NONE: NodeFlags = NodeFlags(0);
    /// A directory.
    pub const DIR: NodeFlags = NodeFlags(1 << 0);
    /// Could not be read (access denied or another error). Shown greyed out.
    pub const INACCESSIBLE: NodeFlags = NodeFlags(1 << 1);
    /// Deleted after the scan; skipped by [`Tree::children`].
    pub const DELETED: NodeFlags = NodeFlags(1 << 2);
    /// An additional hard link to a file already counted elsewhere.
    /// Its own `size` and `logical` are 0 so the bytes are counted once.
    pub const HARDLINK: NodeFlags = NodeFlags(1 << 3);
    /// A junction, symbolic link or mount point. Never followed.
    pub const REPARSE: NodeFlags = NodeFlags(1 << 4);
    /// A cloud placeholder (e.g. OneDrive "online-only"). Only locally
    /// present bytes are counted in `size`; it is never opened.
    pub const CLOUD: NodeFlags = NodeFlags(1 << 5);
    pub const HIDDEN: NodeFlags = NodeFlags(1 << 6);
    pub const SYSTEM: NodeFlags = NodeFlags(1 << 7);
    /// NTFS-compressed or sparse: `size` (on disk) may be below `logical`.
    pub const COMPRESSED: NodeFlags = NodeFlags(1 << 8);
    /// An NTFS metadata file such as `$MFT` (only seen by the MFT scanner).
    pub const METAFILE: NodeFlags = NodeFlags(1 << 9);
    /// Not a flag: bits 12..16 hold [`Node::name_cat`]. The tree sets them
    /// whenever it stores a name and ignores them in flags handed to it.
    /// (Bits 10 and 11 are the tree's own and always read as 0.)
    pub const NAME_CAT: NodeFlags = NodeFlags(0xF000);

    /// These flags with [`NodeFlags::NAME_CAT`] set to `code`.
    #[inline]
    pub(crate) const fn with_name_cat(self, code: u8) -> NodeFlags {
        NodeFlags(self.0 & !(Self::NAME_CAT.0 | FORM) | (code as u16 & 0xF) << 12)
    }

    /// These flags with only [`NodeFlags::NAME_CAT`] replaced (for the
    /// tree's stored flags, whose own bits stay).
    #[inline]
    pub(crate) const fn with_name_cat_only(self, code: u8) -> NodeFlags {
        NodeFlags(self.0 & !Self::NAME_CAT.0 | (code as u16 & 0xF) << 12)
    }

    #[inline]
    pub const fn contains(self, other: NodeFlags) -> bool {
        self.0 & other.0 == other.0
    }

    #[inline]
    pub const fn intersects(self, other: NodeFlags) -> bool {
        self.0 & other.0 != 0
    }

    #[inline]
    pub fn insert(&mut self, other: NodeFlags) {
        self.0 |= other.0;
    }

    #[inline]
    pub fn remove(&mut self, other: NodeFlags) {
        self.0 &= !other.0;
    }
}

impl std::ops::BitOr for NodeFlags {
    type Output = NodeFlags;
    #[inline]
    fn bitor(self, rhs: NodeFlags) -> NodeFlags {
        NodeFlags(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for NodeFlags {
    #[inline]
    fn bitor_assign(&mut self, rhs: NodeFlags) {
        self.0 |= rhs.0;
    }
}

impl std::fmt::Debug for NodeFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const NAMES: [&str; 10] = [
            "DIR",
            "INACCESSIBLE",
            "DELETED",
            "HARDLINK",
            "REPARSE",
            "CLOUD",
            "HIDDEN",
            "SYSTEM",
            "COMPRESSED",
            "METAFILE",
        ];
        let mut first = true;
        for (i, name) in NAMES.iter().enumerate() {
            if self.0 & (1 << i) != 0 {
                if !first {
                    f.write_str("|")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        if first {
            f.write_str("NONE")?;
        }
        let cat = self.0 >> 12;
        if cat != 0 {
            write!(f, " cat={cat}")?;
        }
        Ok(())
    }
}

/// One file or directory as the tree stores it: exactly 20 bytes.
///
/// A file keeps its logical length in `val` (32 bits) and two bits in
/// `flags` ([`FORM`]) that say how its size on disk follows from that
/// length: rounded up to the volume's cluster size (nearly every file),
/// rounded up to 8 bytes (what NTFS reports for a file small enough to
/// live in its MFT record), or zero. A directory, a file of 4 GiB or
/// more and a file whose size on disk is none of these (compressed,
/// sparse, preallocated) is *wide*: `val` is then the index of a
/// [`Wide`] record with both sizes in full and the first child.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(crate) struct Slot {
    pub(crate) parent: NodeId,
    pub(crate) next_sibling: NodeId,
    /// Offset of the name in the [`NamePool`].
    pub(crate) name_off: u32,
    /// Length of the name as stored in the [`NamePool`] (compressed, so
    /// not the length of the name itself).
    pub(crate) name_len: u16,
    /// The node's [`NodeFlags`] plus its [`FORM`] bits.
    pub(crate) flags: NodeFlags,
    val: u32,
}

/// The bits of [`Slot::flags`] that say how a node's sizes are stored.
const FORM: u16 = 0b11 << 10;
/// Size on disk = logical length rounded up to the cluster size.
const FORM_CLUSTER: u16 = 0 << 10;
/// Size on disk = logical length rounded up to 8 bytes.
const FORM_QUAD: u16 = 1 << 10;
/// Size on disk = 0.
const FORM_ZERO: u16 = 2 << 10;
/// Both sizes (and the first child) are in a [`Wide`] record.
const FORM_WIDE: u16 = 3 << 10;

impl Slot {
    #[inline]
    fn form(&self) -> u16 {
        self.flags.0 & FORM
    }

    #[inline]
    pub(crate) fn is_wide(&self) -> bool {
        self.form() == FORM_WIDE
    }

    #[inline]
    pub(crate) fn is_dir(&self) -> bool {
        self.flags.contains(NodeFlags::DIR)
    }

    #[inline]
    pub(crate) fn is_deleted(&self) -> bool {
        self.flags.contains(NodeFlags::DELETED)
    }
}

/// The sizes and first child of a directory or of a file that does not fit
/// a [`Slot`]: exactly 16 bytes.
///
/// The two sizes are stored in 48 bits each (up to 256 TiB). Size arithmetic
/// wraps modulo 2^48, so adding and later removing the same amount always
/// cancels out; a total beyond 256 TiB would read wrong, but no Windows volume
/// gets there.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(crate) struct Wide {
    size_lo: u32,
    logical_lo: u32,
    pub(crate) first_child: NodeId,
    size_hi: u16,
    logical_hi: u16,
}

/// The largest size a node holds, plus one.
pub const SIZE_LIMIT: u64 = 1 << 48;
const SIZE_MASK: u64 = SIZE_LIMIT - 1;

const _: () = assert!(std::mem::size_of::<Slot>() == 20);
const _: () = assert!(std::mem::size_of::<Wide>() == 16);

impl Wide {
    /// A record with the given sizes (taken modulo [`SIZE_LIMIT`]).
    #[inline]
    fn new(size: u64, logical: u64, first_child: NodeId) -> Wide {
        Wide {
            size_lo: size as u32,
            logical_lo: logical as u32,
            first_child,
            size_hi: (size >> 32) as u16,
            logical_hi: (logical >> 32) as u16,
        }
    }

    #[inline]
    fn size(&self) -> u64 {
        (self.size_hi as u64) << 32 | self.size_lo as u64
    }

    #[inline]
    fn logical(&self) -> u64 {
        (self.logical_hi as u64) << 32 | self.logical_lo as u64
    }

    #[inline]
    fn set_sizes(&mut self, size: u64, logical: u64) {
        self.size_lo = size as u32;
        self.size_hi = (size >> 32) as u16;
        self.logical_lo = logical as u32;
        self.logical_hi = (logical >> 32) as u16;
    }

    /// Adds to both sizes, wrapping modulo [`SIZE_LIMIT`] (so a negative
    /// delta, `x.wrapping_neg()`, subtracts).
    #[inline]
    pub(crate) fn add_sizes(&mut self, dsize: u64, dlogical: u64) {
        self.set_sizes(
            self.size().wrapping_add(dsize),
            self.logical().wrapping_add(dlogical),
        );
    }
}

/// One file or directory, as [`Tree::node`] hands it out (a copy: the tree
/// stores nodes more compactly, see [`Tree::memory`]).
#[derive(Clone, Copy, Debug)]
pub struct Node {
    size: u64,
    logical: u64,
    pub parent: NodeId,
    pub first_child: NodeId,
    pub next_sibling: NodeId,
    pub flags: NodeFlags,
}

impl Node {
    /// What the node's name says about its category
    /// ([`category::name_code`](crate::category::name_code)); see
    /// [`category::of`](crate::category::of) for the category itself.
    #[inline]
    pub fn name_cat(&self) -> u8 {
        (self.flags.0 >> 12) as u8
    }

    /// Size on disk (allocated bytes). For directories, the sum over live
    /// children.
    #[inline]
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Logical file length. For directories, the sum over live children.
    #[inline]
    pub fn logical(&self) -> u64 {
        self.logical
    }

    #[inline]
    pub fn is_dir(&self) -> bool {
        self.flags.contains(NodeFlags::DIR)
    }

    #[inline]
    pub fn is_deleted(&self) -> bool {
        self.flags.contains(NodeFlags::DELETED)
    }
}

/// A `Vec` split into fixed-size chunks so that pushing never moves
/// existing elements and spare capacity is bounded by one chunk.
pub(crate) struct ChunkedVec<T> {
    chunks: Vec<Vec<T>>,
    len: usize,
}

const NODE_CHUNK_SHIFT: u32 = 16; // 65,536 nodes (1.25 MiB) per chunk
const NODE_CHUNK: usize = 1 << NODE_CHUNK_SHIFT;
const NODE_CHUNK_MASK: usize = NODE_CHUNK - 1;

impl<T> ChunkedVec<T> {
    pub(crate) const fn new() -> Self {
        ChunkedVec {
            chunks: Vec::new(),
            len: 0,
        }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub(crate) fn push(&mut self, value: T) -> usize {
        let idx = self.len;
        if idx & NODE_CHUNK_MASK == 0 {
            self.chunks.push(Vec::with_capacity(NODE_CHUNK));
        }
        // The last chunk always has room: it was created with NODE_CHUNK capacity.
        self.chunks.last_mut().unwrap().push(value);
        self.len += 1;
        idx
    }

    #[inline]
    pub(crate) fn get(&self, idx: usize) -> &T {
        &self.chunks[idx >> NODE_CHUNK_SHIFT][idx & NODE_CHUNK_MASK]
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, idx: usize) -> &mut T {
        &mut self.chunks[idx >> NODE_CHUNK_SHIFT][idx & NODE_CHUNK_MASK]
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.chunks.iter().flat_map(|c| c.iter())
    }

    #[allow(dead_code)] // for bulk builders (MFT)
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.chunks.iter_mut().flat_map(|c| c.iter_mut())
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.chunks.len() * NODE_CHUNK * std::mem::size_of::<T>()
            + self.chunks.capacity() * std::mem::size_of::<Vec<T>>()
    }
}

/// A reusable buffer of directory entries, filled by a scanner for one
/// directory and handed to [`Tree::add_children`] in a single call.
#[derive(Default)]
pub struct EntryBatch {
    entries: Vec<BatchEntry>,
    names: Vec<u8>,
    /// The names encoded ahead of time by [`EntryBatch::encode_names`]:
    /// the encoder, the encodings back to back, and where each one ends.
    encoded_by: Option<Encoder>,
    encoded: Vec<u8>,
    encoded_ends: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
pub struct BatchEntry {
    pub size: u64,
    pub logical: u64,
    pub flags: NodeFlags,
    name_start: u32,
    name_len: u16,
}

impl EntryBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.names.clear();
        self.encoded_by = None;
        self.encoded.clear();
        self.encoded_ends.clear();
    }

    /// Encodes all names for the tree's name pool now (outside the tree's
    /// lock), with the encoder from [`Tree::name_encoder`]. Entries added
    /// afterwards are encoded when the batch is inserted.
    pub fn encode_names(&mut self, enc: &Encoder) {
        self.encoded.clear();
        self.encoded_ends.clear();
        for e in &self.entries {
            let name =
                &self.names[e.name_start as usize..e.name_start as usize + e.name_len as usize];
            enc.encode(name, &mut self.encoded);
            self.encoded_ends.push(self.encoded.len() as u32);
        }
        self.encoded_by = Some(enc.clone());
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds an entry whose name is (possibly ill-formed) UTF-16.
    pub fn push_utf16(&mut self, name: &[u16], size: u64, logical: u64, flags: NodeFlags) {
        let start = self.names.len();
        wtf8::push_utf16(&mut self.names, name);
        self.push_raw(start, size, logical, flags);
    }

    /// Adds an entry whose name is already WTF-8 (or UTF-8).
    pub fn push_wtf8(&mut self, name: &[u8], size: u64, logical: u64, flags: NodeFlags) {
        let start = self.names.len();
        self.names.extend_from_slice(name);
        self.push_raw(start, size, logical, flags);
    }

    fn push_raw(&mut self, start: usize, size: u64, logical: u64, flags: NodeFlags) {
        let len = self.names.len() - start;
        let name_len = u16::try_from(len).expect("name too long");
        let code = crate::category::name_code(&self.names[start..]);
        self.entries.push(BatchEntry {
            size,
            logical,
            flags: flags.with_name_cat(code),
            name_start: start as u32,
            name_len,
        });
    }

    pub fn entries(&self) -> &[BatchEntry] {
        &self.entries
    }

    /// The WTF-8 name of an entry from [`EntryBatch::entries`].
    pub fn name(&self, e: &BatchEntry) -> &[u8] {
        &self.names[e.name_start as usize..e.name_start as usize + e.name_len as usize]
    }
}

/// The whole scanned tree.
pub struct Tree {
    pub(crate) nodes: ChunkedVec<Slot>,
    /// The records of the wide nodes, in the order they became wide.
    pub(crate) wide: ChunkedVec<Wide>,
    /// The volume's cluster size minus one (see [`Tree::set_cluster_size`]).
    cluster_mask: u64,
    pub(crate) names: NamePool,
    /// See [`Tree::changed_bytes`].
    pub(crate) changed: u64,
    /// See [`Tree::touched_since`].
    pub(crate) journal: Journal,
}

/// How many folder changes the journal keeps (16 KiB).
pub const JOURNAL_LEN: usize = 4096;

/// The last [`JOURNAL_LEN`] folders whose entries changed, oldest first, in
/// a ring (see [`Tree::touched_since`]).
#[derive(Default)]
pub(crate) struct Journal {
    ring: Vec<NodeId>,
    /// Entries ever recorded.
    seq: u64,
}

impl Journal {
    #[inline]
    fn record(&mut self, dir: NodeId) {
        const LEN: u64 = JOURNAL_LEN as u64;
        // a folder filled or grown in steps is one entry
        if self.seq > 0 && self.ring[((self.seq - 1) % LEN) as usize] == dir {
            return;
        }
        let i = (self.seq % LEN) as usize;
        if i == self.ring.len() {
            self.ring.push(dir);
        } else {
            self.ring[i] = dir;
        }
        self.seq += 1;
    }
}

/// A tree shared between the scanner threads, the watcher and the UI.
pub type SharedTree = Arc<RwLock<Tree>>;

/// The cluster size of a default NTFS volume.
const DEFAULT_CLUSTER: u64 = 4096;

/// Memory used by a [`Tree`], in bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct TreeMemory {
    /// The nodes, wide records included.
    pub nodes: usize,
    /// How many nodes have a wide record (every directory, and the files
    /// whose sizes do not fit a slot).
    pub wide: usize,
    pub names: usize,
    /// What the names would take uncompressed (not part of the total).
    pub names_raw: usize,
}

impl TreeMemory {
    pub fn total(&self) -> usize {
        self.nodes + self.names
    }
}

impl Tree {
    /// Creates a tree with only the root directory. `root_name` is the full
    /// path of the scanned root as WTF-8, e.g. `C:\` or `D:\Games`.
    pub fn new(root_name: &[u8]) -> Tree {
        let mut names = NamePool::with_dedup();
        let root = names.push(root_name);
        Tree::with_names(names, root, DEFAULT_CLUSTER, NodeId::NONE)
    }

    /// A tree with only the root directory, whose name is already at `root`
    /// (offset, stored length) in `names`.
    pub(crate) fn with_names(
        names: NamePool,
        root: (u32, u16),
        cluster: u64,
        first_child: NodeId,
    ) -> Tree {
        let mut t = Tree {
            nodes: ChunkedVec::new(),
            wide: ChunkedVec::new(),
            cluster_mask: DEFAULT_CLUSTER - 1,
            names,
            changed: 0,
            journal: Journal::default(),
        };
        t.set_cluster_size(cluster);
        t.push_node(
            0,
            0,
            NodeId::NONE,
            first_child,
            NodeId::NONE,
            root,
            NodeFlags::DIR,
        );
        t
    }

    /// Tells the tree the cluster size (allocation unit) of the scanned
    /// volume, a power of two. A file whose size on disk is its length
    /// rounded up to that takes the least room, so scanners call this
    /// before they add anything; any other file is stored correctly all the
    /// same, only wider. Ignored (returns false) once the tree has nodes
    /// below the root, or if `bytes` is not a power of two.
    pub fn set_cluster_size(&mut self, bytes: u64) -> bool {
        if self.nodes.len() > 1 || !bytes.is_power_of_two() {
            return false;
        }
        self.cluster_mask = bytes - 1;
        true
    }

    /// See [`Tree::set_cluster_size`]; 4,096 unless a scanner said otherwise.
    pub fn cluster_size(&self) -> u64 {
        self.cluster_mask + 1
    }

    /// How a file with these sizes fits a [`Slot`], if it does.
    #[inline]
    fn form_for(&self, size: u64, logical: u64) -> Option<u16> {
        if logical > u64::from(u32::MAX) {
            None
        } else if size == (logical + self.cluster_mask) & !self.cluster_mask {
            Some(FORM_CLUSTER)
        } else if size == (logical + 7) & !7 {
            Some(FORM_QUAD)
        } else if size == 0 {
            Some(FORM_ZERO)
        } else {
            None
        }
    }

    /// The flags (with their [`FORM`]) and `val` of a slot for a node with
    /// these sizes; a wide node's record is added here.
    #[inline]
    fn pack(
        &mut self,
        size: u64,
        logical: u64,
        first_child: NodeId,
        flags: NodeFlags,
    ) -> (NodeFlags, u32) {
        let flags = flags.0 & !FORM;
        if flags & NodeFlags::DIR.0 == 0 {
            if let Some(form) = self.form_for(size, logical) {
                return (NodeFlags(flags | form), logical as u32);
            }
        }
        let at = self.wide.push(Wide::new(size, logical, first_child));
        (NodeFlags(flags | FORM_WIDE), at as u32)
    }

    /// Appends a node and returns its index. Links are stored as given;
    /// `first_child` only counts for a directory.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub(crate) fn push_node(
        &mut self,
        size: u64,
        logical: u64,
        parent: NodeId,
        first_child: NodeId,
        next_sibling: NodeId,
        name: (u32, u16),
        flags: NodeFlags,
    ) -> usize {
        let (flags, val) = self.pack(size, logical, first_child, flags);
        self.nodes.push(Slot {
            parent,
            next_sibling,
            name_off: name.0,
            name_len: name.1,
            flags,
            val,
        })
    }

    #[inline]
    pub(crate) fn slot(&self, id: NodeId) -> &Slot {
        self.nodes.get(id.index())
    }

    #[inline]
    pub(crate) fn slot_mut(&mut self, id: NodeId) -> &mut Slot {
        self.nodes.get_mut(id.index())
    }

    /// The wide record of `id`, made first if the node has none yet.
    #[inline]
    pub(crate) fn wide_mut(&mut self, id: NodeId) -> &mut Wide {
        let s = *self.slot(id);
        let at = if s.is_wide() {
            s.val as usize
        } else {
            let n = self.node(id);
            let at = self.wide.push(Wide::new(n.size, n.logical, NodeId::NONE));
            let s = self.slot_mut(id);
            s.flags.0 |= FORM_WIDE;
            s.val = at as u32;
            at
        };
        self.wide.get_mut(at)
    }

    /// The head of `id`'s sibling list of children, deleted ones included.
    #[inline]
    pub(crate) fn first_child(&self, id: NodeId) -> NodeId {
        let s = self.slot(id);
        if s.is_wide() {
            self.wide.get(s.val as usize).first_child
        } else {
            NodeId::NONE
        }
    }

    /// Points `id` (a directory) at the head of its children.
    #[inline]
    pub(crate) fn set_first_child(&mut self, id: NodeId, child: NodeId) {
        self.wide_mut(id).first_child = child;
    }

    /// Creates a tree rooted at a filesystem path.
    pub fn for_path(root: &std::path::Path) -> Tree {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            let units: Vec<u16> = root.as_os_str().encode_wide().collect();
            Tree::new(&wtf8::from_utf16(&units))
        }
        #[cfg(not(windows))]
        {
            Tree::new(root.to_string_lossy().as_bytes())
        }
    }

    /// The name pool's encoder once it encodes names; see
    /// [`EntryBatch::encode_names`].
    pub fn name_encoder(&self) -> Option<Encoder> {
        self.names.encoder()
    }

    /// Releases memory only needed while scanning (the name
    /// de-duplication table) and compresses the names again with a table
    /// trained on the whole tree. Called by [`crate::scan::scan`] when it
    /// ends.
    pub fn end_scan(&mut self) {
        self.names.end_dedup();
        self.recompress_names();
    }

    /// Re-encodes every name with a symbol table trained on names drawn
    /// evenly from the whole tree. The pool trains its first table on the
    /// first megabyte of names, which come from a few folders (on Windows,
    /// mostly the system's) and suit the rest of the drive less well. Also
    /// drops names that no node uses any more. Skipped for small trees,
    /// where the first table saw most names anyway, and when a name would
    /// grow past 65,535 bytes; the old names also stay when they take less
    /// room than the new ones would.
    pub fn recompress_names(&mut self) {
        use rayon::prelude::*;

        if self.names.raw_bytes() < 4 * crate::names::TRAIN_AT {
            return;
        }
        // Node indices sorted by name offset: each distinct name is then
        // one run, decoded and encoded once however many nodes share it.
        let mut order: Vec<u64> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| u64::from(n.name_off) << 32 | i as u64)
            .collect();
        order.par_sort_unstable();
        let node_at = |pos: u32| self.nodes.get(order[pos as usize] as u32 as usize);
        // Where each run starts (an empty name shares its offset with the
        // next name, so the length counts too).
        let starts: Vec<u32> = (0..order.len() as u32)
            .filter(|&p| {
                p == 0 || {
                    let (a, b) = (node_at(p - 1), node_at(p));
                    a.name_off != b.name_off || a.name_len != b.name_len
                }
            })
            .collect();
        let name_at = |pos: u32| {
            let n = node_at(pos);
            self.names.get(n.name_off, n.name_len)
        };

        // the sample: every few names, about SAMPLE_BYTES of them
        let every = self
            .names
            .raw_bytes()
            .div_ceil(NamePool::SAMPLE_BYTES)
            .max(1);
        let sample: Vec<Vec<u8>> = starts
            .iter()
            .step_by(every)
            .map(|&p| name_at(p).to_vec())
            .collect();
        let sample: Vec<&[u8]> = sample.iter().map(|v| &v[..]).collect();
        let mut pool = NamePool::trained_on(&sample);
        let enc = pool.encoder().expect("a trained pool encodes");

        // Encode on all threads, a slice of names at a time (the codes back
        // to back, and each name's encoded and plain length), then append
        // them in order and note where each one went.
        let mut placed: Vec<(u32, u16)> = Vec::with_capacity(starts.len());
        for batch in starts.chunks(1 << 20) {
            // per slice: the codes back to back; each name's encoded and plain length
            type Piece = (Vec<u8>, Vec<(u16, u16)>);
            let pieces: Option<Vec<Piece>> = batch
                .par_chunks(64 * 1024)
                .map(|ps| {
                    // sized to come straight from the OS (and go back to it)
                    let mut codes = Vec::with_capacity(ps.len() * 24);
                    let mut lens = Vec::with_capacity(ps.len());
                    for &p in ps {
                        let name = name_at(p);
                        let at = codes.len();
                        enc.encode(&name, &mut codes);
                        // A name over 32 KiB (only the root's path can be
                        // one) may grow past what a stored length holds.
                        let len = u16::try_from(codes.len() - at).ok()?;
                        lens.push((len, name.len() as u16));
                    }
                    Some((codes, lens))
                })
                .collect();
            // Then the names stay as they are: nothing has changed yet.
            let Some(pieces) = pieces else {
                return;
            };
            for (codes, lens) in pieces {
                let mut at = 0;
                for (len, raw) in lens {
                    let end = at + len as usize;
                    placed.push(pool.append_encoded(raw as usize, &codes[at..end]));
                    at = end;
                }
            }
        }

        // Names alike throughout compress as well with the first table:
        // keep the pool that is smaller.
        if pool.stored_bytes() >= self.names.stored_bytes() {
            return;
        }
        // point every node of each run at the new copy
        for (i, &(off, len)) in placed.iter().enumerate() {
            let from = starts[i] as usize;
            let to = starts.get(i + 1).map_or(order.len(), |&p| p as usize);
            for &k in &order[from..to] {
                let n = self.nodes.get_mut(k as u32 as usize);
                n.name_off = off;
                n.name_len = len;
            }
        }
        self.names = pool;
    }

    /// Wraps the tree for sharing between threads.
    pub fn into_shared(self) -> SharedTree {
        Arc::new(RwLock::new(self))
    }

    #[inline]
    pub fn root(&self) -> NodeId {
        NodeId::ROOT
    }

    /// Number of nodes, including deleted ones.
    #[inline]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.nodes.len() == 0
    }

    /// The node `id`: its sizes, links and flags.
    #[inline]
    pub fn node(&self, id: NodeId) -> Node {
        let s = self.slot(id);
        let (size, logical, first_child) = match s.form() {
            FORM_WIDE => {
                let w = self.wide.get(s.val as usize);
                (w.size(), w.logical(), w.first_child)
            }
            form => {
                let logical = u64::from(s.val);
                let size = match form {
                    FORM_CLUSTER => (logical + self.cluster_mask) & !self.cluster_mask,
                    FORM_QUAD => (logical + 7) & !7,
                    _ => 0,
                };
                (size, logical, NodeId::NONE)
            }
        };
        Node {
            size,
            logical,
            parent: s.parent,
            first_child,
            next_sibling: s.next_sibling,
            flags: NodeFlags(s.flags.0 & !FORM),
        }
    }

    /// The node's own name as WTF-8 (for the root: the scanned path).
    #[inline]
    pub fn name(&self, id: NodeId) -> Name<'_> {
        let n = self.slot(id);
        self.names.get(n.name_off, n.name_len)
    }

    /// The node's own name for display.
    pub fn name_lossy(&self, id: NodeId) -> String {
        wtf8::to_string_lossy(&self.name(id))
    }

    /// Live children (deleted ones skipped), in no particular order.
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            tree: self,
            next: self.first_child(id),
        }
    }

    /// The ancestors of `id`, starting with its parent and ending at the root.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut cur = self.slot(id).parent;
        std::iter::from_fn(move || {
            if cur.is_none() {
                return None;
            }
            let out = cur;
            cur = self.slot(cur).parent;
            Some(out)
        })
    }

    /// True if `id` or one of its ancestors is deleted.
    pub fn is_deleted(&self, id: NodeId) -> bool {
        self.slot(id).is_deleted() || self.ancestors(id).any(|a| self.slot(a).is_deleted())
    }

    /// Full path as WTF-8, built by walking up to the root.
    pub fn path_wtf8(&self, id: NodeId) -> Vec<u8> {
        let mut chain: Vec<NodeId> = Vec::with_capacity(16);
        let mut cur = id;
        while !cur.is_none() {
            chain.push(cur);
            cur = self.slot(cur).parent;
        }
        let mut out = Vec::with_capacity(128);
        for (i, &n) in chain.iter().rev().enumerate() {
            if i > 0 && !matches!(out.last(), Some(b'\\') | Some(b'/')) {
                out.push(b'\\');
            }
            out.extend_from_slice(&self.name(n));
        }
        out
    }

    /// Full path as UTF-16 without a terminating NUL, for Win32 calls.
    pub fn path_utf16(&self, id: NodeId) -> Vec<u16> {
        wtf8::to_utf16(&self.path_wtf8(id))
    }

    /// Full path for display.
    pub fn path_lossy(&self, id: NodeId) -> String {
        wtf8::to_string_lossy(&self.path_wtf8(id))
    }

    /// Full path as a `PathBuf`.
    pub fn path(&self, id: NodeId) -> std::path::PathBuf {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&self.path_utf16(id)).into()
        }
        #[cfg(not(windows))]
        {
            self.path_lossy(id).into()
        }
    }

    /// Finds a live child by name, case-insensitively (as NTFS does).
    /// Linear in the number of children; meant for incremental updates.
    pub fn find_child(&self, parent: NodeId, name: &[u8]) -> Option<NodeId> {
        self.children(parent)
            .find(|&c| wtf8::eq_ignore_case(&self.name(c), name))
    }

    /// Resolves a path below the root (components separated by `\` or `/`)
    /// to a live node. `rel` is relative to the root, e.g. `Users\me\a.txt`.
    pub fn lookup_relative(&self, rel: &[u8]) -> Option<NodeId> {
        let mut cur = NodeId::ROOT;
        for comp in rel.split(|&b| b == b'\\' || b == b'/') {
            if comp.is_empty() || comp == b"." {
                continue;
            }
            cur = self.find_child(cur, comp)?;
        }
        Some(cur)
    }

    /// Appends all entries of `batch` as children of `parent` and adds their
    /// sizes to `parent` and all its ancestors. Returns the new node ids,
    /// which are contiguous and in batch order.
    pub fn add_children(&mut self, parent: NodeId, batch: &EntryBatch) -> Range<u32> {
        let start = self.nodes.len() as u32;
        let n = batch.entries.len() as u32;
        if n == 0 {
            return start..start;
        }
        let old_first = self.first_child(parent);
        let (mut dsize, mut dlogical) = (0u64, 0u64);
        let encoded = match &batch.encoded_by {
            Some(enc) if batch.encoded_ends.len() == batch.entries.len() => Some(enc),
            _ => None,
        };
        for (i, e) in batch.entries.iter().enumerate() {
            let name = batch.name(e);
            let (name_off, name_len) = match encoded {
                Some(enc) => {
                    let start = if i == 0 {
                        0
                    } else {
                        batch.encoded_ends[i - 1] as usize
                    };
                    let end = batch.encoded_ends[i] as usize;
                    self.names
                        .push_encoded(name, &batch.encoded[start..end], enc)
                }
                None => self.names.push(name),
            };
            let id = start + i as u32;
            let next = if id + 1 < start + n {
                NodeId(id + 1)
            } else {
                old_first
            };
            self.push_node(
                e.size,
                e.logical,
                parent,
                NodeId::NONE,
                next,
                (name_off, name_len),
                e.flags,
            );
            dsize = dsize.wrapping_add(e.size);
            dlogical = dlogical.wrapping_add(e.logical);
        }
        self.set_first_child(parent, NodeId(start));
        self.add_to_ancestors(parent, dsize, dlogical);
        start..start + n
    }

    /// Adds a single child (used by incremental updates). Returns its id.
    pub fn add_child(
        &mut self,
        parent: NodeId,
        name: &[u8],
        size: u64,
        logical: u64,
        flags: NodeFlags,
    ) -> NodeId {
        let mut b = EntryBatch::new();
        b.push_wtf8(name, size, logical, flags);
        NodeId(self.add_children(parent, &b).start)
    }

    /// Number of live children. Linear in the length of the sibling list
    /// (the count is not stored, to keep the nodes small).
    pub fn child_count(&self, id: NodeId) -> usize {
        self.children(id).count()
    }

    /// Adds `dsize` / `dlogical` (wrapping, so negative deltas work) to
    /// `from` and every ancestor of it. Stops after the first deleted node,
    /// whose bytes are already excluded from its parent.
    #[inline]
    pub(crate) fn add_to_ancestors(&mut self, from: NodeId, dsize: u64, dlogical: u64) {
        // every change of a folder's entries comes through here, also those
        // that move no bytes (an empty file added or removed)
        self.journal.record(from);
        if dsize == 0 && dlogical == 0 {
            return;
        }
        let moved = (dsize as i64)
            .unsigned_abs()
            .max((dlogical as i64).unsigned_abs());
        self.changed = self.changed.wrapping_add(moved);
        let mut cur = from;
        while !cur.is_none() {
            let s = *self.slot(cur);
            self.wide_mut(cur).add_sizes(dsize, dlogical);
            if s.is_deleted() {
                break;
            }
            cur = s.parent;
        }
    }

    /// Bytes added, removed or resized anywhere in the tree so far (the
    /// larger of the on-disk and logical change, summed; a move counts
    /// twice). It only grows: the difference between two readings bounds how
    /// much any folder's total moved in between, so the UI can skip redraws
    /// for changes too small to see. Wraps around after 16 EiB.
    pub fn changed_bytes(&self) -> u64 {
        self.changed
    }

    /// Records that the entries of folder `dir` changed in a way that moves
    /// no bytes (a rename, new flags); see [`Tree::touched_since`].
    #[inline]
    pub(crate) fn touch(&mut self, dir: NodeId) {
        self.journal.record(dir);
    }

    /// Position of the change journal; pass it to [`Tree::touched_since`]
    /// later to learn which folders changed in between.
    pub fn change_seq(&self) -> u64 {
        self.journal.seq
    }

    /// The folders whose entries changed (added, removed, resized, renamed,
    /// moved, new flags) since the journal stood at `seq` on this tree,
    /// oldest first, maybe with repeats. Only the folder that holds the
    /// changed entry is listed: its ancestors' totals changed too. `None`
    /// when the journal no longer reaches back that far (more than
    /// [`JOURNAL_LEN`] changes since, e.g. during a scan): assume that
    /// everything changed.
    pub fn touched_since(&self, seq: u64) -> Option<impl Iterator<Item = NodeId> + '_> {
        const LEN: u64 = JOURNAL_LEN as u64;
        let j = &self.journal;
        if seq > j.seq || j.seq - seq > LEN {
            return None;
        }
        Some((seq..j.seq).map(move |s| j.ring[(s % LEN) as usize]))
    }

    /// Changes a file's sizes and updates all its ancestors by the difference.
    pub fn set_file_size(&mut self, id: NodeId, size: u64, logical: u64) {
        let n = self.node(id);
        debug_assert!(!n.is_dir(), "directory sizes are derived from children");
        let dsize = size.wrapping_sub(n.size());
        let dlogical = logical.wrapping_sub(n.logical());
        let (size, logical) = (size & SIZE_MASK, logical & SIZE_MASK);
        // a file that was ever wide stays wide: its record is there anyway
        match self.form_for(size, logical) {
            Some(form) if !self.slot(id).is_wide() => {
                let s = self.slot_mut(id);
                s.flags.0 = s.flags.0 & !FORM | form;
                s.val = logical as u32;
            }
            _ => self.wide_mut(id).set_sizes(size, logical),
        }
        let (parent, deleted) = (n.parent, n.is_deleted());
        if !deleted {
            self.add_to_ancestors(parent, dsize, dlogical);
        }
    }

    /// Replaces a node's flags (sizes and links are untouched).
    pub fn set_flags(&mut self, id: NodeId, flags: NodeFlags) {
        // a file's flags decide its category (NTFS metafiles)
        let old = *self.slot(id);
        self.touch(if old.parent.is_none() { id } else { old.parent });
        let mut f = flags.with_name_cat((old.flags.0 >> 12) as u8);
        f.remove(NodeFlags::DELETED);
        if old.is_deleted() {
            f.insert(NodeFlags::DELETED);
        }
        f.0 |= old.form();
        self.slot_mut(id).flags = f;
        if f.contains(NodeFlags::DIR) {
            // a directory keeps its sums and children in a wide record
            self.wide_mut(id);
        }
    }

    /// Marks a node deleted and removes its bytes from all ancestors.
    /// The node keeps its own sizes (for display, e.g. "freed 3.2 GB").
    /// Returns false if it was already deleted. The root cannot be deleted.
    pub fn mark_deleted(&mut self, id: NodeId) -> bool {
        if id == NodeId::ROOT || self.is_deleted(id) {
            return false;
        }
        self.slot_mut(id).flags.insert(NodeFlags::DELETED);
        let n = self.node(id);
        let (size, logical, parent) = (n.size(), n.logical(), n.parent);
        self.add_to_ancestors(parent, size.wrapping_neg(), logical.wrapping_neg());
        true
    }

    /// Marks every live child of `dir` deleted (e.g. before rescanning it).
    pub fn clear_children(&mut self, dir: NodeId) {
        let kids: Vec<NodeId> = self.children(dir).collect();
        for k in kids {
            self.mark_deleted(k);
        }
    }

    /// Heap memory held by the tree.
    pub fn memory(&self) -> TreeMemory {
        TreeMemory {
            nodes: self.nodes.heap_bytes() + self.wide.heap_bytes(),
            wide: self.wide.len(),
            names: self.names.heap_bytes(),
            names_raw: self.names.raw_bytes(),
        }
    }

    /// Verifies the size and count invariants of the whole tree.
    /// O(n); meant for tests and debug assertions.
    pub fn check_invariants(&self) -> Result<(), String> {
        let n = self.nodes.len();
        let mut size = vec![0u64; n];
        let mut logical = vec![0u64; n];
        let mut count = vec![0u32; n];
        let mut wide = 0;
        for i in 0..n {
            let node = self.node(NodeId(i as u32));
            let slot = self.nodes.get(i);
            if slot.is_wide() {
                wide += 1;
                if slot.val as usize >= self.wide.len() {
                    return Err(format!("node {i} has bad wide record {}", slot.val));
                }
            } else if node.is_dir() {
                return Err(format!("dir {i} has no wide record"));
            }
            if i == 0 {
                if !node.parent.is_none() {
                    return Err("root has a parent".into());
                }
                continue;
            }
            let p = node.parent.index();
            if p >= n {
                return Err(format!("node {i} has bad parent {p}"));
            }
            if !self.nodes.get(p).is_dir() {
                return Err(format!("node {i} has a non-directory parent {p}"));
            }
            if !node.is_deleted() {
                size[p] = size[p].wrapping_add(node.size()) & SIZE_MASK;
                logical[p] = logical[p].wrapping_add(node.logical()) & SIZE_MASK;
                count[p] += 1;
            }
        }
        if wide != self.wide.len() {
            return Err(format!(
                "{wide} wide nodes but {} wide records",
                self.wide.len()
            ));
        }
        for i in 0..n {
            let id = NodeId(i as u32);
            let node = self.node(id);
            let listed = self.children(id).count() as u32;
            if listed != count[i] {
                return Err(format!(
                    "node {i} ({}) lists {} live children but {} point to it",
                    self.path_lossy(id),
                    listed,
                    count[i]
                ));
            }
            if node.is_dir() && (node.size() != size[i] || node.logical() != logical[i]) {
                return Err(format!(
                    "dir {i} ({}) has size {}/{} but children sum to {}/{}",
                    self.path_lossy(id),
                    node.size(),
                    node.logical(),
                    size[i],
                    logical[i]
                ));
            }
        }
        Ok(())
    }
}

/// Iterator over the live children of a node.
pub struct Children<'a> {
    tree: &'a Tree,
    next: NodeId,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    #[inline]
    fn next(&mut self) -> Option<NodeId> {
        while !self.next.is_none() {
            let id = self.next;
            let n = self.tree.slot(id);
            self.next = n.next_sibling;
            if !n.is_deleted() {
                return Some(id);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tree {
        let mut t = Tree::new(b"C:\\");
        let mut b = EntryBatch::new();
        b.push_wtf8(b"Users", 0, 0, NodeFlags::DIR);
        b.push_wtf8(
            b"pagefile.sys",
            4096 * 100,
            409_000,
            NodeFlags::HIDDEN | NodeFlags::SYSTEM,
        );
        b.push_wtf8(b"Windows", 0, 0, NodeFlags::DIR);
        let r = t.add_children(NodeId::ROOT, &b);
        assert_eq!(r, 1..4);
        b.clear();
        b.push_utf16(
            &"照片.jpg".encode_utf16().collect::<Vec<_>>(),
            8192,
            8000,
            NodeFlags::NONE,
        );
        b.push_wtf8(b"notes.txt", 4096, 12, NodeFlags::NONE);
        t.add_children(NodeId(1), &b);
        t
    }

    #[test]
    fn a_node_takes_20_bytes_and_a_wide_one_16_more() {
        assert_eq!(std::mem::size_of::<Slot>(), 20);
        assert_eq!(std::mem::size_of::<Wide>(), 16);
    }

    #[test]
    fn sizes_use_48_bits_and_wrap_consistently() {
        let mut t = Tree::new(b"X:\\");
        let d = t.add_child(NodeId::ROOT, b"d", 0, 0, NodeFlags::DIR);
        let big = SIZE_LIMIT - 4096;
        let f = t.add_child(d, b"big", big, big - 1, NodeFlags::NONE);
        assert_eq!(t.node(f).size(), big);
        assert_eq!(t.node(f).logical(), big - 1);
        assert_eq!(t.node(NodeId::ROOT).size(), big);
        let g = t.add_child(d, b"small", 4096 * 3, 100, NodeFlags::NONE);
        // past 256 TiB the total wraps, but removing the file again cancels
        assert_eq!(t.node(d).size(), 4096 * 2);
        t.check_invariants().unwrap();
        t.mark_deleted(g);
        assert_eq!(t.node(d).size(), big);
        assert_eq!(t.node(NodeId::ROOT).logical(), big - 1);
        t.set_file_size(f, 1 << 40, 5);
        assert_eq!(t.node(NodeId::ROOT).size(), 1 << 40);
        assert_eq!(t.node(NodeId::ROOT).logical(), 5);
        t.check_invariants().unwrap();
    }

    #[test]
    fn every_pair_of_sizes_reads_back_and_only_odd_ones_are_wide() {
        const G: u64 = 1 << 30;
        // (size on disk, logical, wide?) on a volume with 4 KiB clusters
        let cases = [
            (0, 0, false),
            (4096, 1, false),
            (4096, 4096, false),
            (8192, 4097, false),
            (4 * G, 4 * G - 1, false),
            // resident in the MFT record, as a directory listing reports it
            (24, 17, false),
            (16, 16, false),
            // resident as the MFT scan counts it; cloud placeholders
            (0, 700, false),
            (0, 5 * G, true),
            // compressed, sparse, preallocated
            (8192, 100_000, true),
            (65_536, 10, true),
            (4096, 0, true),
            // 4 GiB and more
            (4 * G, 4 * G, true),
            (300 * G, 300 * G - 5, true),
        ];
        let mut t = Tree::new(b"X:\\");
        let mut b = EntryBatch::new();
        for (i, &(size, logical, _)) in cases.iter().enumerate() {
            b.push_wtf8(format!("f{i}").as_bytes(), size, logical, NodeFlags::HIDDEN);
        }
        b.push_wtf8(b"dir", 0, 0, NodeFlags::DIR);
        let ids = t.add_children(NodeId::ROOT, &b);
        let dir = NodeId(ids.end - 1);
        for (i, &(size, logical, wide)) in cases.iter().enumerate() {
            let id = NodeId(ids.start + i as u32);
            let n = t.node(id);
            assert_eq!((n.size(), n.logical()), (size, logical), "case {i}");
            assert_eq!(t.slot(id).is_wide(), wide, "case {i}");
            assert_eq!(n.flags, NodeFlags::HIDDEN, "case {i}");
            assert!(n.first_child.is_none());
        }
        let wide = cases.iter().filter(|c| c.2).count();
        // the root and `dir` are wide too
        assert_eq!(t.memory().wide, wide + 2);
        t.check_invariants().unwrap();

        // every case to every other one and back, below `dir` too
        let f = t.add_child(dir, b"f", 0, 0, NodeFlags::NONE);
        for &(s0, l0, _) in &cases {
            for &(s1, l1, _) in &cases {
                t.set_file_size(f, s0, l0);
                t.set_file_size(f, s1, l1);
                let n = t.node(f);
                assert_eq!((n.size(), n.logical()), (s1, l1));
                assert_eq!(t.node(dir).size(), s1);
                assert_eq!(t.node(dir).logical(), l1);
            }
        }
        t.check_invariants().unwrap();
        // a file that was wide once keeps its one record
        assert_eq!(t.memory().wide, wide + 3);
        // flags leave the sizes alone, and a file turned folder is wide
        let g = NodeId(ids.start + 5);
        t.set_flags(g, NodeFlags::SYSTEM);
        assert_eq!((t.node(g).size(), t.node(g).logical()), (24, 17));
        assert_eq!(t.node(g).flags, NodeFlags::SYSTEM);
        let e = NodeId(ids.start);
        t.set_flags(e, NodeFlags::DIR);
        t.add_child(e, b"inside", 4096, 10, NodeFlags::NONE);
        assert_eq!(t.node(e).size(), 4096);
        assert_eq!(t.child_count(e), 1);
        t.check_invariants().unwrap();
    }

    #[test]
    fn the_cluster_size_is_set_before_anything_is_added() {
        let mut t = Tree::new(b"E:\\");
        assert_eq!(t.cluster_size(), 4096);
        assert!(!t.set_cluster_size(3000));
        assert!(t.set_cluster_size(128 << 10));
        let a = t.add_child(NodeId::ROOT, b"a", 128 << 10, 5, NodeFlags::NONE);
        let b = t.add_child(NodeId::ROOT, b"b", 4096, 5, NodeFlags::NONE);
        assert!(!t.slot(a).is_wide());
        assert!(t.slot(b).is_wide());
        assert_eq!(t.node(a).size(), 128 << 10);
        assert_eq!(t.node(b).size(), 4096);
        assert!(!t.set_cluster_size(4096));
        assert_eq!(t.node(a).size(), 128 << 10);
        t.check_invariants().unwrap();
    }

    #[test]
    fn sizes_propagate_to_ancestors() {
        let t = sample();
        assert_eq!(t.node(NodeId::ROOT).size(), 409_600 + 8192 + 4096);
        assert_eq!(t.node(NodeId::ROOT).logical(), 409_000 + 8000 + 12);
        assert_eq!(t.node(NodeId(1)).size(), 8192 + 4096);
        assert_eq!(t.child_count(NodeId::ROOT), 3);
        t.check_invariants().unwrap();
    }

    #[test]
    fn paths_and_lookup() {
        let t = sample();
        let photo = t.lookup_relative("users\\照片.JPG".as_bytes()).unwrap();
        assert_eq!(t.path_lossy(photo), "C:\\Users\\照片.jpg");
        assert_eq!(t.path_lossy(NodeId::ROOT), "C:\\");
        assert_eq!(t.path_lossy(NodeId(3)), "C:\\Windows");
        assert!(t.lookup_relative(b"Users/missing").is_none());
        assert_eq!(t.lookup_relative(b""), Some(NodeId::ROOT));
    }

    #[test]
    fn delete_and_resize() {
        let mut t = sample();
        let notes = t.lookup_relative(b"Users\\notes.txt").unwrap();
        t.set_file_size(notes, 8192, 5000);
        assert_eq!(t.node(NodeId(1)).size(), 8192 + 8192);
        t.check_invariants().unwrap();

        assert!(t.mark_deleted(NodeId(1)));
        assert!(!t.mark_deleted(NodeId(1)));
        assert!(
            !t.mark_deleted(notes),
            "descendant of a deleted node is already deleted"
        );
        assert_eq!(t.node(NodeId::ROOT).size(), 409_600);
        assert_eq!(t.child_count(NodeId::ROOT), 2);
        assert_eq!(t.children(NodeId::ROOT).count(), 2);
        assert!(t.is_deleted(notes));
        // Resizing a file under a deleted directory must not touch live sums.
        t.set_file_size(notes, 1, 1);
        t.check_invariants().unwrap();
        assert!(!t.mark_deleted(NodeId::ROOT));
    }

    #[test]
    fn changed_bytes_counts_every_size_change() {
        let mut t = sample();
        let start = t.changed_bytes();
        let notes = t.lookup_relative(b"Users\\notes.txt").unwrap();
        // The larger of the two deltas: on disk 4096 -> 8192, logical 12 -> 5000.
        t.set_file_size(notes, 8192, 5000);
        assert_eq!(t.changed_bytes() - start, 4988);
        // Shrinking counts too.
        t.set_file_size(notes, 4096, 12);
        assert_eq!(t.changed_bytes() - start, 2 * 4988);
        let before = t.changed_bytes();
        t.set_file_size(notes, 4096, 12);
        assert_eq!(t.changed_bytes(), before, "no change, nothing counted");
        let w = t.lookup_relative(b"Windows").unwrap();
        t.add_child(w, b"new.log", 4096, 100, NodeFlags::NONE);
        assert_eq!(t.changed_bytes() - before, 4096);
        let before = t.changed_bytes();
        assert!(t.mark_deleted(NodeId(1)));
        assert_eq!(t.changed_bytes() - before, 8192 + 4096);
    }

    #[test]
    fn journal_lists_the_folders_that_changed() {
        let mut t = sample();
        let users = t.lookup_relative(b"Users").unwrap();
        let w = t.lookup_relative(b"Windows").unwrap();
        let notes = t.lookup_relative(b"Users\\notes.txt").unwrap();
        let seq = t.change_seq();
        assert_eq!(t.touched_since(seq).unwrap().count(), 0);
        // an empty file moves no bytes but is a change
        t.add_child(w, b"empty.log", 0, 0, NodeFlags::NONE);
        // the same folder twice in a row is one entry
        t.set_file_size(notes, 8192, 5000);
        t.set_file_size(notes, 4096, 12);
        t.set_flags(notes, NodeFlags::HIDDEN);
        assert!(t.mark_deleted(w));
        let got: Vec<NodeId> = t.touched_since(seq).unwrap().collect();
        assert_eq!(got, [w, users, NodeId::ROOT]);
        // older positions fall out of the ring
        let seq = t.change_seq();
        for i in 0..=JOURNAL_LEN {
            let parent = if i % 2 == 0 { users } else { NodeId::ROOT };
            t.add_child(parent, format!("f{i}").as_bytes(), 1, 1, NodeFlags::NONE);
        }
        assert!(t.touched_since(seq).is_none());
        assert!(t.touched_since(seq + 1).is_some());
        assert!(t.touched_since(t.change_seq() + 1).is_none());
    }

    #[test]
    fn add_child_after_batch() {
        let mut t = sample();
        let w = t.lookup_relative(b"Windows").unwrap();
        let f = t.add_child(w, b"new.log", 4096, 100, NodeFlags::NONE);
        assert_eq!(t.path_lossy(f), "C:\\Windows\\new.log");
        let users = t.lookup_relative(b"Users").unwrap();
        let g = t.add_child(users, b"later.bin", 4096, 4096, NodeFlags::NONE);
        assert_eq!(t.children(users).count(), 3);
        assert_eq!(t.children(users).next(), Some(g));
        t.check_invariants().unwrap();
    }

    #[test]
    fn chunked_storage_spans_chunks() {
        let mut t = Tree::new(b"R:\\");
        let mut b = EntryBatch::new();
        for i in 0..200_000u32 {
            b.push_wtf8(
                format!("file-{i:06}.dat").as_bytes(),
                u64::from(i).next_multiple_of(4096),
                i as u64,
                NodeFlags::NONE,
            );
        }
        t.add_children(NodeId::ROOT, &b);
        assert_eq!(t.len(), 200_001);
        let on_disk: u64 = (0..200_000u64).map(|i| i.next_multiple_of(4096)).sum();
        assert_eq!(t.node(NodeId::ROOT).size(), on_disk);
        assert_eq!(t.node(NodeId(150_001)).size(), 151_552);
        assert_eq!(t.node(NodeId(150_001)).logical(), 150_000);
        assert_eq!(t.name(NodeId(150_001)), b"file-150000.dat");
        assert_eq!(t.children(NodeId::ROOT).count(), 200_000);
        t.check_invariants().unwrap();
        let m = t.memory();
        // 200k nodes need 4 chunks of 65,536 nodes; the root is wide.
        assert_eq!(m.wide, 1);
        assert!(
            m.nodes >= 200_001 * 20 && m.nodes < 5 * 65_536 * 20,
            "{m:?}"
        );
    }

    #[test]
    fn names_compress_better_after_the_scan() {
        // The first names (all alike) are what the scan-time table learns;
        // most of the tree looks different.
        let mut t = Tree::new(b"C:\\");
        let mut b = EntryBatch::new();
        for i in 0..20_000u32 {
            let name =
                format!("amd64_microsoft-windows-part{i}_31bf3856ad364e35_10.0.22621.{i}_none");
            b.push_wtf8(name.as_bytes(), 4096, 1, NodeFlags::NONE);
        }
        b.push_wtf8(b"dir", 0, 0, NodeFlags::DIR);
        t.add_children(NodeId::ROOT, &b);
        let dir = NodeId(t.len() as u32 - 1);
        for i in 0..400_000u32 {
            b.clear();
            let name = match i % 4 {
                0 => format!("照片 {i}.jpg"),
                1 => format!("IMG_{:05}.JPG", i % 40_000),
                2 => format!("{:016x}", u64::from(i).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
                _ => format!("node_modules-{}-index.js", i % 300),
            };
            b.push_wtf8(name.as_bytes(), 4096, 1, NodeFlags::NONE);
            t.add_children(dir, &b);
        }
        let names: Vec<Vec<u8>> = (0..t.len() as u32)
            .map(|i| t.name(NodeId(i)).to_vec())
            .collect();
        let before = (t.names.raw_bytes(), t.names.stored_bytes());
        assert!(before.0 > 4 * crate::names::TRAIN_AT, "{before:?}");
        t.end_scan();
        let after = (t.names.raw_bytes(), t.names.stored_bytes());
        for (i, name) in names.iter().enumerate() {
            assert_eq!(t.name(NodeId(i as u32)), &name[..], "node {i}");
        }
        // duplicates stay shared
        assert!(after.0 <= before.0, "{before:?} {after:?}");
        assert!(
            (after.1 as f64) < before.1 as f64 * 0.85,
            "{before:?} {after:?}"
        );
        eprintln!("raw, stored before {before:?} after {after:?}");
        assert_eq!(
            t.lookup_relative(b"dir\\IMG_00001.JPG")
                .map(|id| t.name(id).to_vec()),
            Some(b"IMG_00001.JPG".to_vec())
        );
        t.check_invariants().unwrap();
        // later names use the new table
        b.clear();
        b.push_wtf8(b"new name.txt", 1, 1, NodeFlags::NONE);
        let r = t.add_children(dir, &b);
        assert_eq!(t.name(NodeId(r.start)), b"new name.txt");
    }

    #[test]
    fn names_never_take_more_room_after_the_scan() {
        // Names alike from first to last (hashes, GUIDs, words in any
        // order): the first table suits them as well as any.
        let mut t = Tree::new(b"C:\\");
        let mut b = EntryBatch::new();
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut rnd = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let words = [
            "Microsoft",
            "Windows",
            "amd64",
            "System32",
            "node_modules",
            "照片",
        ];
        let exts = [".dll", ".exe", ".js", ".json", ".png", ".mui", ""];
        for _ in 0..400_000 {
            let r = rnd();
            let w = |k: u32| words[(r >> k) as usize % words.len()];
            let e = exts[(r >> 40) as usize % exts.len()];
            let name = match r % 4 {
                0 => format!("{}_{}_{:016x}{e}", w(8), w(16), rnd()),
                1 => format!(
                    "{{{:08X}-{:04X}-{:012X}}}",
                    rnd() as u32,
                    rnd() as u16,
                    rnd() >> 16
                ),
                2 => format!("{}{}{e}", w(8), (r >> 20) % 500),
                _ => format!("{:x}{e}", rnd()),
            };
            b.push_wtf8(name.as_bytes(), 1, 1, NodeFlags::NONE);
        }
        t.add_children(NodeId::ROOT, &b);
        let names: Vec<Vec<u8>> = (0..t.len() as u32)
            .map(|i| t.name(NodeId(i)).to_vec())
            .collect();
        let stored = t.names.stored_bytes();
        assert!(t.names.raw_bytes() > 4 * crate::names::TRAIN_AT);
        t.end_scan();
        assert!(t.names.stored_bytes() <= stored);
        for (i, name) in names.iter().enumerate() {
            assert_eq!(t.name(NodeId(i as u32)), &name[..], "node {i}");
        }
    }

    #[test]
    fn names_stay_when_one_would_grow_too_long() {
        // A root path near 64 KiB of bytes no table can cover: most of
        // them take two bytes encoded.
        let mut x = 1u64;
        let root: Vec<u8> = (0..65_000)
            .map(|_| {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                (x >> 56) as u8
            })
            .collect();
        let mut t = Tree::new(&root);
        let mut b = EntryBatch::new();
        for i in 0..250_000u32 {
            let name = format!("file number {i} of the tree.txt");
            b.push_wtf8(name.as_bytes(), 1, 1, NodeFlags::NONE);
        }
        t.add_children(NodeId::ROOT, &b);
        assert!(t.names.raw_bytes() > 4 * crate::names::TRAIN_AT);
        let stored = t.names.stored_bytes();
        t.end_scan();
        assert_eq!(t.name(NodeId::ROOT), &root[..]);
        assert_eq!(t.name(NodeId(1)), b"file number 0 of the tree.txt");
        assert_eq!(
            t.name(NodeId(250_000)),
            b"file number 249999 of the tree.txt"
        );
        assert_eq!(t.names.stored_bytes(), stored);
    }
}
