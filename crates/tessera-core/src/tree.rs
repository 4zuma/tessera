//! The compact in-memory file tree.
//!
//! Every file and directory is one 40-byte [`Node`] in a chunked arena, and
//! nodes refer to each other by 32-bit [`NodeId`] instead of pointers. Names
//! live in a separate WTF-8 [`NamePool`]. Nothing is allocated per node, and
//! the arena grows in fixed chunks, so growing it never copies the existing
//! nodes and never over-allocates by more than one chunk.
//!
//! Invariants (checked by [`Tree::check_invariants`] in tests):
//! * node 0 is the root directory; it has no parent;
//! * a directory's `size` / `logical` are the sums over its live children;
//! * `child_count` counts live (not deleted) children;
//! * deleted nodes stay in their parent's sibling list but are skipped by
//!   [`Tree::children`]; their descendants are implicitly deleted.

use std::ops::Range;
use std::sync::{Arc, RwLock};

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
        Ok(())
    }
}

/// One file or directory: exactly 40 bytes.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Node {
    /// Size on disk (allocated bytes). For directories, the sum over live children.
    pub size: u64,
    /// Logical file length. For directories, the sum over live children.
    pub logical: u64,
    pub parent: NodeId,
    pub first_child: NodeId,
    pub next_sibling: NodeId,
    /// Offset of the name in the [`NamePool`].
    pub name_off: u32,
    /// Length of the name in WTF-8 bytes.
    pub name_len: u16,
    pub flags: NodeFlags,
    /// Number of live children.
    pub child_count: u32,
}

const _: () = assert!(std::mem::size_of::<Node>() == 40);

impl Node {
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

const NODE_CHUNK_SHIFT: u32 = 16; // 65,536 nodes (2.5 MiB) per chunk
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

/// All names, concatenated as WTF-8 in 1 MiB chunks. A name never straddles
/// two chunks; the few bytes left at the end of a full chunk are wasted.
pub struct NamePool {
    chunks: Vec<Vec<u8>>,
}

const NAME_CHUNK_SHIFT: u32 = 20; // 1 MiB
const NAME_CHUNK: usize = 1 << NAME_CHUNK_SHIFT;

impl NamePool {
    pub const fn new() -> Self {
        NamePool { chunks: Vec::new() }
    }

    /// Stores `name` and returns its offset.
    ///
    /// # Panics
    /// If `name` is longer than `u16::MAX` bytes or the pool exceeds 4 GiB.
    pub fn push(&mut self, name: &[u8]) -> u32 {
        assert!(name.len() <= u16::MAX as usize, "name too long");
        let need_new = match self.chunks.last() {
            Some(c) => c.len() + name.len() > c.capacity(),
            None => true,
        };
        if need_new {
            self.chunks
                .push(Vec::with_capacity(NAME_CHUNK.max(name.len())));
        }
        let ci = self.chunks.len() - 1;
        let chunk = &mut self.chunks[ci];
        let off = (ci << NAME_CHUNK_SHIFT) + chunk.len();
        chunk.extend_from_slice(name);
        u32::try_from(off).expect("name pool exceeds 4 GiB")
    }

    #[inline]
    pub fn get(&self, off: u32, len: u16) -> &[u8] {
        let off = off as usize;
        let c = &self.chunks[off >> NAME_CHUNK_SHIFT];
        let start = off & (NAME_CHUNK - 1);
        &c[start..start + len as usize]
    }

    pub fn heap_bytes(&self) -> usize {
        self.chunks.iter().map(|c| c.capacity()).sum::<usize>()
            + self.chunks.capacity() * std::mem::size_of::<Vec<u8>>()
    }
}

impl Default for NamePool {
    fn default() -> Self {
        Self::new()
    }
}

/// A reusable buffer of directory entries, filled by a scanner for one
/// directory and handed to [`Tree::add_children`] in a single call.
#[derive(Default)]
pub struct EntryBatch {
    entries: Vec<BatchEntry>,
    names: Vec<u8>,
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
        self.entries.push(BatchEntry {
            size,
            logical,
            flags,
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
    pub(crate) nodes: ChunkedVec<Node>,
    pub(crate) names: NamePool,
}

/// A tree shared between the scanner threads, the watcher and the UI.
pub type SharedTree = Arc<RwLock<Tree>>;

/// Memory used by a [`Tree`], in bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct TreeMemory {
    pub nodes: usize,
    pub names: usize,
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
        let mut t = Tree {
            nodes: ChunkedVec::new(),
            names: NamePool::new(),
        };
        let name_off = t.names.push(root_name);
        t.nodes.push(Node {
            size: 0,
            logical: 0,
            parent: NodeId::NONE,
            first_child: NodeId::NONE,
            next_sibling: NodeId::NONE,
            name_off,
            name_len: root_name.len() as u16,
            flags: NodeFlags::DIR,
            child_count: 0,
        });
        t
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

    #[inline]
    pub fn node(&self, id: NodeId) -> &Node {
        self.nodes.get(id.index())
    }

    #[inline]
    pub(crate) fn node_mut(&mut self, id: NodeId) -> &mut Node {
        self.nodes.get_mut(id.index())
    }

    /// The node's own name as WTF-8 (for the root: the scanned path).
    #[inline]
    pub fn name(&self, id: NodeId) -> &[u8] {
        let n = self.node(id);
        self.names.get(n.name_off, n.name_len)
    }

    /// The node's own name for display.
    pub fn name_lossy(&self, id: NodeId) -> String {
        wtf8::to_string_lossy(self.name(id))
    }

    /// Live children (deleted ones skipped), in no particular order.
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            tree: self,
            next: self.node(id).first_child,
        }
    }

    /// The ancestors of `id`, starting with its parent and ending at the root.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut cur = self.node(id).parent;
        std::iter::from_fn(move || {
            if cur.is_none() {
                return None;
            }
            let out = cur;
            cur = self.node(cur).parent;
            Some(out)
        })
    }

    /// True if `id` or one of its ancestors is deleted.
    pub fn is_deleted(&self, id: NodeId) -> bool {
        self.node(id).is_deleted() || self.ancestors(id).any(|a| self.node(a).is_deleted())
    }

    /// Full path as WTF-8, built by walking up to the root.
    pub fn path_wtf8(&self, id: NodeId) -> Vec<u8> {
        let mut chain: Vec<NodeId> = Vec::with_capacity(16);
        let mut cur = id;
        while !cur.is_none() {
            chain.push(cur);
            cur = self.node(cur).parent;
        }
        let mut out = Vec::with_capacity(128);
        for (i, &n) in chain.iter().rev().enumerate() {
            if i > 0 && !matches!(out.last(), Some(b'\\') | Some(b'/')) {
                out.push(b'\\');
            }
            out.extend_from_slice(self.name(n));
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
            .find(|&c| wtf8::eq_ignore_case(self.name(c), name))
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
        let old_first = self.node(parent).first_child;
        let (mut dsize, mut dlogical) = (0u64, 0u64);
        for (i, e) in batch.entries.iter().enumerate() {
            let name = batch.name(e);
            let name_off = self.names.push(name);
            let id = start + i as u32;
            let next = if id + 1 < start + n {
                NodeId(id + 1)
            } else {
                old_first
            };
            self.nodes.push(Node {
                size: e.size,
                logical: e.logical,
                parent,
                first_child: NodeId::NONE,
                next_sibling: next,
                name_off,
                name_len: e.name_len,
                flags: e.flags,
                child_count: 0,
            });
            dsize = dsize.wrapping_add(e.size);
            dlogical = dlogical.wrapping_add(e.logical);
        }
        let p = self.node_mut(parent);
        p.first_child = NodeId(start);
        p.child_count += n;
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

    /// Adds `dsize` / `dlogical` (wrapping, so negative deltas work) to
    /// `from` and every ancestor of it. Stops after the first deleted node,
    /// whose bytes are already excluded from its parent.
    #[inline]
    pub(crate) fn add_to_ancestors(&mut self, from: NodeId, dsize: u64, dlogical: u64) {
        if dsize == 0 && dlogical == 0 {
            return;
        }
        let mut cur = from;
        while !cur.is_none() {
            let n = self.node_mut(cur);
            n.size = n.size.wrapping_add(dsize);
            n.logical = n.logical.wrapping_add(dlogical);
            if n.is_deleted() {
                break;
            }
            cur = n.parent;
        }
    }

    /// Changes a file's sizes and updates all its ancestors by the difference.
    pub fn set_file_size(&mut self, id: NodeId, size: u64, logical: u64) {
        let n = self.node_mut(id);
        debug_assert!(!n.is_dir(), "directory sizes are derived from children");
        let dsize = size.wrapping_sub(n.size);
        let dlogical = logical.wrapping_sub(n.logical);
        n.size = size;
        n.logical = logical;
        let (parent, deleted) = (n.parent, n.is_deleted());
        if !deleted {
            self.add_to_ancestors(parent, dsize, dlogical);
        }
    }

    /// Replaces a node's flags (sizes and links are untouched).
    pub fn set_flags(&mut self, id: NodeId, flags: NodeFlags) {
        let deleted = self.node(id).flags.contains(NodeFlags::DELETED);
        let mut f = flags;
        f.remove(NodeFlags::DELETED);
        if deleted {
            f.insert(NodeFlags::DELETED);
        }
        self.node_mut(id).flags = f;
    }

    /// Marks a node deleted and removes its bytes from all ancestors.
    /// The node keeps its own sizes (for display, e.g. "freed 3.2 GB").
    /// Returns false if it was already deleted. The root cannot be deleted.
    pub fn mark_deleted(&mut self, id: NodeId) -> bool {
        if id == NodeId::ROOT || self.is_deleted(id) {
            return false;
        }
        let n = self.node_mut(id);
        n.flags.insert(NodeFlags::DELETED);
        let (size, logical, parent) = (n.size, n.logical, n.parent);
        self.node_mut(parent).child_count -= 1;
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
            nodes: self.nodes.heap_bytes(),
            names: self.names.heap_bytes(),
        }
    }

    /// Verifies the size and count invariants of the whole tree.
    /// O(n); meant for tests and debug assertions.
    pub fn check_invariants(&self) -> Result<(), String> {
        let n = self.nodes.len();
        let mut size = vec![0u64; n];
        let mut logical = vec![0u64; n];
        let mut count = vec![0u32; n];
        for (i, node) in self.nodes.iter().enumerate() {
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
                size[p] = size[p].wrapping_add(node.size);
                logical[p] = logical[p].wrapping_add(node.logical);
                count[p] += 1;
            }
        }
        for (i, node) in self.nodes.iter().enumerate() {
            let id = NodeId(i as u32);
            let listed = self.children(id).count() as u32;
            if listed != node.child_count || count[i] != node.child_count {
                return Err(format!(
                    "node {i} ({}) child_count {} but {} listed / {} by parent links",
                    self.path_lossy(id),
                    node.child_count,
                    listed,
                    count[i]
                ));
            }
            if node.is_dir() && (node.size != size[i] || node.logical != logical[i]) {
                return Err(format!(
                    "dir {i} ({}) has size {}/{} but children sum to {}/{}",
                    self.path_lossy(id),
                    node.size,
                    node.logical,
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
            let n = self.tree.node(id);
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
    fn node_is_40_bytes() {
        assert_eq!(std::mem::size_of::<Node>(), 40);
    }

    #[test]
    fn sizes_propagate_to_ancestors() {
        let t = sample();
        assert_eq!(t.node(NodeId::ROOT).size, 409_600 + 8192 + 4096);
        assert_eq!(t.node(NodeId::ROOT).logical, 409_000 + 8000 + 12);
        assert_eq!(t.node(NodeId(1)).size, 8192 + 4096);
        assert_eq!(t.node(NodeId::ROOT).child_count, 3);
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
        assert_eq!(t.node(NodeId(1)).size, 8192 + 8192);
        t.check_invariants().unwrap();

        assert!(t.mark_deleted(NodeId(1)));
        assert!(!t.mark_deleted(NodeId(1)));
        assert!(
            !t.mark_deleted(notes),
            "descendant of a deleted node is already deleted"
        );
        assert_eq!(t.node(NodeId::ROOT).size, 409_600);
        assert_eq!(t.node(NodeId::ROOT).child_count, 2);
        assert_eq!(t.children(NodeId::ROOT).count(), 2);
        assert!(t.is_deleted(notes));
        // Resizing a file under a deleted directory must not touch live sums.
        t.set_file_size(notes, 1, 1);
        t.check_invariants().unwrap();
        assert!(!t.mark_deleted(NodeId::ROOT));
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
                4096,
                i as u64,
                NodeFlags::NONE,
            );
        }
        t.add_children(NodeId::ROOT, &b);
        assert_eq!(t.len(), 200_001);
        assert_eq!(t.node(NodeId::ROOT).size, 4096 * 200_000);
        assert_eq!(t.name(NodeId(150_001)), b"file-150000.dat");
        assert_eq!(t.children(NodeId::ROOT).count(), 200_000);
        t.check_invariants().unwrap();
        let m = t.memory();
        // 200k nodes need 4 chunks of 65,536 nodes.
        assert!(
            m.nodes >= 200_001 * 40 && m.nodes < 5 * 65_536 * 40,
            "{m:?}"
        );
    }

    #[test]
    fn name_pool_does_not_straddle_chunks() {
        let mut p = NamePool::new();
        let long = vec![b'x'; 60_000];
        let mut offs = Vec::new();
        for _ in 0..40 {
            offs.push(p.push(&long));
        }
        for &o in &offs {
            assert_eq!(p.get(o, 60_000).len(), 60_000);
        }
        assert!(p.heap_bytes() >= 40 * 60_000);
    }
}
