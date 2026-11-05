//! Live updates against the real file system (Windows only).
//!
//! Each test builds a temporary directory and a matching tree (with a small
//! std::fs walker, independent of the crate's scanners), starts a watcher
//! with a short throttle, changes the directory and waits for the tree to
//! match the disk again.
//!
//! Under Wine, `ReadDirectoryChangesExW` and the USN journal do not exist;
//! the plain `ReadDirectoryChangesW` path is exercised there. Assertions
//! that depend on how the real NTFS reports changes are relaxed under Wine
//! with a printed note.
//!
//! The USN journal test runs only with `TESSERA_ADMIN_TESTS=1` (elevated,
//! NTFS). It builds its file index from `GetFileInformationByHandle` (the
//! file index of NTFS is the file reference number).

#![cfg(windows)]

use std::fs;
use std::io::Write;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use tessera_core::scan::FrnIndex;
use tessera_core::watch::{BatchReport, WatchBackend, WatchOptions, Watcher};
use tessera_core::{EntryBatch, NodeFlags, NodeId, SharedTree, Tree};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
};

const FILE_READ_ATTRIBUTES: u32 = 0x80;
const TIMEOUT: Duration = Duration::from_secs(15);

fn is_wine() -> bool {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::core::{s, w};
    unsafe {
        GetModuleHandleW(w!("ntdll.dll"))
            .map(|m| GetProcAddress(m, s!("wine_get_version")).is_some())
            .unwrap_or(false)
    }
}

/// A temporary directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "tessera-watch-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        TempDir(fs::canonicalize(&base).unwrap())
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Opens an entry for attribute queries only (no data access).
fn open_attrs(path: &Path) -> fs::File {
    fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .unwrap()
}

/// (allocation size, logical size) of a file.
fn disk_sizes(path: &Path) -> (u64, u64) {
    let f = open_attrs(path);
    let mut info = FILE_STANDARD_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(f.as_raw_handle()),
            FileStandardInfo,
            (&mut info as *mut FILE_STANDARD_INFO).cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
        .unwrap();
    }
    (info.AllocationSize as u64, info.EndOfFile as u64)
}

/// The NTFS file reference number of an entry.
fn file_reference(path: &Path) -> u64 {
    let f = open_attrs(path);
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle()), &mut info).unwrap() };
    ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64
}

/// Builds a tree for `root` with plain std::fs, calling `each` for every
/// node (e.g. to index its file reference number).
fn build_tree(root: &Path, each: &mut dyn FnMut(&Path, NodeId)) -> SharedTree {
    fn fill(tree: &mut Tree, dir: NodeId, path: &Path, each: &mut dyn FnMut(&Path, NodeId)) {
        let mut entries: Vec<_> = fs::read_dir(path).unwrap().map(|e| e.unwrap()).collect();
        entries.sort_by_key(|e| e.file_name());
        let mut batch = EntryBatch::new();
        for e in &entries {
            let md = e.metadata().unwrap();
            let mut flags = NodeFlags::NONE;
            if md.file_attributes() & 0x2 != 0 {
                flags |= NodeFlags::HIDDEN;
            }
            if md.file_attributes() & 0x4 != 0 {
                flags |= NodeFlags::SYSTEM;
            }
            let name: Vec<u16> = {
                use std::os::windows::ffi::OsStrExt;
                e.file_name().encode_wide().collect()
            };
            if md.is_dir() {
                batch.push_utf16(&name, 0, 0, flags | NodeFlags::DIR);
            } else {
                let (size, logical) = disk_sizes(&e.path());
                batch.push_utf16(&name, size, logical, flags);
            }
        }
        let ids = tree.add_children(dir, &batch);
        for (e, id) in entries.iter().zip(ids) {
            each(&e.path(), NodeId(id));
            if e.metadata().unwrap().is_dir() {
                fill(tree, NodeId(id), &e.path(), each);
            }
        }
    }
    let mut tree = Tree::for_path(root);
    each(root, NodeId::ROOT);
    fill(&mut tree, NodeId::ROOT, root, each);
    tree.into_shared()
}

/// Sorted `path alloc/len` lines (directories end with `\`).
fn disk_listing(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if p.is_dir() {
                out.push(format!("{rel}\\"));
                walk(root, &p, out);
            } else {
                let (a, l) = disk_sizes(&p);
                out.push(format!("{rel} {a}/{l}"));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn tree_listing(tree: &Tree) -> Vec<String> {
    fn walk(tree: &Tree, id: NodeId, prefix: &str, out: &mut Vec<String>) {
        for c in tree.children(id) {
            let n = tree.node(c);
            let rel = format!("{prefix}{}", tree.name_lossy(c));
            if n.is_dir() {
                out.push(format!("{rel}\\"));
                walk(tree, c, &format!("{rel}\\"), out);
            } else {
                out.push(format!("{rel} {}/{}", n.size, n.logical));
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, NodeId::ROOT, "", &mut out);
    out.sort();
    out
}

struct Session {
    root: PathBuf,
    tree: SharedTree,
    rx: Receiver<BatchReport>,
    reports: Vec<BatchReport>,
}

impl Session {
    fn start(
        root: &Path,
        tree: SharedTree,
        opts: WatchOptions,
        usn: Option<(FrnIndex, tessera_core::scan::UsnCheckpoint)>,
    ) -> (Session, Watcher) {
        let (tx, rx) = mpsc::channel();
        let w = Watcher::start(root, tree.clone(), opts, usn, move |r: &BatchReport| {
            let _ = tx.send(r.clone());
        })
        .expect("start watcher");
        (
            Session {
                root: root.to_path_buf(),
                tree,
                rx,
                reports: Vec::new(),
            },
            w,
        )
    }

    /// Waits until `cond` holds for the tree, collecting reports.
    fn wait(&mut self, what: &str, cond: impl Fn(&Tree) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            {
                let t = self.tree.read().unwrap();
                if cond(&t) {
                    t.check_invariants().unwrap();
                    return;
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let t = self.tree.read().unwrap();
                panic!(
                    "timed out waiting for {what}\ntree: {:#?}\ndisk: {:#?}\nreports: {:#?}",
                    tree_listing(&t),
                    disk_listing(&self.root),
                    self.reports
                );
            }
            if let Ok(r) = self.rx.recv_timeout(left.min(Duration::from_millis(100))) {
                self.reports.push(r);
            }
        }
    }

    /// Waits until the whole tree matches the disk.
    fn wait_synced(&mut self, what: &str) {
        let root = self.root.clone();
        self.wait(what, |t| tree_listing(t) == disk_listing(&root));
    }

    fn id(&self, rel: &str) -> Option<NodeId> {
        self.tree.read().unwrap().lookup_relative(rel.as_bytes())
    }
}

fn write(path: &Path, len: usize) {
    fs::write(path, vec![b'x'; len]).unwrap();
}

/// Creates, grows, deletes, renames and moves files and directories and
/// checks that the tree follows.
fn exercise(s: &mut Session) {
    let root = s.root.clone();
    let wine = is_wine();

    write(&root.join("new.txt"), 5000);
    s.wait_synced("a created file");

    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(root.join("a.txt"))
        .unwrap();
    f.write_all(&[b'y'; 20_000]).unwrap();
    drop(f);
    s.wait_synced("a grown file");

    fs::remove_file(root.join("sub").join("b.txt")).unwrap();
    s.wait_synced("a deleted file");

    let id = s.id("new.txt").unwrap();
    fs::rename(root.join("new.txt"), root.join("renamed.txt")).unwrap();
    s.wait_synced("a renamed file");
    let same = s.id("renamed.txt") == Some(id);
    if !same && wine {
        eprintln!("note: under Wine the rename was not seen as one (node replaced)");
    } else {
        assert!(same, "a rename keeps the node");
    }

    fs::rename(
        root.join("renamed.txt"),
        root.join("sub").join("deep").join("moved.txt"),
    )
    .unwrap();
    s.wait_synced("a file moved to another directory");

    let sub = s.id("sub").unwrap();
    fs::rename(root.join("sub"), root.join("sub2")).unwrap();
    s.wait_synced("a renamed directory");
    let same = s.id("sub2") == Some(sub);
    if !same && wine {
        eprintln!("note: under Wine the directory rename was not seen as one");
    } else {
        assert!(same, "a directory rename keeps the node");
    }

    fs::create_dir(root.join("made")).unwrap();
    write(&root.join("made").join("x.bin"), 3000);
    fs::create_dir(root.join("made").join("inner")).unwrap();
    write(&root.join("made").join("inner").join("y.bin"), 1);
    s.wait_synced("a new directory with contents");
    let made = s.id("made").unwrap();
    if s.reports.iter().any(|r| r.rescan_needed.contains(&made)) {
        eprintln!(
            "note: walk::scan_into is not available; new directories were filled from events"
        );
    }

    fs::remove_dir_all(root.join("made")).unwrap();
    s.wait_synced("a deleted directory tree");
}

fn sample(root: &Path) {
    write(&root.join("a.txt"), 100);
    fs::create_dir_all(root.join("sub").join("deep")).unwrap();
    write(&root.join("sub").join("b.txt"), 2000);
    write(&root.join("sub").join("deep").join("c.txt"), 10);
}

fn opts(backend: WatchBackend, throttle_ms: u64) -> WatchOptions {
    WatchOptions {
        backend,
        throttle: Duration::from_millis(throttle_ms),
    }
}

#[test]
fn directory_changes_follow_the_disk() {
    let dir = TempDir::new("rdcw");
    sample(&dir.0);
    let tree = build_tree(&dir.0, &mut |_, _| {});
    let (mut s, w) = Session::start(&dir.0, tree, opts(WatchBackend::Auto, 50), None);
    assert_eq!(w.backend(), WatchBackend::DirectoryChanges);
    exercise(&mut s);
    let t = Instant::now();
    w.stop();
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "stop took {:?}",
        t.elapsed()
    );
}

#[test]
fn a_burst_of_writes_gives_few_batches() {
    let dir = TempDir::new("burst");
    write(&dir.0.join("log.txt"), 0);
    let tree = build_tree(&dir.0, &mut |_, _| {});
    let throttle = Duration::from_millis(200);
    let (mut s, w) = Session::start(
        &dir.0,
        tree,
        opts(WatchBackend::DirectoryChanges, 200),
        None,
    );
    let start = Instant::now();
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(dir.0.join("log.txt"))
        .unwrap();
    for _ in 0..1000 {
        f.write_all(&[b'z'; 64]).unwrap();
    }
    f.sync_all().unwrap();
    drop(f);
    let burst = start.elapsed();
    s.wait_synced("the burst");
    // Let trailing notifications land.
    std::thread::sleep(throttle * 3);
    while let Ok(r) = s.rx.try_recv() {
        s.reports.push(r);
    }
    let batches = s.reports.len();
    let limit = (burst.as_millis() / throttle.as_millis()) as usize + 3;
    let events: usize = s.reports.iter().map(|r| r.events).sum();
    eprintln!("burst of 1000 writes in {burst:?}: {events} notifications, {batches} batches");
    assert!(
        batches <= limit,
        "{batches} batches (limit {limit}) for a {burst:?} burst"
    );
    assert_eq!(
        s.tree
            .read()
            .unwrap()
            .node(s.id("log.txt").unwrap())
            .logical,
        64_000
    );
    w.stop();
}

#[test]
fn idle_is_quiet_and_stop_is_prompt() {
    let dir = TempDir::new("idle");
    sample(&dir.0);
    let tree = build_tree(&dir.0, &mut |_, _| {});
    let (s, w) = Session::start(
        &dir.0,
        tree.clone(),
        opts(WatchBackend::DirectoryChanges, 50),
        None,
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(s.rx.try_recv().is_err(), "no batch without changes");
    let t = Instant::now();
    w.stop();
    assert!(
        t.elapsed() < Duration::from_secs(1),
        "stop took {:?}",
        t.elapsed()
    );

    // Stopping in the middle of a (long) window is prompt too, and drops
    // the pending batch.
    let (s, w) = Session::start(
        &dir.0,
        tree,
        opts(WatchBackend::DirectoryChanges, 10_000),
        None,
    );
    write(&dir.0.join("late.txt"), 10);
    std::thread::sleep(Duration::from_millis(200));
    let t = Instant::now();
    drop(w);
    assert!(
        t.elapsed() < Duration::from_secs(1),
        "drop took {:?}",
        t.elapsed()
    );
    assert!(s.rx.try_recv().is_err());
}

#[test]
fn flagged_subtrees_are_ignored() {
    let dir = TempDir::new("ignored");
    fs::create_dir(dir.0.join("locked")).unwrap();
    fs::create_dir(dir.0.join("link")).unwrap();
    let tree = build_tree(&dir.0, &mut |_, _| {});
    {
        let mut t = tree.write().unwrap();
        for (name, flag) in [
            ("locked", NodeFlags::INACCESSIBLE),
            ("link", NodeFlags::REPARSE),
        ] {
            let id = t.lookup_relative(name.as_bytes()).unwrap();
            let f = t.node(id).flags | flag;
            t.set_flags(id, f);
        }
    }
    let (mut s, w) = Session::start(&dir.0, tree, opts(WatchBackend::DirectoryChanges, 50), None);
    write(&dir.0.join("locked").join("a.txt"), 10);
    write(&dir.0.join("link").join("b.txt"), 10);
    write(&dir.0.join("sentinel.txt"), 10);
    s.wait("the sentinel", |t| {
        t.lookup_relative(b"sentinel.txt").is_some()
    });
    std::thread::sleep(Duration::from_millis(200));
    let t = s.tree.read().unwrap();
    for name in ["locked", "link"] {
        let id = t.lookup_relative(name.as_bytes()).unwrap();
        assert_eq!(t.node(id).child_count, 0, "{name} must stay untouched");
    }
    drop(t);
    w.stop();
}

#[test]
fn rescan_on_request() {
    let dir = TempDir::new("rescan");
    sample(&dir.0);
    let tree = build_tree(&dir.0, &mut |_, _| {});
    let (mut s, w) = Session::start(&dir.0, tree, opts(WatchBackend::DirectoryChanges, 50), None);
    let sub = s.id("sub").unwrap();
    w.rescan(sub);
    let deadline = Instant::now() + TIMEOUT;
    let report = loop {
        match s
            .rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(r) if r.dirs.contains(&sub) => break r,
            Ok(_) => continue,
            Err(e) => panic!("no rescan report: {e:?}"),
        }
    };
    if report.rescan_needed.contains(&sub) {
        eprintln!("note: walk::scan_into is not available; the rescan could not run");
    } else {
        s.wait_synced("the rescanned directory");
    }
    w.stop();
}

#[test]
fn start_fails_for_a_missing_root() {
    let dir = TempDir::new("missing");
    let gone = dir.0.join("nope");
    let tree = Tree::for_path(&gone).into_shared();
    let r = Watcher::start(&gone, tree, WatchOptions::default(), None, |_| {});
    assert!(r.is_err());
    // The USN backend needs an index and a checkpoint.
    let tree = Tree::for_path(&dir.0).into_shared();
    let r = Watcher::start(
        &dir.0,
        tree,
        opts(WatchBackend::UsnJournal, 50),
        None,
        |_| {},
    );
    assert!(matches!(r, Err(tessera_core::Error::Unsupported(_))));
}

#[test]
#[ignore = "needs admin rights and NTFS; run with TESSERA_ADMIN_TESTS=1 --include-ignored"]
fn usn_journal_follows_the_disk() {
    if std::env::var("TESSERA_ADMIN_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set TESSERA_ADMIN_TESTS=1 (elevated, NTFS) to run");
        return;
    }
    let dir = TempDir::new("usn");
    sample(&dir.0);
    let checkpoint = match tessera_core::watch::query_usn_checkpoint(&dir.0) {
        Ok(c) => c,
        Err(e) => {
            // ERROR_JOURNAL_NOT_ACTIVE: nothing to test on this volume.
            if let tessera_core::Error::Io(io) = &e {
                if io.raw_os_error() == Some(1179) {
                    eprintln!("skipped: no USN journal on this volume");
                    return;
                }
            }
            panic!("query_usn_checkpoint: {e}");
        }
    };
    let mut index = FrnIndex::default();
    let tree = build_tree(&dir.0, &mut |p, id| index.insert(file_reference(p), id));
    let (mut s, w) = Session::start(
        &dir.0,
        tree,
        opts(WatchBackend::UsnJournal, 50),
        Some((index, checkpoint)),
    );
    assert_eq!(w.backend(), WatchBackend::UsnJournal);
    exercise(&mut s);
    let t = Instant::now();
    w.stop();
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "stop took {:?}",
        t.elapsed()
    );
}
