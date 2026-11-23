//! Parallel directory walk: no admin rights needed, any file system.
//!
//! # How it works
//!
//! * One task per directory on a dedicated work-stealing pool (rayon,
//!   [`ScanOptions::threads`] workers, default [`default_threads`]). A task
//!   carries the directory's node id and its full NT path (`\??\C:\...`,
//!   so paths beyond `MAX_PATH` work). The calling thread lists the scan
//!   root itself and only starts the pool if the root has subdirectories;
//!   with `threads == 1` everything runs on the calling thread.
//! * A task opens its directory once with `NtOpenFile` and lists it with
//!   `NtQueryDirectoryFile(FileIdFullDirectoryInformation)` into a 64 KiB
//!   buffer owned by the worker thread. One call returns hundreds of
//!   entries with sizes, attributes, reparse tag and file id, so no file is
//!   ever opened (and no cloud placeholder is ever hydrated). File systems
//!   that reject that query fall back to `FileFullDirectoryInformation`,
//!   then to `FindFirstFileExW`.
//! * Directories below the root are opened with `FILE_OPEN_REPARSE_POINT`:
//!   a reparse point is opened as itself, never followed, so a junction
//!   created between listing a parent and opening the child cannot send the
//!   walk elsewhere, and cloud folders list what is on disk without asking
//!   their provider for anything.
//! * Entries go into a worker-local [`EntryBatch`]. The batch is inserted
//!   with one [`Tree::add_children`](crate::Tree::add_children) call under
//!   the tree's write lock, which is never held during a system call. Once
//!   the tree compresses names, workers encode them before taking the lock. Huge
//!   directories are inserted in slices of 4096 entries to bound the
//!   batch's memory. The returned id range names the new child
//!   directories, which become new tasks.
//! * Directories behind name-surrogate reparse points (junctions, symbolic
//!   links, mount points) are flagged [`NodeFlags::REPARSE`] and never
//!   entered; other reparse points (OneDrive and other cloud folders) are.
//! * Hard links are counted once: on local NTFS through a bitmap over MFT
//!   record numbers (~0.5 MiB for 4 million records), elsewhere through a
//!   sharded hash set of file ids. Both live only for the scan.
//! * A directory that cannot be opened or listed is flagged
//!   [`NodeFlags::INACCESSIBLE`] and the walk goes on.
//!
//! All scan state (pool threads, buffers, the hard-link set) is dropped
//! before [`scan`] returns.

mod links;
mod nt;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, PoisonError, RwLockWriteGuard};
use std::time::Instant;

use super::{ScanMode, ScanOptions, ScanProgress, ScanSummary};
use crate::tree::Encoder;
use crate::{EntryBatch, Error, NodeFlags, NodeId, Result, SharedTree, Tree};

use links::LinkSet;
use nt::{Entry, Level, ListError, Open};

/// Size of each worker's `NtQueryDirectoryFile` buffer.
const QUERY_BUFFER_BYTES: usize = 64 * 1024;
/// A directory's entries are inserted into the tree at least every this
/// many entries, which bounds the per-worker batch memory.
const FLUSH_ENTRIES: usize = 4096;

/// Walks `root` in parallel and inserts every entry into `tree`, whose
/// root node must have no children yet.
pub fn scan(
    root: &Path,
    opts: &ScanOptions,
    tree: &SharedTree,
    progress: &ScanProgress,
) -> Result<ScanSummary> {
    scan_into(tree, NodeId::ROOT, root, opts, progress)
}

/// Walks the directory at `path` and inserts its contents below the
/// existing directory node `dir`, which must have no live children.
/// Used for the initial scan (with the root) and by the watcher to fill in
/// a newly created or moved-in directory.
///
/// `path` itself is followed even if it is a junction or symbolic link;
/// links below it are not.
///
/// Errors: [`Error::Io`] if `path` does not exist or is not a directory,
/// [`Error::Cancelled`] if `progress` was cancelled (the tree then holds
/// whatever was inserted so far). If `path` exists but cannot be listed
/// (e.g. access denied), `dir` is flagged [`NodeFlags::INACCESSIBLE`] and
/// the scan succeeds with `inaccessible == 1`, as for any directory below.
///
/// Hard links are deduplicated within this call only: a link to a file
/// that an earlier scan already counted elsewhere in the tree is counted
/// again.
pub fn scan_into(
    tree: &SharedTree,
    dir: NodeId,
    path: &Path,
    opts: &ScanOptions,
    progress: &ScanProgress,
) -> Result<ScanSummary> {
    walk(tree, dir, path, opts, progress, Tweaks::default()).map(|(summary, _)| summary)
}

/// Default worker count: directory listing waits on the disk as much as it
/// computes, so twice the logical CPUs, within [4, 16].
///
/// More than 16 does not help: Windows opens and closes directories at a
/// rate that stops growing there (measured on a 16-core machine with an
/// NVMe drive: about 40,000 directories a second from 16 threads up to 256,
/// with a warm or an empty file cache alike), and the extra threads only
/// queue up inside the kernel. 16 threads were 7% faster than 32 there and
/// spent half the CPU time.
pub fn default_threads() -> usize {
    let cpus = std::thread::available_parallelism().map_or(4, |n| n.get());
    (cpus * 2).clamp(4, 16)
}

/// How hard links are detected (see [`links`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkMode {
    /// The file system has no hard links (or does not say).
    Off,
    /// Bitmap over NTFS MFT record numbers.
    Records,
    /// Hash set of 64-bit file ids.
    Ids,
}

/// Overrides for tests: force a query level or a hard-link mode.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Tweaks {
    pub level: Level,
    pub links: Option<LinkMode>,
}

/// Internals reported by a walk, for tests and benchmarks.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct WalkStats {
    /// Threads used (1: everything ran on the calling thread).
    pub threads: usize,
    /// Heap bytes held by the hard-link set at the end of the walk.
    pub link_set_bytes: usize,
    pub link_mode: LinkMode,
    /// The query level in use when the walk ended.
    pub level: Level,
}

fn walk(
    tree: &SharedTree,
    dir: NodeId,
    path: &Path,
    opts: &ScanOptions,
    progress: &ScanProgress,
    tweaks: Tweaks,
) -> Result<(ScanSummary, WalkStats)> {
    let start = Instant::now();
    let root = nt::root_path(path)?;
    let caps = nt::volume_caps(&root);
    let link_mode = tweaks.links.unwrap_or(match &caps {
        // File ids on local NTFS are MFT file references.
        Some(c)
            if c.supports_hard_links && c.file_system.eq_ignore_ascii_case("NTFS") && !c.remote =>
        {
            LinkMode::Records
        }
        Some(c) if c.supports_hard_links => LinkMode::Ids,
        _ => LinkMode::Off,
    });
    let threads = match opts.threads {
        0 => default_threads(),
        n => n,
    };
    let cluster = caps.as_ref().map_or(4096, |c| c.cluster);
    // only a tree not yet filled takes it (not one the watcher adds to)
    tree.write()
        .unwrap_or_else(PoisonError::into_inner)
        .set_cluster_size(cluster);
    let ctx = Ctx::new(tree, progress, link_mode, tweaks.level, cluster, threads);
    let caller = threads;
    let win32_root = nt::win32_root(&root);

    // The calling thread lists the root itself: a leaf directory (the
    // common case when the watcher fills in a new folder) needs no pool.
    let mut pending: Vec<Job> = Vec::new();
    ctx.process(
        Job {
            node: dir,
            path: root.into_boxed_slice(),
            open: Open::Follow,
        },
        caller,
        &mut |job| pending.push(job),
    )?;

    let mut used_threads = 1;
    if !pending.is_empty() {
        let pool = if threads > 1 {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|i| format!("tessera-walk-{i}"))
                .build()
                .ok()
        } else {
            None
        };
        match pool {
            Some(pool) => {
                used_threads = threads;
                let ctx = &ctx;
                pool.scope(|s| {
                    for job in pending.drain(..) {
                        spawn(s, ctx, job);
                    }
                });
                // Dropping the pool lets its (idle) threads exit.
            }
            None => {
                // Depth first on the calling thread.
                while let Some(job) = pending.pop() {
                    ctx.process(job, caller, &mut |child| pending.push(child))?;
                }
            }
        }
    }

    let stats = WalkStats {
        threads: used_threads,
        link_set_bytes: ctx.links.heap_bytes(),
        link_mode,
        level: Level::from_u8(ctx.level.load(Relaxed)),
    };
    let cancelled = ctx.cancelled.load(Relaxed) || progress.is_cancelled();
    let summary = ScanSummary {
        mode: ScanMode::Walk,
        elapsed: start.elapsed(),
        files: ctx.files.load(Relaxed),
        dirs: ctx.dirs.load(Relaxed),
        inaccessible: ctx.inaccessible.load(Relaxed),
        ..ScanSummary::default()
    };
    // Free every buffer and the hard-link set before the last query.
    drop(ctx);
    if cancelled {
        return Err(Error::Cancelled);
    }
    // The verbatim spelling works beyond MAX_PATH.
    let volume = super::volume_info(path)
        .or_else(|_| super::volume_info(&win32_root))
        .ok();
    Ok((ScanSummary { volume, ..summary }, stats))
}

fn spawn<'s>(s: &rayon::Scope<'s>, ctx: &'s Ctx<'_>, job: Job) {
    s.spawn(move |s| {
        let slot = rayon::current_thread_index().unwrap_or(ctx.scratch.len() - 1);
        // Only the scan root can fail, and it is never spawned.
        let _ = ctx.process(job, slot, &mut |child| spawn(s, ctx, child));
    });
}

/// One directory to list.
struct Job {
    node: NodeId,
    /// NT path, no terminating NUL.
    path: Box<[u16]>,
    open: Open,
}

/// State shared by all workers of one walk.
struct Ctx<'a> {
    tree: &'a SharedTree,
    progress: &'a ScanProgress,
    links: LinkSet,
    /// Current [`Level`]; only ever moves down to more basic queries.
    level: AtomicU8,
    cluster: u64,
    files: AtomicU64,
    dirs: AtomicU64,
    inaccessible: AtomicU64,
    cancelled: AtomicBool,
    /// Per-worker buffers, indexed by rayon thread index (the last slot is
    /// the calling thread's). Each slot is only ever locked by its own
    /// thread; the mutex just makes that provable.
    scratch: Box<[Mutex<Option<Box<Scratch>>>]>,
}

/// A worker's reusable buffers.
struct Scratch {
    /// `NtQueryDirectoryFile` output (u64 for alignment).
    query: Vec<u64>,
    pending: Pending,
}

/// Entries listed but not yet inserted, and the subdirectories among them.
#[derive(Default)]
struct Pending {
    batch: EntryBatch,
    /// (index in `batch`, range in `names`, open mode) per subdirectory.
    subdirs: Vec<(u32, std::ops::Range<usize>, Open)>,
    /// UTF-16 names of the subdirectories, to build their paths.
    names: Vec<u16>,
    files: u64,
    dirs: u64,
    bytes: u64,
    /// The tree's name encoder once it has one: names are encoded here,
    /// outside the tree's lock.
    encoder: Option<Encoder>,
}

impl Scratch {
    fn new() -> Box<Scratch> {
        Box::new(Scratch {
            query: vec![0u64; QUERY_BUFFER_BYTES / 8],
            pending: Pending::default(),
        })
    }
}

impl<'a> Ctx<'a> {
    /// `workers` pool threads; their scratch slots are 0..workers and the
    /// calling thread's is `workers`.
    fn new(
        tree: &'a SharedTree,
        progress: &'a ScanProgress,
        link_mode: LinkMode,
        level: Level,
        cluster: u64,
        workers: usize,
    ) -> Self {
        Ctx {
            tree,
            progress,
            links: match link_mode {
                LinkMode::Off => LinkSet::Off,
                LinkMode::Records => LinkSet::records(),
                LinkMode::Ids => LinkSet::ids(),
            },
            level: AtomicU8::new(level as u8),
            cluster: cluster.max(1),
            files: AtomicU64::new(0),
            dirs: AtomicU64::new(0),
            inaccessible: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            scratch: (0..=workers).map(|_| Mutex::new(None)).collect(),
        }
    }

    fn write_tree(&self) -> RwLockWriteGuard<'_, Tree> {
        self.tree.write().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_cancelled(&self) -> bool {
        if self.progress.is_cancelled() {
            self.cancelled.store(true, Relaxed);
        }
        self.cancelled.load(Relaxed)
    }

    fn add_flags(&self, node: NodeId, flags: NodeFlags) {
        let mut tree = self.write_tree();
        let f = tree.node(node).flags | flags;
        tree.set_flags(node, f);
    }

    fn mark_inaccessible(&self, node: NodeId) {
        self.add_flags(node, NodeFlags::INACCESSIBLE);
        self.inaccessible.fetch_add(1, Relaxed);
        self.progress.inaccessible.fetch_add(1, Relaxed);
    }

    /// Lists one directory, inserts its entries and hands its
    /// subdirectories to `spawn`. Fails only for a scan root that does not
    /// exist or is not a directory.
    fn process(&self, job: Job, slot: usize, spawn: &mut dyn FnMut(Job)) -> Result<()> {
        if self.is_cancelled() {
            return Ok(());
        }
        let mut guard = self.scratch[slot]
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let scratch = guard.get_or_insert_with(Scratch::new);
        self.list_dir(&job, scratch, spawn)
    }

    fn list_dir(&self, job: &Job, scratch: &mut Scratch, spawn: &mut dyn FnMut(Job)) -> Result<()> {
        let Scratch { query, pending } = scratch;
        let mut level = Level::from_u8(self.level.load(Relaxed));
        let open = job.open;
        // In FindFirstFileExW mode only a reparse check needs a handle.
        let handle = if level < Level::Find || open == Open::NoFollowReadTag {
            match nt::open_dir(&job.path, open) {
                Ok(h) => Some(h),
                Err(code) => return self.failed(job, code),
            }
        } else {
            None
        };
        if open == Open::NoFollowReadTag
            && let Some(h) = &handle
            && let Ok(tag) = nt::reparse_tag(h)
            && nt::is_name_surrogate(tag)
        {
            // Its parent's listing did not say what kind of reparse point
            // this is; now we know it is a link: don't enter it.
            self.add_flags(job.node, NodeFlags::REPARSE);
            return Ok(());
        }

        let cancelled = || self.is_cancelled();
        let mut sink = Sink {
            ctx: self,
            job,
            pending,
            spawn,
        };
        let result = loop {
            match (level, &handle) {
                (Level::IdFull | Level::Full, Some(h)) => {
                    match nt::list(h, level, query, &cancelled, &mut |e| sink.push(e)) {
                        Ok(()) => break Ok(()),
                        Err(ListError::Failed(code)) => break Err(code),
                        Err(ListError::Unsupported) => {
                            level = level.next();
                            self.level.fetch_max(level as u8, Relaxed);
                        }
                    }
                }
                _ => break nt::list_find(&job.path, &cancelled, &mut |e| sink.push(e)),
            }
        };
        sink.flush();
        match result {
            Ok(()) => Ok(()),
            Err(code) => self.failed(job, code),
        }
    }

    /// Handles a directory that could not be opened or listed.
    fn failed(&self, job: &Job, code: u32) -> Result<()> {
        if job.open == Open::Follow
            && matches!(
                code,
                nt::ERROR_FILE_NOT_FOUND
                    | nt::ERROR_PATH_NOT_FOUND
                    | nt::ERROR_DIRECTORY
                    | nt::ERROR_INVALID_NAME
                    | nt::ERROR_BAD_NETPATH
                    | nt::ERROR_BAD_NET_NAME
            )
        {
            return Err(Error::Io(nt::io_error(code)));
        }
        self.mark_inaccessible(job.node);
        Ok(())
    }
}

/// Turns listed entries into tree entries for one directory.
struct Sink<'c, 'j, 'p, 's> {
    ctx: &'c Ctx<'c>,
    job: &'j Job,
    pending: &'p mut Pending,
    spawn: &'s mut dyn FnMut(Job),
}

impl Sink<'_, '_, '_, '_> {
    fn push(&mut self, e: &Entry<'_>) {
        const DOT: u16 = b'.' as u16;
        if e.name == [DOT] || e.name == [DOT, DOT] || e.name.is_empty() {
            return;
        }
        let a = e.attributes;
        let is_dir = a & nt::FILE_ATTRIBUTE_DIRECTORY != 0;
        let mut flags = NodeFlags::NONE;
        if is_dir {
            flags |= NodeFlags::DIR;
        }
        if a & nt::FILE_ATTRIBUTE_HIDDEN != 0 {
            flags |= NodeFlags::HIDDEN;
        }
        if a & nt::FILE_ATTRIBUTE_SYSTEM != 0 {
            flags |= NodeFlags::SYSTEM;
        }
        if a & (nt::FILE_ATTRIBUTE_COMPRESSED | nt::FILE_ATTRIBUTE_SPARSE_FILE) != 0 {
            flags |= NodeFlags::COMPRESSED;
        }
        let cloud = a
            & (nt::FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
                | nt::FILE_ATTRIBUTE_RECALL_ON_OPEN
                | nt::FILE_ATTRIBUTE_OFFLINE)
            != 0;
        if cloud {
            flags |= NodeFlags::CLOUD;
        }
        let mut descend = is_dir.then_some(Open::NoFollow);
        if a & nt::FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            match e.reparse_tag {
                // The file system did not report the tag: the child's own
                // task reads it before entering.
                0 => descend = descend.map(|_| Open::NoFollowReadTag),
                tag if nt::is_name_surrogate(tag) => {
                    flags |= NodeFlags::REPARSE;
                    descend = None;
                }
                nt::IO_REPARSE_TAG_WOF => flags |= NodeFlags::COMPRESSED,
                _ => {}
            }
        }

        let (mut size, mut logical) = if is_dir {
            (0, 0)
        } else {
            let allocated = e.allocated.unwrap_or_else(|| {
                // FindFirstFileExW reports no allocation: estimate it.
                if cloud {
                    0
                } else {
                    e.logical.div_ceil(self.ctx.cluster) * self.ctx.cluster
                }
            });
            (allocated, e.logical)
        };
        if !is_dir
            && !self
                .ctx
                .links
                .first_sighting(e.file_id, size == 0 && logical == 0)
        {
            flags |= NodeFlags::HARDLINK;
            size = 0;
            logical = 0;
        }

        let p = &mut *self.pending;
        if let Some(open) = descend {
            let start = p.names.len();
            p.names.extend_from_slice(e.name);
            p.subdirs
                .push((p.batch.len() as u32, start..p.names.len(), open));
        }
        p.batch.push_utf16(e.name, size, logical, flags);
        if is_dir {
            p.dirs += 1;
        } else {
            p.files += 1;
        }
        p.bytes += size;
        if p.batch.len() >= FLUSH_ENTRIES {
            self.flush();
        }
    }

    /// Inserts the pending entries and spawns their subdirectories.
    fn flush(&mut self) {
        let p = &mut *self.pending;
        if p.batch.is_empty() {
            return;
        }
        if let Some(enc) = &p.encoder {
            p.batch.encode_names(enc);
        }
        let ids = {
            let mut tree = self.ctx.write_tree();
            let ids = tree.add_children(self.job.node, &p.batch);
            if p.encoder.is_none() {
                p.encoder = tree.name_encoder();
            }
            ids
        };
        let ctx = self.ctx;
        ctx.files.fetch_add(p.files, Relaxed);
        ctx.dirs.fetch_add(p.dirs, Relaxed);
        ctx.progress.files.fetch_add(p.files, Relaxed);
        ctx.progress.dirs.fetch_add(p.dirs, Relaxed);
        ctx.progress.bytes.fetch_add(p.bytes, Relaxed);
        for (index, name, open) in p.subdirs.drain(..) {
            (self.spawn)(Job {
                node: NodeId(ids.start + index),
                path: nt::child_path(&self.job.path, &p.names[name]),
                open,
            });
        }
        p.batch.clear();
        p.names.clear();
        p.files = 0;
        p.dirs = 0;
        p.bytes = 0;
    }
}
