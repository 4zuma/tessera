//! Applying a coalesced [`Batch`] to the tree.
//!
//! This part is portable: the file system is reached only through the
//! [`Probe`] trait (attributes of one entry) and a directory scanner
//! callback (in production `walk::scan_into`), so it is unit tested on any
//! platform with a fake file system.
//!
//! Application is level-triggered: an entry's final state (present with
//! these sizes, or gone) is reconciled with the tree, so the result does not
//! depend on how many events led there or in which order they were merged.
//!
//! Order of work for one batch:
//! 1. entries whose notification carried no attributes are probed, without
//!    holding any lock (USN: final paths are worked out under a read lock);
//! 2. under a single `tree.write()`: requested rescans, renames (in order),
//!    path entries (shallow first, so parents exist before children), USN
//!    entries (deletions first, then parents before children);
//! 3. without the lock, new directories are filled by the scanner. Entries
//!    below them were held back; they are dropped when the scan succeeded
//!    (it saw the newer state) and applied one by one when it could not run;
//! 4. hard links, without any lock: files that may have gained or lost a
//!    name (new files, changes through a name that does not count the bytes,
//!    removed names, files of scanned directories) are probed for their
//!    link count, and the names of those with several are read;
//! 5. under `tree.write()`, each such file's bytes are counted on exactly
//!    one of its names in the tree (see [`Applier::reconcile_links`]).
//!
//! Hard links are only followed where the probe can see them (local NTFS),
//! and at most [`LINK_PROBES`] probes are spent on them per batch.

// Without Windows only the unit tests drive this code.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::{HashMap, HashSet};

use super::BatchReport;
use super::coalesce::{
    Batch, Info, PathChange, Rename, UsnChange, component_spans, fold_key, split_last,
};
use super::usn::reason;
use crate::scan::FrnIndex;
use crate::tree::{NodeFlags, NodeId, SharedTree, Tree};
use crate::{Error, Result, wtf8};

/// Win32 file attribute bits used here (values from the Windows SDK).
pub(crate) mod attr {
    pub const HIDDEN: u32 = 0x2;
    pub const SYSTEM: u32 = 0x4;
    pub const DIRECTORY: u32 = 0x10;
    pub const SPARSE_FILE: u32 = 0x200;
    pub const REPARSE_POINT: u32 = 0x400;
    pub const COMPRESSED: u32 = 0x800;
    pub const OFFLINE: u32 = 0x1000;
    pub const RECALL_ON_OPEN: u32 = 0x4_0000;
    pub const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
}

/// Reparse tags with this bit (junctions, symbolic links) point to another
/// name; such entries are never followed.
const TAG_NAME_SURROGATE: u32 = 0x2000_0000;

/// Node flags that mirror file attributes and may change in place.
const ATTR_FLAGS: NodeFlags = NodeFlags(
    NodeFlags::HIDDEN.0 | NodeFlags::SYSTEM.0 | NodeFlags::COMPRESSED.0 | NodeFlags::CLOUD.0,
);

/// Changes below directories with these flags are ignored.
const BARRIER: NodeFlags = NodeFlags(NodeFlags::INACCESSIBLE.0 | NodeFlags::REPARSE.0);

/// Bits of a file reference number that hold the MFT record number.
const RECORD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// Nodes that never take part in hard-link accounting.
const NOT_LINKED: NodeFlags =
    NodeFlags(NodeFlags::DIR.0 | NodeFlags::METAFILE.0 | NodeFlags::CLOUD.0 | NodeFlags::REPARSE.0);

/// Hard-link probes per batch, at most (each costs about as much as one
/// attribute query). Beyond that a hard-linked file's bytes may be counted
/// on two names until a later change to it or a rescan.
pub(crate) const LINK_PROBES: usize = 1 << 16;

/// Attributes and sizes of one entry, as read from the file system.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Stat {
    pub attributes: u32,
    /// Meaningful only with `FILE_ATTRIBUTE_REPARSE_POINT`.
    pub reparse_tag: u32,
    /// Allocated bytes (size on disk).
    pub size: u64,
    /// Logical length.
    pub logical: u64,
    /// Last write time ([`crate::Node::mtime`] units, 0 = unknown).
    pub mtime: u32,
}

impl Stat {
    pub fn is_dir(&self) -> bool {
        self.attributes & attr::DIRECTORY != 0
    }

    pub fn flags(&self) -> NodeFlags {
        flags_from_attributes(self.attributes, self.reparse_tag)
    }
}

/// Node flags for a file with these attributes.
pub(crate) fn flags_from_attributes(attributes: u32, reparse_tag: u32) -> NodeFlags {
    let mut f = NodeFlags::NONE;
    let has = |bits: u32| attributes & bits != 0;
    if has(attr::DIRECTORY) {
        f |= NodeFlags::DIR;
    }
    if has(attr::HIDDEN) {
        f |= NodeFlags::HIDDEN;
    }
    if has(attr::SYSTEM) {
        f |= NodeFlags::SYSTEM;
    }
    if has(attr::COMPRESSED | attr::SPARSE_FILE) {
        f |= NodeFlags::COMPRESSED;
    }
    if has(attr::RECALL_ON_DATA_ACCESS | attr::RECALL_ON_OPEN | attr::OFFLINE) {
        f |= NodeFlags::CLOUD;
    }
    if has(attr::REPARSE_POINT) && reparse_tag & TAG_NAME_SURROGATE != 0 {
        f |= NodeFlags::REPARSE;
    }
    f
}

/// A file's sizes and names, read through its id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Links {
    /// Allocated bytes (size on disk).
    pub size: u64,
    /// Logical length.
    pub logical: u64,
    /// The paths of the file's names (hard links) that are inside the
    /// watched root, relative to it. Names elsewhere are left out.
    pub names: Vec<Vec<u8>>,
}

/// Read-only access to the file system for the applier.
pub(crate) trait Probe {
    /// The current state of the entry at `rel` (relative to the watched
    /// root). Reads attributes only: never opens file contents, never
    /// hydrates cloud files. `Unknown` when it cannot tell.
    fn stat(&mut self, rel: &[u8]) -> Info;

    /// The long name of the last component of `rel`, which may be a short
    /// 8.3 name. Only asked when a name containing `~` is not in the tree.
    fn long_name(&mut self, _rel: &[u8]) -> Option<Vec<u8>> {
        None
    }

    /// The path, relative to the watched root, of the file whose reference
    /// number is `frn`, or `None` if it is elsewhere or gone (USN only).
    fn locate(&mut self, _frn: u64) -> Option<Vec<u8>> {
        None
    }

    /// True if files can have several names (hard links) that
    /// [`Probe::link_count`] and [`Probe::links`] report (local NTFS).
    fn tracks_links(&self) -> bool {
        false
    }

    /// The id and the number of names of the file at `rel`.
    fn link_count(&mut self, _rel: &[u8]) -> Option<(u64, u32)> {
        None
    }

    /// The sizes and names of the file with id `id` (an NTFS file reference
    /// number), or `None` if it is gone or cannot be read.
    fn links(&mut self, _id: u64) -> Option<Links> {
        None
    }
}

/// Fills a newly added, empty directory: `(tree, dir, path relative to
/// the root)`. Takes the tree lock itself, as `walk::scan_into` does.
pub(crate) type ScanDir<'a> = dyn FnMut(&SharedTree, NodeId, &[u8]) -> Result<()> + 'a;

impl FrnIndex {
    /// Maps the file with reference number `frn` to `node`. Only the MFT
    /// record number (the low 48 bits) is used, so a reused record simply
    /// replaces the old mapping.
    ///
    /// The MFT scanner builds the index; this lets other code (the USN
    /// watcher, tests) extend it.
    pub fn insert(&mut self, frn: u64, node: NodeId) {
        let rec = frn & RECORD_MASK;
        if rec >= u32::MAX as u64 {
            return;
        }
        let rec = rec as usize;
        if rec >= self.record_to_node.len() {
            self.record_to_node.resize(rec + 1, u32::MAX);
        }
        self.record_to_node[rec] = node.0;
    }

    /// Forgets the mapping of `frn`.
    pub fn remove(&mut self, frn: u64) {
        if let Some(slot) = self.record_to_node.get_mut((frn & RECORD_MASK) as usize) {
            *slot = u32::MAX;
        }
    }
}

/// What the USN applier keeps between batches.
pub(crate) struct UsnState {
    pub index: FrnIndex,
    /// Directories known not to be in the tree (elsewhere on the volume or
    /// below an ignored directory), so that they are located only once.
    outside: HashSet<u64>,
}

impl UsnState {
    pub fn new(index: FrnIndex) -> Self {
        UsnState {
            index,
            outside: HashSet::new(),
        }
    }

    /// The live node of `frn`, if it is mapped.
    fn live(&self, tree: &Tree, frn: u64) -> Option<NodeId> {
        let id = self.index.get(frn & RECORD_MASK)?;
        tree.is_live(id).then_some(id)
    }

    fn insert(&mut self, frn: u64, node: NodeId) {
        self.index.insert(frn, node);
        self.outside.remove(&frn);
    }
}

impl Tree {
    /// True if `id` is a valid, not deleted node.
    pub(crate) fn is_live(&self, id: NodeId) -> bool {
        id.index() < self.len() && !self.is_deleted(id)
    }

    /// True if `id` or one of its ancestors is flagged inaccessible or
    /// reparse point, i.e. changes below `id` are not tracked.
    fn behind_barrier(&self, id: NodeId) -> bool {
        self.node(id).flags.intersects(BARRIER)
            || self
                .ancestors(id)
                .any(|a| self.node(a).flags.intersects(BARRIER))
    }

    /// Path of `id` relative to the root, `\`-separated WTF-8 (empty for
    /// the root).
    pub(crate) fn rel_path_wtf8(&self, id: NodeId) -> Vec<u8> {
        let mut chain = Vec::new();
        let mut cur = id;
        while cur != NodeId::ROOT && !cur.is_none() {
            chain.push(cur);
            cur = self.node(cur).parent;
        }
        let mut out = Vec::new();
        for &n in chain.iter().rev() {
            if !out.is_empty() {
                out.push(b'\\');
            }
            out.extend_from_slice(self.name(n));
        }
        out
    }

    /// Renames a live node and/or moves it below another live directory,
    /// keeping all size and count invariants. Returns false (and changes
    /// nothing) for the root, a deleted node, or a move into its own subtree.
    pub(crate) fn move_node(&mut self, id: NodeId, new_parent: NodeId, new_name: &[u8]) -> bool {
        if id == NodeId::ROOT
            || !self.is_live(id)
            || !self.is_live(new_parent)
            || !self.node(new_parent).is_dir()
            || new_parent == id
            || self.ancestors(new_parent).any(|a| a == id)
        {
            return false;
        }
        if self.name(id) != new_name {
            let off = self.names.push(new_name);
            let n = self.node_mut(id);
            n.name_off = off;
            n.name_len = new_name.len() as u16;
        }
        let n = *self.node(id);
        if n.parent == new_parent {
            return true;
        }
        // Unlink from the old parent's sibling list.
        let old = n.parent;
        let mut prev = NodeId::NONE;
        let mut cur = self.node(old).first_child;
        while cur != id {
            prev = cur;
            cur = self.node(cur).next_sibling;
        }
        if prev.is_none() {
            self.node_mut(old).first_child = n.next_sibling;
        } else {
            self.node_mut(prev).next_sibling = n.next_sibling;
        }
        self.add_to_ancestors(old, n.size.wrapping_neg(), n.logical.wrapping_neg());
        // Link at the head of the new parent's list.
        let first = self.node(new_parent).first_child;
        let m = self.node_mut(id);
        m.parent = new_parent;
        m.next_sibling = first;
        let p = self.node_mut(new_parent);
        p.first_child = id;
        self.add_to_ancestors(new_parent, n.size, n.logical);
        true
    }
}

/// Counters and side results of one batch, shared by all apply rounds.
#[derive(Default)]
struct Tally {
    created: usize,
    deleted: usize,
    modified: usize,
    moved: usize,
    dirs: HashSet<NodeId>,
    rescan: Vec<NodeId>,
    /// New directories to fill, with their relative paths.
    scans: Vec<(NodeId, Vec<u8>)>,
    scan_keys: HashSet<Vec<u8>>,
    scan_nodes: HashSet<NodeId>,
    /// Path entries below a directory waiting for its scan.
    deferred: Vec<PathChange>,
    /// Files that may have gained or lost a name: their link count is read
    /// after the batch.
    link_nodes: Vec<NodeId>,
    /// Ids of files whose names are read and reconciled after the batch.
    link_ids: Vec<u64>,
}

enum Resolved {
    Node(NodeId),
    /// Below an ignored directory.
    Ignored,
    /// The tree has no entry for the component ending at byte `end`, whose
    /// parent is `parent` (or has a file where a directory is expected).
    Missing {
        parent: NodeId,
        end: usize,
    },
}

struct Applier<'a> {
    tree: &'a mut Tree,
    probe: &'a mut dyn Probe,
    t: &'a mut Tally,
}

impl Applier<'_> {
    fn find(&mut self, parent: NodeId, rel: &[u8], name: &[u8]) -> Option<NodeId> {
        if let Some(c) = self.tree.find_child(parent, name) {
            return Some(c);
        }
        if name.contains(&b'~') {
            let long = self.probe.long_name(rel)?;
            if !wtf8::eq_ignore_case(&long, name) {
                return self.tree.find_child(parent, &long);
            }
        }
        None
    }

    /// The live node at `rel`.
    fn resolve(&mut self, rel: &[u8]) -> Resolved {
        let mut cur = NodeId::ROOT;
        let mut cur_end = 0;
        for (start, end) in component_spans(rel) {
            let n = self.tree.node(cur);
            if !n.is_dir() {
                return Resolved::Missing {
                    parent: n.parent,
                    end: cur_end,
                };
            }
            if n.flags.intersects(BARRIER) {
                return Resolved::Ignored;
            }
            match self.find(cur, &rel[..end], &rel[start..end]) {
                Some(c) => {
                    cur = c;
                    cur_end = end;
                }
                None => return Resolved::Missing { parent: cur, end },
            }
        }
        Resolved::Node(cur)
    }

    /// The live directory at `rel`, whose children are tracked.
    fn resolve_dir(&mut self, rel: &[u8]) -> Resolved {
        match self.resolve(rel) {
            Resolved::Node(id) => {
                let n = self.tree.node(id);
                if !n.is_dir() {
                    let end = component_spans(rel).last().map_or(0, |s| s.1);
                    Resolved::Missing {
                        parent: n.parent,
                        end,
                    }
                } else if n.flags.intersects(BARRIER) {
                    Resolved::Ignored
                } else {
                    Resolved::Node(id)
                }
            }
            other => other,
        }
    }

    /// True if `rel` is strictly below a directory waiting for its scan.
    fn under_scan(&self, rel: &[u8]) -> bool {
        if self.t.scan_keys.is_empty() {
            return false;
        }
        let key = fold_key(rel);
        key.iter()
            .enumerate()
            .any(|(i, &b)| b == b'\\' && self.t.scan_keys.contains(&key[..i]))
    }

    /// True if `id` is (below) a directory waiting for its scan.
    fn scan_covers(&self, id: NodeId) -> bool {
        !self.t.scan_nodes.is_empty()
            && (self.t.scan_nodes.contains(&id)
                || self
                    .tree
                    .ancestors(id)
                    .any(|a| self.t.scan_nodes.contains(&a)))
    }

    fn schedule_scan(&mut self, id: NodeId, rel: &[u8]) {
        self.t.scan_keys.insert(fold_key(rel));
        self.t.scan_nodes.insert(id);
        self.t.scans.push((id, rel.to_vec()));
    }

    /// Has the link count of file `id` read after the batch.
    fn check_links(&mut self, id: NodeId) {
        if self.probe.tracks_links() && self.t.link_nodes.len() < LINK_PROBES {
            self.t.link_nodes.push(id);
        }
    }

    /// Has the names of the file with id `frn` reconciled after the batch.
    fn check_file(&mut self, frn: u64) {
        if self.probe.tracks_links() {
            self.t.link_ids.push(frn);
        }
    }

    fn delete(&mut self, id: NodeId) {
        let n = *self.tree.node(id);
        if n.flags.contains(NodeFlags::METAFILE) {
            return;
        }
        if self.tree.mark_deleted(id) {
            self.t.deleted += 1;
            self.t.dirs.insert(n.parent);
        }
    }

    /// Brings an existing node up to date. Returns false if the entry on
    /// disk is of another kind (file / directory / reparse point): the node
    /// is then deleted and the caller adds a new one.
    fn update(&mut self, id: NodeId, stat: &Stat) -> bool {
        let n = *self.tree.node(id);
        if n.flags.contains(NodeFlags::METAFILE) {
            return true;
        }
        let flags = stat.flags();
        if n.is_dir() != stat.is_dir()
            || n.flags.contains(NodeFlags::REPARSE) != flags.contains(NodeFlags::REPARSE)
        {
            self.delete(id);
            return false;
        }
        let merged = NodeFlags((n.flags.0 & !ATTR_FLAGS.0) | (flags.0 & ATTR_FLAGS.0));
        let mut changed = false;
        if merged != n.flags {
            self.tree.set_flags(id, merged);
            changed = true;
        }
        if n.flags.contains(NodeFlags::HARDLINK) {
            // Another name counts the bytes; it is brought up to date (or
            // this one takes over) after the batch.
            self.check_links(id);
        } else if !n.is_dir() && (n.size != stat.size || n.logical != stat.logical) {
            self.tree.set_file_size(id, stat.size, stat.logical);
            changed = true;
        }
        // A new time alone doesn't move any rectangle: not counted as a change.
        if stat.mtime != 0 && n.mtime != stat.mtime {
            self.tree.set_mtime(id, stat.mtime);
        }
        if changed {
            self.t.modified += 1;
            self.t.dirs.insert(n.parent);
        }
        true
    }

    /// Adds a new entry; a new directory is filled later if `scan`.
    fn add(&mut self, parent: NodeId, rel: &[u8], name: &[u8], stat: &Stat, scan: bool) -> NodeId {
        let flags = stat.flags();
        let (size, logical) = if stat.is_dir() {
            (0, 0)
        } else {
            (stat.size, stat.logical)
        };
        let long = if name.contains(&b'~') {
            self.probe.long_name(rel)
        } else {
            None
        };
        let id = self.tree.add_child(
            parent,
            long.as_deref().unwrap_or(name),
            size,
            logical,
            flags,
            stat.mtime,
        );
        self.t.created += 1;
        self.t.dirs.insert(parent);
        if scan && stat.is_dir() && !flags.contains(NodeFlags::REPARSE) {
            self.schedule_scan(id, rel);
        }
        id
    }

    fn present(&mut self, parent: NodeId, rel: &[u8], name: &[u8], stat: &Stat) {
        if let Some(id) = self.find(parent, rel, name) {
            if self.update(id, stat) {
                return;
            }
        }
        let id = self.add(parent, rel, name, stat, true);
        if !stat.is_dir() {
            // Possibly a new name of a file the tree already counts.
            self.check_links(id);
        }
    }

    fn gone(&mut self, parent: NodeId, name: &[u8]) {
        if let Some(id) = self.tree.find_child(parent, name) {
            self.delete(id);
        }
    }

    /// The tree lacks the directory `rel` (child of `parent`) although an
    /// entry below it exists: bring it in, with a scan of its contents.
    fn heal(&mut self, parent: NodeId, rel: &[u8]) {
        if self.under_scan(rel) || !self.tree.node(parent).is_dir() {
            return;
        }
        let (_, name) = split_last(rel);
        if let Info::Present(stat) = self.probe.stat(rel) {
            self.present(parent, rel, name, &stat);
        }
    }

    fn rescan_request(&mut self, id: NodeId) {
        if !self.tree.is_live(id) {
            return;
        }
        let n = *self.tree.node(id);
        if !n.is_dir()
            || n.flags
                .intersects(NodeFlags(NodeFlags::REPARSE.0 | NodeFlags::METAFILE.0))
        {
            return;
        }
        if n.flags.contains(NodeFlags::INACCESSIBLE) {
            let mut f = n.flags;
            f.remove(NodeFlags::INACCESSIBLE);
            self.tree.set_flags(id, f);
        }
        self.tree.clear_children(id);
        self.t.dirs.insert(id);
        let rel = self.tree.rel_path_wtf8(id);
        self.schedule_scan(id, &rel);
    }

    fn overflow(&mut self, rel: &[u8]) {
        let id = match self.resolve(rel) {
            Resolved::Node(id) if self.tree.node(id).is_dir() => id,
            _ => NodeId::ROOT,
        };
        if !self.t.rescan.contains(&id) {
            self.t.rescan.push(id);
        }
    }

    fn rename(&mut self, r: &Rename) {
        let src = match self.resolve(&r.from) {
            Resolved::Node(id) if id != NodeId::ROOT => id,
            // Unknown source (e.g. created in this batch, or below an
            // ignored directory): the entry at `r.to` adds it.
            _ => return,
        };
        if self.tree.node(src).flags.contains(NodeFlags::METAFILE) {
            return;
        }
        if self.under_scan(&r.to) {
            self.delete(src);
            return;
        }
        let old_parent = self.tree.node(src).parent;
        let (parent_rel, name) = split_last(&r.to);
        match self.resolve_dir(parent_rel) {
            Resolved::Node(p) => {
                if let Some(existing) = self.tree.find_child(p, name) {
                    if existing == src {
                        // Case-only rename.
                        if self.tree.name(src) != name && self.tree.move_node(src, p, name) {
                            self.t.moved += 1;
                            self.t.dirs.insert(p);
                        }
                        return;
                    }
                    self.delete(existing); // replaced by the renamed entry
                }
                if self.tree.move_node(src, p, name) {
                    self.t.moved += 1;
                    self.t.dirs.insert(old_parent);
                    self.t.dirs.insert(p);
                } else {
                    self.delete(src);
                }
            }
            // Moved out of view (or into a directory the tree lacks so far,
            // which brings it back with a scan).
            _ => self.delete(src),
        }
    }

    fn path_change(&mut self, pc: &PathChange) {
        if self.under_scan(&pc.path) {
            self.t.deferred.push(pc.clone());
            return;
        }
        let (parent_rel, name) = split_last(&pc.path);
        if name.is_empty() {
            return;
        }
        match self.resolve_dir(parent_rel) {
            Resolved::Node(parent) => match &pc.info {
                Info::Present(stat) => self.present(parent, &pc.path, name, stat),
                Info::Gone => self.gone(parent, name),
                Info::Unknown => {}
            },
            Resolved::Ignored => {}
            Resolved::Missing { parent, end } => {
                if let Info::Present(_) = pc.info {
                    self.heal(parent, &pc.path[..end]);
                    if self.under_scan(&pc.path) {
                        self.t.deferred.push(pc.clone());
                    }
                }
            }
        }
    }

    /// The live, tracked directory with reference number `frn`.
    fn usn_dir(&self, st: &UsnState, frn: u64) -> Option<NodeId> {
        let id = st.live(self.tree, frn)?;
        (self.tree.node(id).is_dir() && !self.tree.behind_barrier(id) && !self.scan_covers(id))
            .then_some(id)
    }

    fn usn_batch(
        &mut self,
        st: &mut UsnState,
        changes: &[UsnChange],
        probed: &[Info],
        links_probed: &[Vec<Info>],
    ) {
        let deleted = |c: &UsnChange| c.last_reason & reason::FILE_DELETE != 0;
        // Deletions first: a record number freed here may be reused by a
        // file created later in the batch.
        for c in changes.iter().filter(|c| deleted(c)) {
            let indexed = st.live(self.tree, c.frn).filter(|&id| id != NodeId::ROOT);
            if let Some(id) = indexed {
                self.delete(id);
            }
            st.index.remove(c.frn);
            // The record names the file's last name. Without an index entry
            // that name is the way to the node; with hard links the index
            // may point at another of the file's names.
            if indexed.is_none() || c.reasons & reason::HARD_LINK_CHANGE != 0 {
                if let Some(p) = self.usn_dir(st, c.parent_frn) {
                    if let Some(id) = self.tree.find_child(p, &c.name) {
                        if indexed.is_none() || !self.tree.node(id).is_dir() {
                            self.delete(id);
                        }
                    }
                }
            }
        }
        // Then the rest in order of first appearance, each after its
        // parent if that is part of the batch too.
        let pos: HashMap<u64, usize> = changes
            .iter()
            .enumerate()
            .filter(|(_, c)| !deleted(c))
            .map(|(i, c)| (c.frn, i))
            .collect();
        let mut seen = vec![false; changes.len()];
        for i in 0..changes.len() {
            if seen[i] || deleted(&changes[i]) {
                continue;
            }
            seen[i] = true;
            let mut chain = vec![i];
            let mut j = i;
            while let Some(&k) = pos.get(&changes[j].parent_frn) {
                if seen[k] {
                    break;
                }
                seen[k] = true;
                chain.push(k);
                j = k;
            }
            for &k in chain.iter().rev() {
                self.usn_one(st, &changes[k], &probed[k]);
            }
        }
        // Hard links added or removed, once all directories are in place.
        for (c, infos) in changes.iter().zip(links_probed) {
            for ((parent, name), info) in c.links.iter().zip(infos) {
                self.usn_link(st, *parent, name, info);
            }
            if !c.links.is_empty() && !deleted(c) {
                self.check_file(c.frn);
            }
        }
    }

    /// A name added to or removed from a file (a hard link): the name is
    /// brought in line with the disk; the file's bytes are settled later.
    fn usn_link(&mut self, st: &UsnState, parent: u64, name: &[u8], info: &Info) {
        let Some(p) = self.usn_dir(st, parent) else {
            return;
        };
        let existing = self.tree.find_child(p, name);
        match info {
            Info::Present(stat) if !stat.is_dir() => {
                if let Some(id) = existing {
                    if self.update(id, stat) {
                        return;
                    }
                }
                let rel = self.child_rel(p, name);
                self.add(p, &rel, name, stat, false);
            }
            Info::Gone => {
                if let Some(id) = existing.filter(|&id| !self.tree.node(id).is_dir()) {
                    self.delete(id);
                }
            }
            _ => {}
        }
    }

    fn usn_one(&mut self, st: &mut UsnState, c: &UsnChange, info: &Info) {
        let indexed = st.live(self.tree, c.frn);
        if indexed == Some(NodeId::ROOT) {
            return;
        }
        let at_old = c.old_location.as_ref().and_then(|(old_parent, old_name)| {
            let p = self.usn_dir(st, *old_parent)?;
            self.tree.find_child(p, old_name)
        });
        let mut node = match (indexed, at_old) {
            // A file with several names: the index points at one of them,
            // and the one renamed is the one at the old location.
            (Some(i), Some(o)) if i != o && !self.tree.node(o).is_dir() => Some(o),
            (i, o) => i.or(o),
        };
        let parent = self.usn_dir(st, c.parent_frn);
        if node.is_none() {
            // E.g. an entry that a directory scan added (scans do not
            // record reference numbers).
            node = parent.and_then(|p| self.tree.find_child(p, &c.name));
        }
        let Some(id) = node else {
            // A new entry (or one moved in from elsewhere).
            if let (Some(p), Info::Present(stat)) = (parent, info) {
                let rel = self.child_rel(p, &c.name);
                let moved_in = c.reasons & reason::FILE_CREATE == 0;
                let id = self.add(p, &rel, &c.name, stat, moved_in);
                st.insert(c.frn, id);
                if moved_in && stat.is_dir() {
                    // Directories known to be outside may be inside now.
                    st.outside.clear();
                }
                if moved_in && !stat.is_dir() {
                    // It may have other names in the tree.
                    self.check_links(id);
                }
            }
            return;
        };
        if self.tree.node(id).flags.contains(NodeFlags::METAFILE) {
            return;
        }
        if self.tree.node(id).flags.contains(NodeFlags::HARDLINK) {
            // The index keeps the name that counts the bytes.
            self.check_file(c.frn);
        } else {
            st.insert(c.frn, id);
        }
        if c.reasons & reason::RENAME_NEW_NAME != 0 {
            let Some(p) = parent else {
                // Moved out of the tree or below an ignored directory.
                let file = !self.tree.node(id).is_dir();
                self.delete(id);
                if st.index.get(c.frn & RECORD_MASK) == Some(id) {
                    st.index.remove(c.frn);
                }
                if file {
                    // Its other names, if any, may now count its bytes.
                    self.check_file(c.frn);
                }
                return;
            };
            let n = *self.tree.node(id);
            if n.parent != p || self.tree.name(id) != &c.name[..] {
                match self.tree.find_child(p, &c.name) {
                    Some(other) if other != id => self.delete(other),
                    _ => {}
                }
                if self.tree.move_node(id, p, &c.name) {
                    self.t.moved += 1;
                    self.t.dirs.insert(n.parent);
                    self.t.dirs.insert(p);
                    if n.is_dir() {
                        // Directories known to be outside may be inside now.
                        st.outside.clear();
                    }
                }
            }
        }
        if self.tree.behind_barrier(self.tree.node(id).parent) {
            return; // below an ignored directory
        }
        match info {
            Info::Present(stat) => {
                if !self.update(id, stat) {
                    let p = self.tree.node(id).parent;
                    let rel = self.child_rel(p, &c.name);
                    let new = self.add(p, &rel, &c.name, stat, true);
                    st.insert(c.frn, new);
                }
            }
            // Existing directories are not probed: attributes come with
            // the record (their reparse state is checked when it changes).
            Info::Unknown if self.tree.node(id).is_dir() => {
                let n = *self.tree.node(id);
                let flags = flags_from_attributes(c.attributes, 0);
                let merged = NodeFlags((n.flags.0 & !ATTR_FLAGS.0) | (flags.0 & ATTR_FLAGS.0));
                if merged != n.flags {
                    self.tree.set_flags(id, merged);
                    self.t.modified += 1;
                    self.t.dirs.insert(n.parent);
                }
            }
            // Gone without a delete record (yet): it will come.
            _ => {}
        }
    }

    /// Makes exactly one of a file's names in the tree count its bytes: the
    /// lowest node id among the names counting them now (so a settled file
    /// does not change), else the lowest; the others become `HARDLINK`
    /// nodes with zero sizes, as the scanners make them. Returns that node.
    fn reconcile_links(&mut self, links: &Links) -> Option<NodeId> {
        let mut nodes: Vec<NodeId> = links
            .names
            .iter()
            .filter_map(|rel| {
                let id = self.tree.lookup_relative(rel)?;
                let n = self.tree.node(id);
                (id != NodeId::ROOT
                    && !n.flags.intersects(NOT_LINKED)
                    && !self.tree.behind_barrier(n.parent))
                .then_some(id)
            })
            .collect();
        nodes.sort_unstable();
        nodes.dedup();
        let counts = |id: NodeId| !self.tree.node(id).flags.contains(NodeFlags::HARDLINK);
        let keep = nodes
            .iter()
            .copied()
            .find(|&id| counts(id))
            .or(nodes.first().copied())?;
        for id in nodes {
            let n = *self.tree.node(id);
            let mut flags = n.flags;
            let (size, logical) = if id == keep {
                flags.remove(NodeFlags::HARDLINK);
                (links.size, links.logical)
            } else {
                flags.insert(NodeFlags::HARDLINK);
                (0, 0)
            };
            if flags == n.flags && size == n.size && logical == n.logical {
                continue;
            }
            self.tree.set_flags(id, flags);
            self.tree.set_file_size(id, size, logical);
            self.t.modified += 1;
            self.t.dirs.insert(n.parent);
        }
        Some(keep)
    }

    fn child_rel(&self, parent: NodeId, name: &[u8]) -> Vec<u8> {
        let mut rel = self.tree.rel_path_wtf8(parent);
        if !rel.is_empty() {
            rel.push(b'\\');
        }
        rel.extend_from_slice(name);
        rel
    }
}

/// Resolves a relative path read from the file system to a live, tracked
/// directory (no short names involved).
fn tracked_dir(tree: &Tree, rel: &[u8]) -> Option<NodeId> {
    let id = tree.lookup_relative(rel)?;
    (tree.node(id).is_dir() && !tree.behind_barrier(id)).then_some(id)
}

/// Where each USN entry is after the batch, and each hard link it gained or
/// lost (by entry, in order).
type UsnPaths = (Vec<Option<Vec<u8>>>, Vec<Vec<Option<Vec<u8>>>>);

/// Works out where each USN entry (and each of its hard-link names) is after
/// the batch, so that it can be probed before the tree is locked for
/// writing. Directories the index does not know are located (and indexed)
/// on the way.
fn usn_paths(
    tree: &Tree,
    st: &mut UsnState,
    probe: &mut dyn Probe,
    changes: &[UsnChange],
) -> UsnPaths {
    let deleted = |c: &UsnChange| c.last_reason & reason::FILE_DELETE != 0;
    let pos: HashMap<u64, usize> = changes
        .iter()
        .enumerate()
        .filter(|(_, c)| !deleted(c))
        .map(|(i, c)| (c.frn, i))
        .collect();
    // Index parents that are neither indexed nor part of the batch.
    let mut wanted: Vec<u64> = Vec::new();
    for c in changes {
        wanted.push(c.parent_frn);
        if let Some((p, _)) = &c.old_location {
            wanted.push(*p);
        }
        wanted.extend(c.links.iter().map(|(p, _)| *p));
    }
    for frn in wanted {
        if pos.contains_key(&frn) || st.live(tree, frn).is_some() || st.outside.contains(&frn) {
            continue;
        }
        match probe.locate(frn).and_then(|rel| tracked_dir(tree, &rel)) {
            Some(id) => st.insert(frn, id),
            None => {
                st.outside.insert(frn);
            }
        }
    }

    let path_of = |name: &[u8], parent: u64| -> Option<Vec<u8>> {
        let mut names: Vec<&[u8]> = vec![name];
        let mut cur = parent;
        for _ in 0..=changes.len() {
            let mapped = st.live(tree, cur);
            let base = match (mapped, pos.get(&cur)) {
                (Some(NodeId::ROOT), _) => NodeId::ROOT,
                (m, Some(&k))
                    if m.is_none() || changes[k].reasons & reason::RENAME_NEW_NAME != 0 =>
                {
                    names.push(&changes[k].name);
                    cur = changes[k].parent_frn;
                    continue;
                }
                (Some(id), _) => id,
                (None, _) => return None,
            };
            if !tree.node(base).is_dir() || tree.behind_barrier(base) {
                return None;
            }
            let mut rel = tree.rel_path_wtf8(base);
            for n in names.iter().rev() {
                if !rel.is_empty() {
                    rel.push(b'\\');
                }
                rel.extend_from_slice(n);
            }
            return Some(rel);
        }
        None // a cycle: corrupt input
    };

    let entries = changes
        .iter()
        .map(|c| {
            if deleted(c) {
                return None;
            }
            let known_dir = st
                .live(tree, c.frn)
                .is_some_and(|id| tree.node(id).is_dir());
            if known_dir && c.reasons & reason::REPARSE_POINT_CHANGE == 0 {
                return None;
            }
            path_of(&c.name, c.parent_frn)
        })
        .collect();
    let links = changes
        .iter()
        .map(|c| c.links.iter().map(|(p, n)| path_of(n, *p)).collect())
        .collect();
    (entries, links)
}

/// The files below `dir`, just filled by a scan, whose bytes it counted
/// (up to the probe budget): the scan only knew the names it saw itself.
fn scanned_files(tree: &Tree, dir: NodeId, out: &mut Vec<NodeId>) {
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for c in tree.children(d) {
            if out.len() >= LINK_PROBES {
                return;
            }
            let n = tree.node(c);
            if n.is_dir() {
                if !n.flags.intersects(BARRIER) {
                    stack.push(c);
                }
            } else if !n
                .flags
                .intersects(NodeFlags(NOT_LINKED.0 | NodeFlags::HARDLINK.0))
                && (n.size | n.logical) != 0
            {
                out.push(c);
            }
        }
    }
}

fn write_lock(shared: &SharedTree) -> std::sync::RwLockWriteGuard<'_, Tree> {
    shared.write().unwrap_or_else(|e| e.into_inner())
}

fn read_lock(shared: &SharedTree) -> std::sync::RwLockReadGuard<'_, Tree> {
    shared.read().unwrap_or_else(|e| e.into_inner())
}

/// Applies one batch and reports what changed. Only fails with
/// [`Error::Cancelled`] (a directory scan was cancelled: the watcher stops).
pub(crate) fn apply_batch(
    shared: &SharedTree,
    mut batch: Batch,
    probe: &mut dyn Probe,
    mut usn: Option<&mut UsnState>,
    scan_dir: &mut ScanDir<'_>,
) -> Result<BatchReport> {
    // 1. Re-read what the notifications did not carry, without any lock.
    for pc in &mut batch.paths {
        if pc.info == Info::Unknown {
            pc.info = probe.stat(&pc.path);
        }
    }
    let mut usn_info = Vec::new();
    let mut usn_link_info = Vec::new();
    if let (Some(st), false) = (usn.as_deref_mut(), batch.usn.is_empty()) {
        let (paths, link_paths) = usn_paths(&read_lock(shared), st, probe, &batch.usn);
        let mut stat = |p: &Option<Vec<u8>>| p.as_ref().map_or(Info::Unknown, |p| probe.stat(p));
        usn_info = paths.iter().map(&mut stat).collect();
        usn_link_info = link_paths
            .iter()
            .map(|l| l.iter().map(&mut stat).collect())
            .collect();
    }
    batch
        .paths
        .sort_by_cached_key(|p| component_spans(&p.path).len());

    // 2. Apply everything under one write lock.
    let mut t = Tally::default();
    {
        let mut tree = write_lock(shared);
        let mut ap = Applier {
            tree: &mut tree,
            probe,
            t: &mut t,
        };
        for &id in &batch.rescan {
            ap.rescan_request(id);
        }
        for r in &batch.renames {
            ap.rename(r);
        }
        for pc in &batch.paths {
            ap.path_change(pc);
        }
        if let Some(st) = usn.as_deref_mut() {
            ap.usn_batch(st, &batch.usn, &usn_info, &usn_link_info);
        }
        for dir in &batch.overflow {
            ap.overflow(dir);
        }
    }

    // 3. Fill new directories; each scan takes the lock as it needs.
    while !t.scans.is_empty() {
        let mut failed: HashSet<Vec<u8>> = HashSet::new();
        for (id, rel) in std::mem::take(&mut t.scans) {
            match scan_dir(shared, id, &rel) {
                // The scan counted each file once among the names it saw;
                // names elsewhere in the tree are checked below. A scan of
                // the root saw them all.
                Ok(()) if id != NodeId::ROOT && probe.tracks_links() => {
                    scanned_files(&read_lock(shared), id, &mut t.link_nodes);
                }
                Ok(()) => {}
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(Error::Unsupported(_)) => {
                    // The entries held back for it are applied below, but
                    // anything that came in without an event is missing.
                    if !t.rescan.contains(&id) {
                        t.rescan.push(id);
                    }
                    failed.insert(fold_key(&rel));
                }
                Err(_) => {
                    let mut tree = write_lock(shared);
                    if tree.is_live(id) {
                        let flags = tree.node(id).flags | NodeFlags::INACCESSIBLE;
                        tree.set_flags(id, flags);
                    }
                }
            }
        }
        t.scan_keys.clear();
        t.scan_nodes.clear();
        let deferred = std::mem::take(&mut t.deferred);
        let mut retry: Vec<PathChange> = deferred
            .into_iter()
            .filter(|pc| {
                let key = fold_key(&pc.path);
                key.iter()
                    .enumerate()
                    .any(|(i, &b)| b == b'\\' && failed.contains(&key[..i]))
            })
            .collect();
        if retry.is_empty() {
            break;
        }
        retry.sort_by_cached_key(|p| component_spans(&p.path).len());
        let mut tree = write_lock(shared);
        let mut ap = Applier {
            tree: &mut tree,
            probe,
            t: &mut t,
        };
        for pc in &retry {
            ap.path_change(pc);
        }
    }

    // 4. Hard links: link counts of the files that may have gained or lost a
    //    name, then the names of those with several (or that count no
    //    bytes), all without any lock.
    let mut found = Vec::new();
    if probe.tracks_links() {
        let mut ids = std::mem::take(&mut t.link_ids);
        ids.extend_from_slice(&batch.unlinked);
        let mut nodes = std::mem::take(&mut t.link_nodes);
        nodes.sort_unstable();
        nodes.dedup();
        let nodes: Vec<(Vec<u8>, bool)> = {
            let tree = read_lock(shared);
            nodes
                .into_iter()
                .filter(|&id| {
                    tree.is_live(id)
                        && !tree.node(id).flags.intersects(NOT_LINKED)
                        && !tree.behind_barrier(tree.node(id).parent)
                })
                .map(|id| {
                    let n = tree.node(id);
                    (
                        tree.rel_path_wtf8(id),
                        n.flags.contains(NodeFlags::HARDLINK),
                    )
                })
                .collect()
        };
        let mut budget = LINK_PROBES;
        for (rel, hardlink) in nodes {
            if budget == 0 {
                break;
            }
            budget -= 1;
            match probe.link_count(&rel) {
                // A name that counts no bytes may be the last one left.
                Some((id, n)) if n > 1 || hardlink => ids.push(id),
                _ => {}
            }
        }
        ids.retain(|&id| id != 0 && id != u64::MAX);
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            if budget == 0 {
                break;
            }
            budget -= 1;
            if let Some(links) = probe.links(id) {
                found.push((id, links));
            }
        }
    }

    // 5. One name of each such file counts its bytes.
    if !found.is_empty() {
        let mut tree = write_lock(shared);
        let mut ap = Applier {
            tree: &mut tree,
            probe,
            t: &mut t,
        };
        for (id, links) in &found {
            if let Some(keep) = ap.reconcile_links(links) {
                if let Some(st) = usn.as_deref_mut() {
                    st.insert(*id, keep);
                }
            }
        }
    }

    let tree = read_lock(shared);
    let mut dirs: Vec<NodeId> = t.dirs.into_iter().filter(|&d| tree.is_live(d)).collect();
    dirs.sort_unstable();
    Ok(BatchReport {
        events: batch.events,
        applied: t.created + t.deleted + t.modified + t.moved,
        created: t.created,
        deleted: t.deleted,
        modified: t.modified,
        moved: t.moved,
        dirs,
        rescan_needed: t.rescan,
        error: batch.failed,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::tree::EntryBatch;
    use crate::watch::coalesce::{Coalescer, Event, UsnRecord};

    /// An in-memory file system: folded relative path -> (path, stat).
    #[derive(Default, Clone)]
    pub(crate) struct FakeFs {
        pub entries: BTreeMap<Vec<u8>, (Vec<u8>, Stat)>,
        /// Short name -> long name, by folded relative path.
        pub short: HashMap<Vec<u8>, Vec<u8>>,
        /// Reference number -> relative path.
        pub ids: HashMap<u64, Vec<u8>>,
        /// File ids by folded relative path, for files given one: names
        /// with the same id are hard links of one file.
        pub file_ids: HashMap<Vec<u8>, u64>,
        pub probes: usize,
        pub link_probes: usize,
    }

    pub(crate) fn file(size: u64) -> Stat {
        Stat {
            attributes: 0x20,
            reparse_tag: 0,
            size: size.div_ceil(4096) * 4096,
            logical: size,
            mtime: 0,
        }
    }

    pub(crate) fn dir() -> Stat {
        Stat {
            attributes: attr::DIRECTORY,
            ..Stat::default()
        }
    }

    impl FakeFs {
        pub fn put(&mut self, rel: &str, stat: Stat) {
            self.entries
                .insert(fold_key(rel.as_bytes()), (rel.as_bytes().to_vec(), stat));
        }

        pub fn remove(&mut self, rel: &str) {
            let key = fold_key(rel.as_bytes());
            let mut below = key.clone();
            below.push(b'\\');
            self.entries
                .retain(|k, _| *k != key && !k.starts_with(&below));
            self.file_ids
                .retain(|k, _| *k != key && !k.starts_with(&below));
        }

        /// Gives the file at `rel` the id `id`.
        pub fn set_id(&mut self, rel: &str, id: u64) {
            self.file_ids.insert(fold_key(rel.as_bytes()), id);
        }

        /// Adds `new` as another name of the file at `existing` (which
        /// needs an id).
        pub fn link(&mut self, existing: &str, new: &str) {
            let key = fold_key(existing.as_bytes());
            let stat = self.entries[&key].1;
            let id = self.file_ids[&key];
            self.put(new, stat);
            self.set_id(new, id);
        }

        /// Renames the file at `from` to `to`, keeping its id.
        pub fn mv(&mut self, from: &str, to: &str) {
            let key = fold_key(from.as_bytes());
            let stat = self.entries[&key].1;
            let id = self.file_ids.get(&key).copied();
            self.remove(from);
            self.put(to, stat);
            if let Some(id) = id {
                self.set_id(to, id);
            }
        }

        /// Gives the file at `rel`, under all its names, new attributes.
        pub fn write(&mut self, rel: &str, stat: Stat) {
            let key = fold_key(rel.as_bytes());
            let keys = match self.file_ids.get(&key) {
                Some(&id) => self.names_of(id),
                None => vec![key],
            };
            for k in keys {
                self.entries.get_mut(&k).unwrap().1 = stat;
            }
        }

        /// The folded keys of the names of file `id`, sorted.
        fn names_of(&self, id: u64) -> Vec<Vec<u8>> {
            let mut keys: Vec<Vec<u8>> = self
                .file_ids
                .iter()
                .filter(|&(_, &v)| v == id)
                .map(|(k, _)| k.clone())
                .collect();
            keys.sort();
            keys
        }

        /// Fills `dir` (relative path `rel`) like `walk::scan_into` would,
        /// counting each file once among the names this scan sees.
        pub fn scan(&self, shared: &SharedTree, dir: NodeId, rel: &[u8]) -> Result<()> {
            self.scan_with(shared, dir, rel, &mut HashSet::new())
        }

        fn scan_with(
            &self,
            shared: &SharedTree,
            dir: NodeId,
            rel: &[u8],
            seen: &mut HashSet<u64>,
        ) -> Result<()> {
            let key = fold_key(rel);
            let mut b = EntryBatch::new();
            let mut subdirs = Vec::new();
            for (k, (path, stat)) in &self.entries {
                let parent = match k.iter().rposition(|&c| c == b'\\') {
                    Some(i) => &k[..i],
                    None => &k[..0],
                };
                if parent != &key[..] {
                    continue;
                }
                let (_, name) = split_last(path);
                let mut flags = stat.flags();
                let (s, l) = if stat.is_dir() {
                    (0, 0)
                } else if self.file_ids.get(k).is_some_and(|&id| !seen.insert(id)) {
                    flags |= NodeFlags::HARDLINK;
                    (0, 0)
                } else {
                    (stat.size, stat.logical)
                };
                b.push_wtf8(name, s, l, flags, stat.mtime);
                if stat.is_dir() && !stat.flags().contains(NodeFlags::REPARSE) {
                    subdirs.push(path.clone());
                }
            }
            let range = shared.write().unwrap().add_children(dir, &b);
            for (i, id) in range.enumerate() {
                let e = b.entries()[i];
                if e.flags.contains(NodeFlags::DIR) && !e.flags.contains(NodeFlags::REPARSE) {
                    let name = b.name(&e).to_vec();
                    let sub = subdirs
                        .iter()
                        .find(|p| split_last(p).1 == &name[..])
                        .unwrap()
                        .clone();
                    self.scan_with(shared, NodeId(id), &sub, seen)?;
                }
            }
            Ok(())
        }
    }

    impl Probe for FakeFs {
        fn stat(&mut self, rel: &[u8]) -> Info {
            self.probes += 1;
            match self.entries.get(&fold_key(rel)) {
                Some((_, s)) => Info::Present(*s),
                None => Info::Gone,
            }
        }

        fn long_name(&mut self, rel: &[u8]) -> Option<Vec<u8>> {
            self.short.get(&fold_key(rel)).cloned()
        }

        fn locate(&mut self, frn: u64) -> Option<Vec<u8>> {
            self.ids.get(&frn).cloned()
        }

        fn tracks_links(&self) -> bool {
            true
        }

        fn link_count(&mut self, rel: &[u8]) -> Option<(u64, u32)> {
            self.link_probes += 1;
            let id = *self.file_ids.get(&fold_key(rel))?;
            Some((id, self.names_of(id).len() as u32))
        }

        fn links(&mut self, id: u64) -> Option<Links> {
            self.link_probes += 1;
            let keys = self.names_of(id);
            let stat = self.entries.get(keys.first()?)?.1;
            Some(Links {
                size: stat.size,
                logical: stat.logical,
                names: keys.iter().map(|k| self.entries[k].0.clone()).collect(),
            })
        }
    }

    /// A tree matching `fs`, built with its scanner.
    pub(crate) fn tree_of(fs: &FakeFs) -> SharedTree {
        let shared = Tree::new(b"R:\\").into_shared();
        fs.scan(&shared, NodeId::ROOT, b"").unwrap();
        shared
    }

    fn sample() -> FakeFs {
        let mut fs = FakeFs::default();
        fs.put("docs", dir());
        fs.put("docs\\a.txt", file(100));
        fs.put("docs\\b.txt", file(5000));
        fs.put("docs\\old", dir());
        fs.put("docs\\old\\c.bin", file(10_000));
        fs.put("music", dir());
        fs.put("music\\song.mp3", file(4_000_000));
        fs.put("top.txt", file(1));
        fs
    }

    fn run(shared: &SharedTree, fs: &mut FakeFs, events: Vec<Event>) -> BatchReport {
        let mut c = Coalescer::default();
        c.extend(events);
        let snapshot = fs.clone();
        let mut scan = |t: &SharedTree, d: NodeId, rel: &[u8]| snapshot.scan(t, d, rel);
        let r = apply_batch(shared, c.take(), fs, None, &mut scan).unwrap();
        shared.read().unwrap().check_invariants().unwrap();
        r
    }

    fn changed(path: &str) -> Event {
        Event::Changed {
            path: path.as_bytes().to_vec(),
            info: Info::Unknown,
        }
    }

    fn renamed(from: &str, to: &str) -> Event {
        Event::Renamed {
            from: from.as_bytes().to_vec(),
            to: to.as_bytes().to_vec(),
            info: Info::Unknown,
        }
    }

    fn get(shared: &SharedTree, rel: &str) -> Option<(NodeId, u64, u64, NodeFlags)> {
        let t = shared.read().unwrap();
        let id = t.lookup_relative(rel.as_bytes())?;
        let n = t.node(id);
        Some((id, n.size, n.logical, n.flags))
    }

    /// The live tree as sorted `path size/logical` lines.
    pub(crate) fn listing(shared: &SharedTree) -> Vec<String> {
        let t = shared.read().unwrap();
        let mut out = Vec::new();
        let mut stack = vec![NodeId::ROOT];
        while let Some(id) = stack.pop() {
            for c in t.children(id) {
                let n = t.node(c);
                let path = String::from_utf8_lossy(&t.rel_path_wtf8(c)).into_owned();
                out.push(if n.is_dir() {
                    format!("{path}\\")
                } else {
                    format!("{path} {}/{}", n.size, n.logical)
                });
                stack.push(c);
            }
        }
        out.sort();
        out
    }

    fn fs_listing(fs: &FakeFs) -> Vec<String> {
        let mut out: Vec<String> = fs
            .entries
            .values()
            .map(|(p, s)| {
                let path = String::from_utf8_lossy(p);
                if s.is_dir() {
                    format!("{path}\\")
                } else {
                    format!("{path} {}/{}", s.size, s.logical)
                }
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn flags_follow_attributes() {
        let f = flags_from_attributes(attr::DIRECTORY | attr::HIDDEN | attr::SYSTEM, 0);
        assert_eq!(f, NodeFlags::DIR | NodeFlags::HIDDEN | NodeFlags::SYSTEM);
        let junction = flags_from_attributes(attr::DIRECTORY | attr::REPARSE_POINT, 0xA000_0003);
        assert!(junction.contains(NodeFlags::REPARSE));
        // A cloud placeholder is a reparse point too, but is not a link.
        let cloud = flags_from_attributes(
            attr::REPARSE_POINT | attr::RECALL_ON_DATA_ACCESS,
            0x9000_601A,
        );
        assert_eq!(cloud, NodeFlags::CLOUD);
        assert_eq!(
            flags_from_attributes(attr::SPARSE_FILE, 0),
            NodeFlags::COMPRESSED
        );
    }

    #[test]
    fn move_node_keeps_invariants() {
        let fs = sample();
        let shared = tree_of(&fs);
        let mut t = shared.write().unwrap();
        let old = t.lookup_relative(b"docs\\old").unwrap();
        let music = t.lookup_relative(b"music").unwrap();
        let docs = t.lookup_relative(b"docs").unwrap();
        let before = t.node(NodeId::ROOT).size;
        assert!(t.move_node(old, music, b"archive"));
        t.check_invariants().unwrap();
        assert!(t.lookup_relative(b"music\\archive\\c.bin").is_some());
        assert_eq!(t.child_count(docs), 2);
        assert_eq!(t.node(NodeId::ROOT).size, before);
        // Into its own subtree, or the root: refused.
        assert!(!t.move_node(music, old, b"loop"));
        assert!(!t.move_node(NodeId::ROOT, music, b"x"));
        // Rename in place.
        assert!(t.move_node(old, music, b"Archive"));
        assert_eq!(t.name(old), b"Archive");
        t.check_invariants().unwrap();
        assert_eq!(t.rel_path_wtf8(old), b"music\\Archive");
    }

    #[test]
    fn create_modify_delete() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        fs.put("docs\\new.log", file(777));
        fs.put("docs\\a.txt", file(9000));
        fs.remove("docs\\b.txt");
        let r = run(
            &shared,
            &mut fs,
            vec![
                changed("docs\\new.log"),
                changed("docs\\a.txt"),
                changed("docs\\a.txt"),
                changed("docs\\b.txt"),
            ],
        );
        assert_eq!((r.created, r.modified, r.deleted), (1, 1, 1));
        assert_eq!(r.events, 4);
        assert_eq!(listing(&shared), fs_listing(&fs));
        let docs = get(&shared, "docs").unwrap().0;
        assert_eq!(r.dirs, vec![docs]);
        // Nothing changed: nothing applied.
        let r = run(&shared, &mut fs, vec![changed("docs\\a.txt")]);
        assert_eq!(r.applied, 0);
    }

    #[test]
    fn extended_info_avoids_probes() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        fs.probes = 0;
        let r = run(
            &shared,
            &mut fs,
            vec![
                Event::Changed {
                    path: b"top.txt".to_vec(),
                    info: Info::Present(file(4097)),
                },
                Event::Changed {
                    path: b"music\\song.mp3".to_vec(),
                    info: Info::Gone,
                },
            ],
        );
        assert_eq!(fs.probes, 0);
        assert_eq!((r.modified, r.deleted), (1, 1));
        assert_eq!(get(&shared, "top.txt").unwrap().1, 8192);
        assert!(get(&shared, "music\\song.mp3").is_none());
    }

    #[test]
    fn deleting_a_directory_with_contents() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        fs.remove("docs");
        let r = run(
            &shared,
            &mut fs,
            vec![
                changed("docs\\old\\c.bin"),
                changed("docs\\old"),
                changed("docs\\a.txt"),
                changed("docs\\b.txt"),
                changed("docs"),
            ],
        );
        // Shallow first: the directory goes, its contents with it.
        assert_eq!(r.deleted, 1);
        assert_eq!(listing(&shared), fs_listing(&fs));
    }

    #[test]
    fn renames_keep_node_identity() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        let old_id = get(&shared, "docs\\old").unwrap().0;
        let c_id = get(&shared, "docs\\old\\c.bin").unwrap().0;
        // Rename a directory, move a file across directories, then rename
        // the file and change its size.
        fs.remove("docs\\old");
        fs.put("docs\\older", dir());
        fs.put("docs\\older\\c.bin", file(10_000));
        fs.remove("top.txt");
        fs.put("music\\top2.txt", file(50));
        let r = run(
            &shared,
            &mut fs,
            vec![
                renamed("docs\\old", "docs\\older"),
                renamed("top.txt", "music\\top.txt"),
                renamed("music\\top.txt", "music\\top2.txt"),
                changed("music\\top2.txt"),
            ],
        );
        assert_eq!(r.moved, 3);
        assert_eq!(r.created + r.deleted, 0);
        assert_eq!(listing(&shared), fs_listing(&fs));
        assert_eq!(get(&shared, "docs\\older").unwrap().0, old_id);
        assert_eq!(get(&shared, "docs\\older\\c.bin").unwrap().0, c_id);
    }

    #[test]
    fn changes_below_a_renamed_directory_follow_it() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        fs.remove("docs\\old");
        fs.put("docs\\new", dir());
        fs.put("docs\\new\\c.bin", file(123_456));
        // The size change is reported under the old name, before the rename.
        let r = run(
            &shared,
            &mut fs,
            vec![
                changed("docs\\old\\c.bin"),
                renamed("docs\\old", "docs\\new"),
            ],
        );
        assert_eq!((r.moved, r.modified), (1, 1));
        assert_eq!(listing(&shared), fs_listing(&fs));
    }

    #[test]
    fn case_only_rename_and_replace() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        fs.remove("docs\\a.txt");
        fs.put("docs\\A.TXT", file(100));
        let r = run(
            &shared,
            &mut fs,
            vec![renamed("docs\\a.txt", "docs\\A.TXT")],
        );
        assert_eq!(r.moved, 1);
        let t = shared.read().unwrap();
        let id = t.lookup_relative(b"docs\\a.txt").unwrap();
        assert_eq!(t.name(id), b"A.TXT");
        drop(t);
        // Rename onto an existing file replaces it.
        fs.remove("docs\\b.txt");
        fs.remove("docs\\A.TXT");
        fs.put("docs\\b.txt", file(100));
        let r = run(
            &shared,
            &mut fs,
            vec![renamed("docs\\A.TXT", "docs\\b.txt")],
        );
        assert_eq!((r.moved, r.deleted), (1, 1));
        assert_eq!(listing(&shared), fs_listing(&fs));
        assert_eq!(get(&shared, "docs\\b.txt").unwrap().0, id);
    }

    #[test]
    fn created_then_renamed_in_one_batch() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        fs.put("docs\\final.txt", file(10));
        let r = run(
            &shared,
            &mut fs,
            vec![
                changed("docs\\tmp.txt"),
                renamed("docs\\tmp.txt", "docs\\final.txt"),
            ],
        );
        assert_eq!((r.created, r.moved), (1, 0));
        assert_eq!(listing(&shared), fs_listing(&fs));
    }

    #[test]
    fn new_directory_is_scanned_once() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        fs.put("proj", dir());
        fs.put("proj\\src", dir());
        fs.put("proj\\src\\main.rs", file(300));
        fs.put("proj\\Cargo.toml", file(80));
        let r = run(
            &shared,
            &mut fs,
            vec![
                changed("proj"),
                changed("proj\\src"),
                changed("proj\\src\\main.rs"),
                changed("proj\\Cargo.toml"),
            ],
        );
        assert_eq!(r.created, 1, "the contents come from the scan");
        assert!(r.rescan_needed.is_empty());
        assert_eq!(listing(&shared), fs_listing(&fs));
        // Late events for scanned entries are idempotent.
        let r = run(&shared, &mut fs, vec![changed("proj\\src\\main.rs")]);
        assert_eq!(r.applied, 0);
        assert_eq!(listing(&shared), fs_listing(&fs));
    }

    #[test]
    fn without_a_scanner_events_still_apply() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        fs.put("proj", dir());
        fs.put("proj\\src", dir());
        fs.put("proj\\src\\main.rs", file(300));
        let mut c = Coalescer::default();
        c.extend(vec![
            changed("proj"),
            changed("proj\\src"),
            changed("proj\\src\\main.rs"),
        ]);
        let mut unsupported =
            |_: &SharedTree, _: NodeId, _: &[u8]| Err(Error::Unsupported("no walk"));
        let r = apply_batch(&shared, c.take(), &mut fs, None, &mut unsupported).unwrap();
        shared.read().unwrap().check_invariants().unwrap();
        assert_eq!(r.created, 3);
        assert_eq!(listing(&shared), fs_listing(&fs));
        let proj = get(&shared, "proj").unwrap().0;
        let src = get(&shared, "proj\\src").unwrap().0;
        assert_eq!(r.rescan_needed, vec![proj, src]);
    }

    #[test]
    fn unreadable_new_directory_is_flagged() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        fs.put("secret", dir());
        let mut c = Coalescer::default();
        c.push(changed("secret"));
        let mut denied = |_: &SharedTree, _: NodeId, _: &[u8]| {
            Err(Error::Io(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied,
            )))
        };
        apply_batch(&shared, c.take(), &mut fs, None, &mut denied).unwrap();
        let flags = get(&shared, "secret").unwrap().3;
        assert!(flags.contains(NodeFlags::INACCESSIBLE | NodeFlags::DIR));
        // Later changes inside are ignored.
        fs.put("secret\\x", file(1));
        let r = run(&shared, &mut fs, vec![changed("secret\\x")]);
        assert_eq!(r.applied, 0);
        let mut c = Coalescer::default();
        c.push(Event::Changed {
            path: b"x".to_vec(),
            info: Info::Unknown,
        });
        let mut cancelled = |_: &SharedTree, _: NodeId, _: &[u8]| Err(Error::Cancelled);
        fs.put("x", dir());
        assert!(matches!(
            apply_batch(&shared, c.take(), &mut fs, None, &mut cancelled),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn ignored_below_reparse_points_and_inaccessible_dirs() {
        let mut fs = sample();
        fs.put(
            "link",
            Stat {
                attributes: attr::DIRECTORY | attr::REPARSE_POINT,
                reparse_tag: 0xA000_0003,
                ..Stat::default()
            },
        );
        let shared = tree_of(&fs);
        {
            let mut t = shared.write().unwrap();
            let music = t.lookup_relative(b"music").unwrap();
            let f = t.node(music).flags | NodeFlags::INACCESSIBLE;
            t.set_flags(music, f);
        }
        fs.put("link\\inside.txt", file(10));
        fs.put("music\\new.mp3", file(10));
        let r = run(
            &shared,
            &mut fs,
            vec![changed("link\\inside.txt"), changed("music\\new.mp3")],
        );
        assert_eq!(r.applied, 0);
        assert!(get(&shared, "link").unwrap().3.contains(NodeFlags::REPARSE));
        assert_eq!(
            shared
                .read()
                .unwrap()
                .child_count(get(&shared, "link").unwrap().0),
            0
        );
        // The junction itself can still be deleted.
        fs.remove("link");
        let r = run(&shared, &mut fs, vec![changed("link")]);
        assert_eq!(r.deleted, 1);
    }

    #[test]
    fn missing_ancestor_is_brought_in_with_a_scan() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        // Created while nobody watched; only a deep change is reported.
        fs.put("games", dir());
        fs.put("games\\saves", dir());
        fs.put("games\\saves\\slot1", file(2000));
        fs.put("games\\config.ini", file(20));
        let r = run(&shared, &mut fs, vec![changed("games\\saves\\slot1")]);
        assert_eq!(r.created, 1);
        assert_eq!(listing(&shared), fs_listing(&fs));
    }

    #[test]
    fn file_replaced_by_directory() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        fs.remove("top.txt");
        fs.put("top.txt", dir());
        fs.put("top.txt\\inner", file(5));
        let r = run(&shared, &mut fs, vec![changed("top.txt")]);
        assert_eq!((r.deleted, r.created), (1, 1));
        assert_eq!(listing(&shared), fs_listing(&fs));
        // And back, with an event for the gone child in the same batch.
        fs.remove("top.txt");
        fs.put("top.txt", file(3));
        let r = run(
            &shared,
            &mut fs,
            vec![changed("top.txt\\inner"), changed("top.txt")],
        );
        assert_eq!((r.deleted, r.created), (1, 1));
        assert_eq!(listing(&shared), fs_listing(&fs));
    }

    #[test]
    fn short_names_resolve_to_long_names() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        fs.short
            .insert(fold_key(b"MUSIC\\SONG~1.MP3"), b"song.mp3".to_vec());
        fs.put("music\\song.mp3", file(5_000_000));
        // The probe reaches the file under its short name too.
        fs.put("music\\SONG~1.MP3", file(5_000_000));
        let r = run(&shared, &mut fs, vec![changed("MUSIC\\SONG~1.MP3")]);
        assert_eq!((r.modified, r.created), (1, 0));
        assert_eq!(get(&shared, "music\\song.mp3").unwrap().2, 5_000_000);
    }

    #[test]
    fn overflow_and_rescan_requests() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        let r = run(&shared, &mut fs, vec![Event::Overflow(Vec::new())]);
        assert_eq!(r.rescan_needed, vec![NodeId::ROOT]);
        let docs = get(&shared, "docs").unwrap().0;
        let r = run(&shared, &mut fs, vec![Event::Overflow(b"docs".to_vec())]);
        assert_eq!(r.rescan_needed, vec![docs]);

        // A requested rescan replaces the directory's contents.
        fs.put("docs\\x.txt", file(1));
        fs.remove("docs\\old");
        let r = run(&shared, &mut fs, vec![Event::Rescan(docs)]);
        assert_eq!(r.dirs, vec![docs]);
        assert_eq!(listing(&shared), fs_listing(&fs));
        let r = run(&shared, &mut fs, vec![Event::Failed("bye".into())]);
        assert_eq!(r.error.as_deref(), Some("bye"));
    }

    // ---- Hard links -----------------------------------------------------

    fn root_size(shared: &SharedTree) -> (u64, u64) {
        let t = shared.read().unwrap();
        let n = t.node(NodeId::ROOT);
        (n.size, n.logical)
    }

    /// The tree has the paths of `fs`, and each file's bytes are counted on
    /// exactly one of its names (the others are zero-size `HARDLINK`s).
    fn check_links(shared: &SharedTree, fs: &FakeFs) {
        let strip = |l: &String| l.split(' ').next().unwrap().to_owned();
        let tree_paths: Vec<String> = listing(shared).iter().map(strip).collect();
        let fs_paths: Vec<String> = fs_listing(fs).iter().map(strip).collect();
        assert_eq!(tree_paths, fs_paths);
        let t = shared.read().unwrap();
        t.check_invariants().unwrap();
        let mut files: HashMap<u64, Vec<&Vec<u8>>> = HashMap::new();
        for (k, (path, stat)) in &fs.entries {
            if stat.is_dir() {
                continue;
            }
            match fs.file_ids.get(k) {
                Some(&id) => files.entry(id).or_default().push(path),
                None => {
                    let n = t.node(t.lookup_relative(path).unwrap());
                    assert!(!n.flags.contains(NodeFlags::HARDLINK));
                    assert_eq!((n.size, n.logical), (stat.size, stat.logical));
                }
            }
        }
        for paths in files.values() {
            let stat = fs.entries[&fold_key(paths[0])].1;
            let nodes: Vec<&crate::Node> = paths
                .iter()
                .map(|p| t.node(t.lookup_relative(p).unwrap()))
                .collect();
            let counting: Vec<_> = nodes
                .iter()
                .filter(|n| !n.flags.contains(NodeFlags::HARDLINK))
                .collect();
            let names: Vec<_> = paths.iter().map(|p| String::from_utf8_lossy(p)).collect();
            assert_eq!(counting.len(), 1, "one of {names:?} counts the bytes");
            assert_eq!(
                (counting[0].size, counting[0].logical),
                (stat.size, stat.logical),
                "{names:?}"
            );
            for n in &nodes {
                if n.flags.contains(NodeFlags::HARDLINK) {
                    assert_eq!((n.size, n.logical), (0, 0), "{names:?}");
                }
            }
        }
    }

    /// The name of file `id` that counts its bytes.
    fn counting(shared: &SharedTree, fs: &FakeFs, id: u64) -> String {
        let t = shared.read().unwrap();
        fs.names_of(id)
            .iter()
            .map(|k| fs.entries[k].0.clone())
            .find(|p| {
                let n = t.node(t.lookup_relative(p).unwrap());
                !n.flags.contains(NodeFlags::HARDLINK)
            })
            .map(|p| String::from_utf8(p).unwrap())
            .unwrap()
    }

    /// `sample()` with docs\b.txt also named music\b-link.txt.
    fn linked() -> FakeFs {
        let mut fs = sample();
        fs.set_id("docs\\b.txt", 500);
        fs.link("docs\\b.txt", "music\\b-link.txt");
        fs
    }

    #[test]
    fn scans_count_hard_links_once() {
        let fs = linked();
        let shared = tree_of(&fs);
        check_links(&shared, &fs);
        let without = tree_of(&sample());
        assert_eq!(root_size(&shared), root_size(&without));
    }

    #[test]
    fn a_new_hard_link_is_counted_once() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        let before = root_size(&shared);
        fs.set_id("docs\\b.txt", 500);
        fs.link("docs\\b.txt", "music\\b-link.txt");
        let stat = fs.entries[&fold_key(b"music\\b-link.txt")].1;
        // As reported with extended information (sizes, no link count).
        let r = run(
            &shared,
            &mut fs,
            vec![Event::Changed {
                path: b"music\\b-link.txt".to_vec(),
                info: Info::Present(stat),
            }],
        );
        assert_eq!(r.created, 1);
        check_links(&shared, &fs);
        assert_eq!(root_size(&shared), before);
        // The older name keeps counting the bytes.
        assert_eq!(counting(&shared, &fs, 500), "docs\\b.txt");
        // A plain new file costs one link-count probe and nothing else.
        fs.link_probes = 0;
        fs.put("music\\new.mp3", file(1));
        run(&shared, &mut fs, vec![changed("music\\new.mp3")]);
        assert_eq!(fs.link_probes, 1);
    }

    #[test]
    fn removing_the_counting_name_moves_the_bytes() {
        for first in [false, true] {
            let mut fs = linked();
            let shared = tree_of(&fs);
            let before = root_size(&shared);
            let sized = counting(&shared, &fs, 500);
            fs.remove(&sized);
            let mut events = vec![changed(&sized)];
            // The removal's notification may come before or after the id.
            if first {
                events.insert(0, Event::Unlinked(500));
            } else {
                events.push(Event::Unlinked(500));
            }
            let r = run(&shared, &mut fs, events);
            assert_eq!((r.deleted, r.modified), (1, 1));
            check_links(&shared, &fs);
            assert_eq!(root_size(&shared), before);
        }
        // Removing a name that counts nothing changes nothing else.
        let mut fs = linked();
        let shared = tree_of(&fs);
        let before = root_size(&shared);
        let sized = counting(&shared, &fs, 500);
        let other = if sized == "docs\\b.txt" {
            "music\\b-link.txt"
        } else {
            "docs\\b.txt"
        };
        fs.remove(other);
        let r = run(&shared, &mut fs, vec![changed(other), Event::Unlinked(500)]);
        assert_eq!((r.deleted, r.modified), (1, 0));
        check_links(&shared, &fs);
        assert_eq!(root_size(&shared), before);
        // The last name gone: nothing left to reconcile.
        fs.remove(&sized);
        run(
            &shared,
            &mut fs,
            vec![changed(&sized), Event::Unlinked(500)],
        );
        check_links(&shared, &fs);
    }

    #[test]
    fn writing_through_another_name_updates_the_counting_one() {
        let mut fs = linked();
        let shared = tree_of(&fs);
        let sized = counting(&shared, &fs, 500);
        let other = if sized == "docs\\b.txt" {
            "music\\b-link.txt"
        } else {
            "docs\\b.txt"
        };
        fs.write(other, file(80_000));
        let r = run(&shared, &mut fs, vec![changed(other)]);
        assert_eq!(r.modified, 1);
        check_links(&shared, &fs);
        assert_eq!(counting(&shared, &fs, 500), sized);
        // Renaming a name keeps the counts.
        fs.mv(other, "music\\renamed.txt");
        let r = run(&shared, &mut fs, vec![renamed(other, "music\\renamed.txt")]);
        assert_eq!(r.moved, 1);
        check_links(&shared, &fs);
    }

    #[test]
    fn scanned_directories_do_not_count_outside_links_twice() {
        let mut fs = linked();
        let shared = tree_of(&fs);
        let before = root_size(&shared);
        // A new directory holding another name of docs\b.txt (as a package
        // manager's store links files into a project).
        fs.put("proj", dir());
        fs.put("proj\\deps", dir());
        fs.link("docs\\b.txt", "proj\\deps\\b.txt");
        let r = run(&shared, &mut fs, vec![changed("proj")]);
        assert_eq!(r.created, 1);
        check_links(&shared, &fs);
        assert_eq!(root_size(&shared), before);
        // A rescanned directory too.
        let music = get(&shared, "music").unwrap().0;
        run(&shared, &mut fs, vec![Event::Rescan(music)]);
        check_links(&shared, &fs);
        assert_eq!(root_size(&shared), before);
        // A rescan of the root sees every name: no probes.
        fs.link_probes = 0;
        run(&shared, &mut fs, vec![Event::Rescan(NodeId::ROOT)]);
        assert_eq!(fs.link_probes, 0);
        check_links(&shared, &fs);
    }

    #[test]
    fn link_probes_are_bounded() {
        let mut fs = sample();
        let shared = tree_of(&fs);
        fs.put("many", dir());
        for i in 0..LINK_PROBES + 10 {
            fs.put(&format!("many\\{i}"), file(1));
        }
        run(&shared, &mut fs, vec![changed("many")]);
        assert_eq!(fs.link_probes, LINK_PROBES);
    }

    // ---- USN ------------------------------------------------------------

    use crate::watch::usn::reason::*;

    struct Usn {
        shared: SharedTree,
        fs: FakeFs,
        st: UsnState,
        frns: HashMap<String, u64>,
        next: u64,
    }

    impl Usn {
        fn new() -> Usn {
            let fs = sample();
            let shared = tree_of(&fs);
            let mut u = Usn {
                shared,
                fs,
                st: UsnState::new(FrnIndex::default()),
                frns: HashMap::new(),
                next: 100,
            };
            let root = u.frn("");
            u.st.index.insert(root, NodeId::ROOT);
            let paths: Vec<Vec<u8>> = u.fs.entries.values().map(|(p, _)| p.clone()).collect();
            for p in paths {
                let id = u.shared.read().unwrap().lookup_relative(&p).unwrap();
                let f = u.frn(std::str::from_utf8(&p).unwrap());
                u.st.index.insert(f, id);
            }
            u
        }

        /// A reference number per path (sequence number in the high bits).
        fn frn(&mut self, path: &str) -> u64 {
            let key = String::from_utf8(fold_key(path.as_bytes())).unwrap();
            if let Some(&f) = self.frns.get(&key) {
                return f;
            }
            self.next += 1;
            let f = self.next | (1 << 48);
            self.frns.insert(key, f);
            self.fs.ids.insert(f, path.as_bytes().to_vec());
            f
        }

        fn rec(&mut self, path: &str, reason: u32) -> Event {
            let (parent, name) = split_last(path.as_bytes());
            let parent = std::str::from_utf8(parent)
                .unwrap()
                .trim_end_matches('\\')
                .to_owned();
            let attributes = self
                .fs
                .entries
                .get(&fold_key(path.as_bytes()))
                .map_or(0x20, |(_, s)| s.attributes);
            Event::Usn(UsnRecord {
                frn: self.frn(path),
                parent_frn: self.frn(&parent),
                reason,
                attributes,
                name: name.to_vec(),
            })
        }

        fn apply(&mut self, events: Vec<Event>) -> BatchReport {
            let mut c = Coalescer::default();
            c.extend(events);
            let snapshot = self.fs.clone();
            let mut scan = |t: &SharedTree, d: NodeId, rel: &[u8]| snapshot.scan(t, d, rel);
            let r = apply_batch(
                &self.shared,
                c.take(),
                &mut self.fs,
                Some(&mut self.st),
                &mut scan,
            )
            .unwrap();
            self.shared.read().unwrap().check_invariants().unwrap();
            r
        }
    }

    #[test]
    fn usn_create_modify_delete() {
        let mut u = Usn::new();
        u.fs.put("docs\\new.txt", file(10));
        u.fs.put("docs\\a.txt", file(70_000));
        u.fs.remove("docs\\b.txt");
        u.fs.put("logs", dir());
        u.fs.put("logs\\1.log", file(3));
        let evs = vec![
            u.rec("docs\\new.txt", FILE_CREATE),
            u.rec("docs\\a.txt", DATA_EXTEND),
            u.rec("docs\\a.txt", DATA_EXTEND | CLOSE),
            u.rec("docs\\b.txt", FILE_DELETE | CLOSE),
            u.rec("logs\\1.log", FILE_CREATE), // child before parent: parent first
            u.rec("logs", FILE_CREATE),
        ];
        let r = u.apply(evs);
        assert_eq!((r.created, r.modified, r.deleted), (3, 1, 1));
        assert_eq!(listing(&u.shared), fs_listing(&u.fs));
        assert!(r.rescan_needed.is_empty());
        // New entries were indexed: a later change finds them by number.
        u.fs.put("logs\\1.log", file(5000));
        let ev = u.rec("logs\\1.log", DATA_EXTEND | CLOSE);
        let r = u.apply(vec![ev]);
        assert_eq!(r.modified, 1);
        assert_eq!(listing(&u.shared), fs_listing(&u.fs));
    }

    #[test]
    fn usn_renames_and_moves() {
        let mut u = Usn::new();
        let old_id = u
            .shared
            .read()
            .unwrap()
            .lookup_relative(b"docs\\old")
            .unwrap();
        let old_frn = u.frn("docs\\old");
        // Rename docs\old -> music\archive (same reference number).
        u.fs.remove("docs\\old");
        u.fs.put("music\\archive", dir());
        u.fs.put("music\\archive\\c.bin", file(10_000));
        let mut evs = vec![u.rec("docs\\old", RENAME_OLD_NAME)];
        let music = u.frn("music");
        evs.push(Event::Usn(UsnRecord {
            frn: old_frn,
            parent_frn: music,
            reason: RENAME_NEW_NAME,
            attributes: attr::DIRECTORY,
            name: b"archive".to_vec(),
        }));
        let r = u.apply(evs);
        assert_eq!(r.moved, 1);
        assert_eq!(listing(&u.shared), fs_listing(&u.fs));
        assert_eq!(
            u.shared
                .read()
                .unwrap()
                .lookup_relative(b"music\\archive")
                .unwrap(),
            old_id
        );
    }

    #[test]
    fn usn_outside_moves_and_reuse() {
        let mut u = Usn::new();
        // Moved out of the tree: the new parent is unknown.
        u.fs.remove("top.txt");
        let top = u.frn("top.txt");
        let elsewhere = 999_999;
        let r = u.apply(vec![Event::Usn(UsnRecord {
            frn: top,
            parent_frn: elsewhere,
            reason: RENAME_NEW_NAME,
            attributes: 0x20,
            name: b"top.txt".to_vec(),
        })]);
        assert_eq!(r.deleted, 1);
        assert!(u.st.outside.contains(&elsewhere));
        // Changes elsewhere on the volume are ignored.
        let r = u.apply(vec![Event::Usn(UsnRecord {
            frn: 5_000_000,
            parent_frn: elsewhere,
            reason: FILE_CREATE,
            attributes: 0x20,
            name: b"x".to_vec(),
        })]);
        assert_eq!(r.applied, 0);
        // A directory moved in from elsewhere is scanned.
        u.fs.put("music\\album", dir());
        u.fs.put("music\\album\\t1.flac", file(30_000_000));
        let ev = u.rec("music\\album", RENAME_NEW_NAME);
        let r = u.apply(vec![ev]);
        assert_eq!(r.created, 1);
        assert_eq!(listing(&u.shared), fs_listing(&u.fs));
        // Its contents are not indexed; a change inside is matched by name.
        u.fs.put("music\\album\\t1.flac", file(1));
        let ev = u.rec("music\\album\\t1.flac", DATA_TRUNCATION | CLOSE);
        let r = u.apply(vec![ev]);
        assert_eq!(r.modified, 1);
        assert_eq!(listing(&u.shared), fs_listing(&u.fs));
        // A record number freed and reused in one batch.
        let a = u.frn("docs\\a.txt");
        u.fs.remove("docs\\a.txt");
        u.fs.put("docs\\z.txt", file(42));
        let mut evs = vec![u.rec("docs\\z.txt", FILE_CREATE)];
        if let Event::Usn(r) = &mut evs[0] {
            r.frn = (a & RECORD_MASK) | (2 << 48);
        }
        evs.push(u.rec("docs\\a.txt", FILE_DELETE | CLOSE));
        let r = u.apply(evs);
        assert_eq!((r.created, r.deleted), (1, 1));
        assert_eq!(listing(&u.shared), fs_listing(&u.fs));
    }

    #[test]
    fn usn_locates_unindexed_directories() {
        let mut u = Usn::new();
        // Forget everything but the root, as after a rescan.
        let root = u.frn("");
        u.st.index = FrnIndex::default();
        u.st.index.insert(root, NodeId::ROOT);
        u.fs.put("docs\\old\\d.bin", file(64));
        let ev = u.rec("docs\\old\\d.bin", FILE_CREATE);
        let r = u.apply(vec![ev]);
        assert_eq!(r.created, 1);
        assert_eq!(listing(&u.shared), fs_listing(&u.fs));
        // An unindexed file renamed: found at its old location.
        u.fs.remove("docs\\b.txt");
        u.fs.put("docs\\old\\b2.txt", file(5000));
        let b = u.frn("docs\\b.txt");
        let old = u.frn("docs\\old");
        let mut evs = vec![u.rec("docs\\b.txt", RENAME_OLD_NAME)];
        evs.push(Event::Usn(UsnRecord {
            frn: b,
            parent_frn: old,
            reason: RENAME_NEW_NAME,
            attributes: 0x20,
            name: b"b2.txt".to_vec(),
        }));
        let r = u.apply(evs);
        assert_eq!(r.moved, 1);
        assert_eq!(listing(&u.shared), fs_listing(&u.fs));
    }

    #[test]
    fn usn_hard_links() {
        let mut u = Usn::new();
        let b = u.frn("docs\\b.txt");
        u.fs.set_id("docs\\b.txt", b);
        let (docs, music) = (u.frn("docs"), u.frn("music"));
        let before = root_size(&u.shared);
        let b_node = get(&u.shared, "docs\\b.txt").unwrap().0;
        let rec = |parent, name: &str, reason| {
            Event::Usn(UsnRecord {
                frn: b,
                parent_frn: parent,
                reason,
                attributes: 0x20,
                name: name.as_bytes().to_vec(),
            })
        };

        // A new name: added, without the bytes.
        u.fs.link("docs\\b.txt", "music\\b2.txt");
        let r = u.apply(vec![
            rec(music, "b2.txt", HARD_LINK_CHANGE),
            rec(music, "b2.txt", HARD_LINK_CHANGE | CLOSE),
        ]);
        assert_eq!(r.created, 1);
        check_links(&u.shared, &u.fs);
        assert_eq!(root_size(&u.shared), before);
        assert_eq!(counting(&u.shared, &u.fs, b), "docs\\b.txt");

        // Renaming that name moves it, not the name the index points at.
        u.fs.mv("music\\b2.txt", "music\\b3.txt");
        let r = u.apply(vec![
            rec(music, "b2.txt", RENAME_OLD_NAME),
            rec(music, "b3.txt", RENAME_NEW_NAME),
            rec(music, "b3.txt", RENAME_NEW_NAME | CLOSE),
        ]);
        assert_eq!(r.moved, 1);
        check_links(&u.shared, &u.fs);
        assert_eq!(get(&u.shared, "docs\\b.txt").unwrap().0, b_node);

        // Written through that name: the counting name follows.
        u.fs.write("music\\b3.txt", file(90_000));
        let r = u.apply(vec![rec(music, "b3.txt", DATA_EXTEND | CLOSE)]);
        assert_eq!(r.modified, 1);
        check_links(&u.shared, &u.fs);

        // The counting name removed: the other one takes the bytes over,
        // and the index follows it.
        u.fs.remove("docs\\b.txt");
        let r = u.apply(vec![
            rec(docs, "b.txt", HARD_LINK_CHANGE),
            rec(docs, "b.txt", HARD_LINK_CHANGE | CLOSE),
        ]);
        assert_eq!((r.deleted, r.modified), (1, 1));
        check_links(&u.shared, &u.fs);
        let b3 = get(&u.shared, "music\\b3.txt").unwrap().0;
        assert_eq!(u.st.live(&u.shared.read().unwrap(), b), Some(b3));

        // Both names removed in one batch, in either order: no leftovers.
        for second_first in [false, true] {
            u.fs.link("music\\b3.txt", "docs\\b4.txt");
            u.apply(vec![rec(docs, "b4.txt", HARD_LINK_CHANGE)]);
            check_links(&u.shared, &u.fs);
            let (a, z) = if second_first {
                ((docs, "b4.txt"), (music, "b3.txt"))
            } else {
                ((music, "b3.txt"), (docs, "b4.txt"))
            };
            let keep = u.fs.clone();
            u.fs.remove("music\\b3.txt");
            u.fs.remove("docs\\b4.txt");
            let r = u.apply(vec![
                rec(a.0, a.1, HARD_LINK_CHANGE | CLOSE),
                rec(z.0, z.1, FILE_DELETE | CLOSE),
            ]);
            assert_eq!(r.deleted, 2);
            check_links(&u.shared, &u.fs);
            // Back for the next round.
            u.fs = keep;
            u.fs.remove("docs\\b4.txt");
            let r = u.apply(vec![rec(music, "b3.txt", HARD_LINK_CHANGE | CLOSE)]);
            assert_eq!(r.created, 1);
            check_links(&u.shared, &u.fs);
        }
    }

    #[test]
    fn usn_moving_a_name_out_moves_the_bytes() {
        let mut u = Usn::new();
        let b = u.frn("docs\\b.txt");
        u.fs.set_id("docs\\b.txt", b);
        let music = u.frn("music");
        u.fs.link("docs\\b.txt", "music\\b2.txt");
        let r = u.apply(vec![Event::Usn(UsnRecord {
            frn: b,
            parent_frn: music,
            reason: HARD_LINK_CHANGE | CLOSE,
            attributes: 0x20,
            name: b"b2.txt".to_vec(),
        })]);
        assert_eq!(r.created, 1);
        let before = root_size(&u.shared);
        // The counting name moved out of the watched tree.
        u.fs.remove("docs\\b.txt");
        let r = u.apply(vec![Event::Usn(UsnRecord {
            frn: b,
            parent_frn: 999_999,
            reason: RENAME_NEW_NAME | CLOSE,
            attributes: 0x20,
            name: b"b.txt".to_vec(),
        })]);
        assert_eq!((r.deleted, r.modified), (1, 1));
        check_links(&u.shared, &u.fs);
        assert_eq!(root_size(&u.shared), before);
    }
}
