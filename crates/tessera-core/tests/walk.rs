//! Windows integration tests for the parallel directory walk: build a
//! directory tree on disk, scan it, and compare against what was built and
//! against an independent `std::fs` walk.
//!
//! Under wine some Windows features are missing (ACLs are not enforced,
//! junctions may not exist); those checks are skipped with a printed note
//! there and must pass on real Windows.

#![cfg(windows)]

use std::collections::HashSet;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use tessera_core::scan::{ScanMode, ScanOptions, ScanProgress, ScanSummary, walk};
use tessera_core::{Error, NodeFlags, NodeId, SharedTree, Tree};

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
/// The Everyone group, so the deny entry applies whoever runs the tests.
const EVERYONE: &str = "*S-1-1-0";

fn is_wine() -> bool {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::core::{s, w};
    // SAFETY: plain lookups of a loaded module and an export name.
    unsafe {
        GetModuleHandleW(w!("ntdll.dll"))
            .ok()
            .and_then(|m| GetProcAddress(m, s!("wine_get_version")))
            .is_some()
    }
}

/// Whether the volume holding `path` reports hard-link support (wine's
/// emulated volumes do not, so the walk does not deduplicate there).
fn supports_hard_links(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{GetVolumeInformationW, GetVolumePathNameW};
    use windows::core::PCWSTR;
    const FILE_SUPPORTS_HARD_LINKS: u32 = 0x0040_0000;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut vol = [0u16; 1024];
    let mut flags = 0u32;
    // SAFETY: NUL-terminated input, correctly sized outputs.
    let ok = unsafe {
        GetVolumePathNameW(PCWSTR(wide.as_ptr()), &mut vol).is_ok()
            && GetVolumeInformationW(
                PCWSTR(vol.as_ptr()),
                None,
                None,
                None,
                Some(&mut flags),
                None,
            )
            .is_ok()
    };
    ok && flags & FILE_SUPPORTS_HARD_LINKS != 0
}

/// A fresh directory under %TEMP%, removed (ACLs restored first) on drop.
struct TestDir {
    path: PathBuf,
    denied: Vec<PathBuf>,
}

impl TestDir {
    fn new(tag: &str) -> TestDir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "tessera-walk-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        TestDir {
            path,
            denied: Vec::new(),
        }
    }

    /// Denies listing `rel` to everyone. Returns true if listing it now
    /// really fails (not under wine, which ignores ACLs).
    fn deny_listing(&mut self, rel: &str) -> bool {
        let dir = self.path.join(rel);
        let ok = Command::new("icacls")
            .arg(&dir)
            .args(["/deny", &format!("{EVERYONE}:(OI)(CI)(RD)")])
            .output()
            .is_ok_and(|o| o.status.success());
        if ok {
            self.denied.push(dir.clone());
        }
        ok && std::fs::read_dir(&dir).is_err()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        for dir in &self.denied {
            let _ = Command::new("icacls")
                .arg(dir)
                .args(["/remove:d", EVERYONE])
                .output();
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn write_file(path: &Path, len: usize) {
    std::fs::write(path, vec![0x5A_u8; len]).unwrap();
}

fn opts(threads: usize) -> ScanOptions {
    ScanOptions {
        mode: ScanMode::Walk,
        threads,
    }
}

fn scan_tree(root: &Path, threads: usize) -> (SharedTree, ScanSummary) {
    let tree = Tree::for_path(root).into_shared();
    let progress = ScanProgress::new();
    let summary = walk::scan(root, &opts(threads), &tree, &progress).expect("scan failed");
    let snap = progress.snapshot();
    assert_eq!(snap.files, summary.files);
    assert_eq!(snap.dirs, summary.dirs);
    assert_eq!(snap.inaccessible, summary.inaccessible);
    assert_eq!(snap.bytes, tree.read().unwrap().node(NodeId::ROOT).size());
    (tree, summary)
}

/// `rel` with `\` separators, as `Tree::lookup_relative` takes it.
fn node(tree: &Tree, rel: &str) -> NodeId {
    tree.lookup_relative(rel.replace('/', "\\").as_bytes())
        .unwrap_or_else(|| panic!("{rel} is missing from the scanned tree"))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Totals {
    files: u64,
    dirs: u64,
    logical: u64,
}

/// Volume serial and file index: the identity of a file across its links.
fn file_key(path: &Path) -> Option<(u32, u64)> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let f = std::fs::File::open(path).ok()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `f` stays open for the call; `info` is writable.
    unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle()), &mut info) }.ok()?;
    Some((
        info.dwVolumeSerialNumber,
        (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64,
    ))
}

/// An independent walk with `std::fs`: counts every entry, does not enter
/// links (std reports name-surrogate reparse points as symlinks), counts
/// the bytes of each hard-linked file once (if `seen` is given), skips
/// unreadable directories.
fn reference_walk(dir: &Path, seen: &mut Option<HashSet<(u32, u64)>>, t: &mut Totals) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries {
        let path = e.unwrap().path();
        let md = std::fs::symlink_metadata(&path).unwrap();
        if md.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0 {
            t.dirs += 1;
            if !md.file_type().is_symlink() {
                reference_walk(&path, seen, t);
            }
        } else {
            t.files += 1;
            let first = match (seen.as_mut(), file_key(&path)) {
                (Some(seen), Some(key)) => seen.insert(key),
                _ => true,
            };
            if first {
                t.logical += md.len();
            }
        }
    }
}

fn reference_totals(root: &Path) -> Totals {
    let mut t = Totals::default();
    let mut seen = supports_hard_links(root).then(HashSet::new);
    reference_walk(root, &mut seen, &mut t);
    t
}

#[test]
fn walk_matches_what_was_built() {
    let wine = is_wine();
    let mut dir = TestDir::new("main");
    let root = dir.path.clone();

    write_file(&root.join("a.txt"), 1000);
    write_file(&root.join("empty.bin"), 0);
    std::fs::create_dir_all(root.join("sub1/deep/deeper")).unwrap();
    write_file(&root.join("sub1/b.bin"), 5000);
    write_file(&root.join("sub1/deep/deeper/c.dat"), 123_456);
    std::fs::create_dir(root.join("empty dir")).unwrap();

    let uni_dir = "日本語フォルダ";
    let uni_file = "ünïcødé 📁 файл.txt";
    let unicode = std::fs::create_dir(root.join(uni_dir)).is_ok();
    if unicode {
        write_file(&root.join(uni_dir).join(uni_file), 777);
    }

    // A path well beyond MAX_PATH (260).
    let mut long = root.join("long");
    let mut long_rel = String::from("long");
    for i in 0..6 {
        let part = format!("{i}-{}", "x".repeat(60));
        long.push(&part);
        long_rel = format!("{long_rel}\\{part}");
    }
    std::fs::create_dir_all(&long).unwrap();
    let long_file = long.join("deep-file.bin");
    write_file(&long_file, 4242);
    long_rel.push_str("\\deep-file.bin");
    assert!(long_file.as_os_str().len() > 300);

    // Two links to one file.
    std::fs::create_dir(root.join("links")).unwrap();
    write_file(&root.join("links/original.bin"), 20_000);
    std::fs::hard_link(
        root.join("links/original.bin"),
        root.join("links/second-link.bin"),
    )
    .unwrap();

    // A junction to a directory that is also scanned directly.
    std::fs::create_dir(root.join("target")).unwrap();
    write_file(&root.join("target/t.txt"), 3333);
    let junction = Command::new("cmd")
        .args(["/C", "mklink", "/J", "junction", "target"])
        .current_dir(&root)
        .output()
        .is_ok_and(|o| o.status.success())
        && std::fs::symlink_metadata(root.join("junction"))
            .is_ok_and(|m| m.file_type().is_symlink());

    // A file name that is not valid UTF-16 (lone surrogate).
    let odd_name = OsString::from_wide(&[0xD800, b'x' as u16, b'.' as u16, b't' as u16]);
    let odd = std::fs::write(root.join(&odd_name), [1u8; 10]).is_ok()
        && std::fs::read_dir(&root)
            .unwrap()
            .any(|e| e.unwrap().file_name() == odd_name);

    // A directory nobody may list.
    std::fs::create_dir(root.join("denied")).unwrap();
    write_file(&root.join("denied/secret.txt"), 999);
    let denied = dir.deny_listing("denied");

    let (tree, summary) = scan_tree(&root, 4);
    let t = tree.read().unwrap();
    t.check_invariants().unwrap();
    assert_eq!(summary.mode, ScanMode::Walk);
    assert!(summary.volume.is_some());

    let logical = |rel: &str| t.node(node(&t, rel)).logical();
    assert_eq!(logical("a.txt"), 1000);
    assert_eq!(logical("empty.bin"), 0);
    assert_eq!(logical("sub1/b.bin"), 5000);
    assert_eq!(logical("sub1/deep/deeper/c.dat"), 123_456);
    assert_eq!(logical("sub1"), 128_456);
    assert!(t.node(node(&t, "sub1/deep")).is_dir());
    assert!(t.node(node(&t, "empty dir")).is_dir());
    assert_eq!(t.child_count(node(&t, "empty dir")), 0);
    assert!(!t.node(node(&t, "a.txt")).is_dir());
    assert!(t.node(node(&t, "a.txt")).size() >= 1000 || wine);

    // Unicode names survive exactly.
    if unicode {
        let uni = node(&t, &format!("{uni_dir}/{uni_file}"));
        assert_eq!(t.name(uni), uni_file.as_bytes());
        assert_eq!(t.node(uni).logical(), 777);
        assert_eq!(t.path(uni), root.join(uni_dir).join(uni_file));
    } else {
        assert!(wine, "cannot create a Unicode file name on Windows");
        println!("note: wine needs a UTF-8 locale (LANG=C.UTF-8) for Unicode names; check skipped");
    }

    let long_id = node(&t, &long_rel);
    assert_eq!(t.node(long_id).logical(), 4242);
    assert_eq!(t.path(long_id), long_file);

    // The hard-linked file is counted once.
    let a = t.node(node(&t, "links/original.bin"));
    let b = t.node(node(&t, "links/second-link.bin"));
    if !supports_hard_links(&root) {
        assert!(wine, "NTFS reports hard-link support on Windows");
        println!("note: volume reports no hard links (wine); dedup check skipped");
        assert_eq!((a.logical(), b.logical()), (20_000, 20_000));
    } else {
        assert_eq!(
            a.flags.contains(NodeFlags::HARDLINK) as u32
                + b.flags.contains(NodeFlags::HARDLINK) as u32,
            1,
            "exactly one link must be flagged: {:?} / {:?}",
            a.flags,
            b.flags
        );
        let (counted, extra) = if a.flags.contains(NodeFlags::HARDLINK) {
            (b, a)
        } else {
            (a, b)
        };
        assert_eq!(counted.logical(), 20_000);
        assert_eq!((extra.size(), extra.logical()), (0, 0));
        assert_eq!(logical("links"), 20_000);
    }

    assert_eq!(logical("target/t.txt"), 3333);
    if junction {
        let j = t.node(node(&t, "junction"));
        assert!(
            j.flags.contains(NodeFlags::REPARSE | NodeFlags::DIR),
            "{:?}",
            j.flags
        );
        assert_eq!(
            t.child_count(node(&t, "junction")),
            0,
            "a junction must not be followed"
        );
        assert_eq!(j.logical(), 0);
    } else {
        assert!(wine, "mklink /J failed on Windows");
        println!("note: no junction support here (wine); junction check skipped");
    }

    if odd {
        let id = t
            .children(NodeId::ROOT)
            .find(|&c| t.path(c).file_name() == Some(odd_name.as_os_str()))
            .expect("ill-formed UTF-16 name lost");
        assert_eq!(t.node(id).logical(), 10);
    } else {
        println!("note: cannot create a file with an ill-formed UTF-16 name here; check skipped");
    }

    let d = t.node(node(&t, "denied"));
    if denied {
        assert!(d.flags.contains(NodeFlags::INACCESSIBLE), "{:?}", d.flags);
        assert_eq!(t.child_count(node(&t, "denied")), 0);
        assert_eq!(summary.inaccessible, 1);
    } else {
        assert!(wine, "icacls /deny had no effect on Windows");
        println!("note: ACLs are not enforced here (wine); inaccessible check skipped");
        assert!(!d.flags.contains(NodeFlags::INACCESSIBLE));
        assert_eq!(summary.inaccessible, 0);
    }

    // Totals agree with an independent std::fs walk.
    let reference = reference_totals(&root);
    assert_eq!(
        Totals {
            files: summary.files,
            dirs: summary.dirs,
            logical: t.node(NodeId::ROOT).logical(),
        },
        reference
    );
    assert_eq!(t.len() as u64 - 1, reference.files + reference.dirs);
}

/// Sorted (path, is_dir) of every node, plus the root's totals.
fn shape(tree: &Tree) -> (Vec<(String, bool)>, u64, u64) {
    let mut v: Vec<(String, bool)> = (1..tree.len() as u32)
        .map(|i| (tree.path_lossy(NodeId(i)), tree.node(NodeId(i)).is_dir()))
        .collect();
    v.sort();
    let r = tree.node(NodeId::ROOT);
    (v, r.size(), r.logical())
}

#[test]
fn thread_counts_and_root_spellings_agree() {
    let dir = TestDir::new("threads");
    let root = dir.path.clone();
    for d in 0..12 {
        let sub = root.join(format!("d{d}"));
        std::fs::create_dir_all(sub.join("x/y")).unwrap();
        for f in 0..(d * 7) {
            write_file(&sub.join(format!("f{f}.bin")), f * 100 + d);
        }
        write_file(&sub.join("x/y/leaf.txt"), d * 3 + 1);
    }
    // One directory larger than a flush slice.
    let big = root.join("big");
    std::fs::create_dir(&big).unwrap();
    for f in 0..5000 {
        write_file(&big.join(format!("{f:05}")), f % 3);
    }

    let (base, _) = scan_tree(&root, 1);
    let base = shape(&base.read().unwrap());
    assert_eq!(
        base.0.len(),
        12 * 3 + (0..12).map(|d| d * 7).sum::<usize>() + 12 + 1 + 5000
    );
    for threads in [0, 2, 16] {
        let (t, _) = scan_tree(&root, threads);
        let t = t.read().unwrap();
        t.check_invariants().unwrap();
        assert_eq!(shape(&t), base, "threads = {threads}");
    }

    // Other spellings of the same root give the same tree.
    let mut verbatim = OsString::from(r"\\?\");
    verbatim.push(std::path::absolute(&root).unwrap());
    let mut trailing = root.clone().into_os_string();
    trailing.push("\\");
    for spelling in [PathBuf::from(verbatim), PathBuf::from(trailing)] {
        let tree = Tree::for_path(&root).into_shared();
        walk::scan(&spelling, &opts(3), &tree, &ScanProgress::new())
            .unwrap_or_else(|e| panic!("{}: {e}", spelling.display()));
        assert_eq!(shape(&tree.read().unwrap()), base, "{}", spelling.display());
    }
    assert_eq!(base.2, reference_totals(&root).logical);
}

#[test]
fn scan_into_fills_a_subdirectory() {
    let dir = TestDir::new("into");
    let root = dir.path.clone();
    std::fs::create_dir_all(root.join("sub/inner")).unwrap();
    write_file(&root.join("sub/x.bin"), 100);
    write_file(&root.join("sub/inner/y.bin"), 200);
    write_file(&root.join("other.bin"), 50);

    let tree = Tree::for_path(&root).into_shared();
    let sub = tree
        .write()
        .unwrap()
        .add_child(NodeId::ROOT, b"sub", 0, 0, NodeFlags::DIR);
    let progress = ScanProgress::new();
    let summary = walk::scan_into(&tree, sub, &root.join("sub"), &opts(0), &progress).unwrap();
    assert_eq!(
        (summary.files, summary.dirs, summary.inaccessible),
        (2, 1, 0)
    );
    assert_eq!(progress.snapshot().files, 2);

    let t = tree.read().unwrap();
    t.check_invariants().unwrap();
    assert_eq!(t.node(NodeId::ROOT).logical(), 300);
    assert_eq!(t.node(sub).logical(), 300);
    assert_eq!(t.node(node(&t, "sub/inner/y.bin")).logical(), 200);
    assert_eq!(t.node(node(&t, "sub/inner")).parent, sub);
    assert!(t.lookup_relative(b"other.bin").is_none());
    assert_eq!(t.len(), 5);

    // Filling a leaf directory: no subdirectories, so no pool is started.
    drop(t);
    let inner_id = node(&tree.read().unwrap(), "sub/inner");
    let leaf = tree
        .write()
        .unwrap()
        .add_child(inner_id, b"new", 0, 0, NodeFlags::DIR);
    std::fs::create_dir(root.join("sub/inner/new")).unwrap();
    write_file(&root.join("sub/inner/new/z.bin"), 7);
    let s = walk::scan_into(
        &tree,
        leaf,
        &root.join("sub/inner/new"),
        &opts(0),
        &progress,
    )
    .unwrap();
    assert_eq!((s.files, s.dirs), (1, 0));
    let t = tree.read().unwrap();
    t.check_invariants().unwrap();
    assert_eq!(t.node(NodeId::ROOT).logical(), 307);
}

#[test]
fn bad_roots_are_errors() {
    let dir = TestDir::new("errors");
    let missing = dir.path.join("does-not-exist");
    let tree = Tree::for_path(&missing).into_shared();
    match walk::scan(&missing, &opts(0), &tree, &ScanProgress::new()) {
        Err(Error::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{e}"),
        other => panic!("expected NotFound, got {other:?}"),
    }

    let file = dir.path.join("plain.txt");
    write_file(&file, 5);
    let tree = Tree::for_path(&file).into_shared();
    match walk::scan(&file, &opts(0), &tree, &ScanProgress::new()) {
        Err(Error::Io(_)) => {}
        other => panic!("expected an I/O error for a file root, got {other:?}"),
    }
    assert_eq!(tree.read().unwrap().len(), 1);
}

#[test]
fn cancelled_scan_stops() {
    let dir = TestDir::new("cancel");
    std::fs::create_dir_all(dir.path.join("a/b")).unwrap();
    write_file(&dir.path.join("a/b/c"), 1);
    let progress = ScanProgress::new();
    progress.cancel();
    let tree = Tree::for_path(&dir.path).into_shared();
    assert!(matches!(
        walk::scan(&dir.path, &opts(0), &tree, &progress),
        Err(Error::Cancelled)
    ));
    assert_eq!(tree.read().unwrap().len(), 1);
}

/// Scans `C:\Windows` (or `TESSERA_BENCH_PATH`) and prints the numbers.
/// `TESSERA_BENCH_THREADS` picks the worker count (default: automatic).
/// Run with `cargo test --release --test walk -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark: scans C:\\Windows"]
fn bench_scan_windows_directory() {
    let path = std::env::var_os("TESSERA_BENCH_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let threads = std::env::var("TESSERA_BENCH_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let tree = Tree::for_path(&path).into_shared();
    let progress = ScanProgress::new();
    let start = Instant::now();
    let summary = walk::scan(&path, &opts(threads), &tree, &progress).unwrap();
    let elapsed = start.elapsed();
    let t = tree.read().unwrap();
    let mem = t.memory();
    println!(
        "walk {}: {} nodes ({} files, {} dirs, {} inaccessible) in {:.3} s \
         = {:.0} entries/s with {} threads; {:.1} GB on disk; tree {:.1} MiB \
         (nodes {:.1} + names {:.1})",
        path.display(),
        t.len(),
        summary.files,
        summary.dirs,
        summary.inaccessible,
        elapsed.as_secs_f64(),
        t.len() as f64 / elapsed.as_secs_f64(),
        if threads == 0 {
            walk::default_threads()
        } else {
            threads
        },
        t.node(NodeId::ROOT).size() as f64 / 1e9,
        mem.total() as f64 / 1048576.0,
        mem.nodes as f64 / 1048576.0,
        mem.names as f64 / 1048576.0,
    );
    t.check_invariants().unwrap();
}
