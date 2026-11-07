//! MFT scan of the real system volume. Needs an elevated process, so these
//! tests are `#[ignore]`d and also check `TESSERA_ADMIN_TESTS=1`. Run them
//! from an elevated prompt with
//! `TESSERA_ADMIN_TESTS=1 cargo test -p tessera-core --test mft -- --include-ignored`
//! (CI does, since its Windows runners are elevated).
#![cfg(windows)]

use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::time::Instant;

use tessera_core::scan::{self, ScanMode, ScanOptions, ScanProgress};
use tessera_core::{Error, NodeFlags, NodeId, Tree};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, GetFileInformationByHandle,
};

fn enabled() -> bool {
    let on = std::env::var("TESSERA_ADMIN_TESTS").as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set TESSERA_ADMIN_TESTS=1 in an elevated prompt to run");
    }
    on
}

/// Removes the test directory at the end.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A fresh directory on the system volume, by its long-name path.
fn test_dir() -> (Cleanup, String) {
    let base = fs::canonicalize(std::env::temp_dir()).unwrap();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let dir = base.join(format!("tessera-mft-{}-{nanos}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let s = dir.to_string_lossy().into_owned();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned();
    (Cleanup(dir), s)
}

/// The MFT record number of a file or directory (low 48 bits of its file
/// index).
fn record_number(path: &Path) -> u64 {
    // Directories can only be opened with backup semantics.
    let f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)
        .unwrap();
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: valid handle and output struct.
    unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle()), &mut info) }.unwrap();
    ((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)) & 0xFFFF_FFFF_FFFF
}

fn write(path: &Path, len: usize) {
    let mut f = fs::File::create(path).unwrap();
    std::io::Write::write_all(&mut f, &vec![0x5Au8; len]).unwrap();
    f.sync_all().unwrap();
}

#[test]
#[ignore = "needs an elevated prompt: set TESSERA_ADMIN_TESTS=1 and pass --include-ignored"]
fn mft_scan_of_the_system_volume() {
    if !enabled() {
        return;
    }
    assert!(
        scan::is_elevated(),
        "TESSERA_ADMIN_TESTS=1 needs an elevated process"
    );

    let (cleanup, dir) = test_dir();
    let d = cleanup.0.clone();
    write(&d.join("a.bin"), 10_000);
    write(&d.join("tiny.txt"), 100);
    fs::create_dir(d.join("sub")).unwrap();
    write(&d.join("sub").join("big.bin"), 1 << 20);
    fs::hard_link(d.join("a.bin"), d.join("sub").join("a-link.bin")).unwrap();
    write(&d.join("Ünïcødé 照片 🦀.txt"), 5000);
    fs::create_dir(d.join("empty")).unwrap();

    let root = PathBuf::from(&dir[..3]); // e.g. C:\
    let rel = &dir[3..];
    assert!(
        scan::mft::can_scan(&root),
        "{} should be MFT-scannable",
        root.display()
    );
    assert!(
        !scan::mft::can_scan(Path::new(&dir)),
        "a folder is not a volume root"
    );

    let tree = Tree::for_path(&root).into_shared();
    let progress = ScanProgress::new();
    let opts = ScanOptions {
        mode: ScanMode::Mft,
        ..Default::default()
    };
    let t0 = Instant::now();
    let summary = scan::scan(&root, &opts, &tree, &progress).unwrap();
    let elapsed = t0.elapsed();
    let t = tree.read().unwrap();
    let mem = t.memory();
    let frn = summary.frn_index.as_ref().expect("FRN index");
    eprintln!(
        "MFT scan of {}: {elapsed:?}, {} nodes ({} files, {} dirs, {} skipped), \
         tree {:.1} MiB (nodes {:.1} + names {:.1}), FRN index {:.1} MiB, {:.2} GiB on disk",
        root.display(),
        t.len(),
        summary.files,
        summary.dirs,
        summary.inaccessible,
        mem.total() as f64 / 1048576.0,
        mem.nodes as f64 / 1048576.0,
        mem.names as f64 / 1048576.0,
        frn.heap_bytes() as f64 / 1048576.0,
        t.node(NodeId::ROOT).size() as f64 / 1073741824.0,
    );

    assert_eq!(summary.mode, ScanMode::Mft);
    t.check_invariants().unwrap();
    assert_eq!(t.name(NodeId::ROOT), &dir.as_bytes()[..3]);
    assert!(summary.usn_checkpoint.is_some(), "C: has a USN journal");
    let vol = summary.volume.as_ref().expect("volume info");
    assert_eq!(vol.file_system, "NTFS");
    assert!(t.node(NodeId::ROOT).size() <= vol.total_bytes);
    let progress = progress.snapshot();
    assert_eq!(progress.files, summary.files);
    assert_eq!(progress.bytes, t.node(NodeId::ROOT).size());

    let mft = t.lookup_relative(b"$MFT").expect("$MFT");
    assert!(t.node(mft).flags.contains(NodeFlags::METAFILE));
    assert!(t.node(mft).size() > 1 << 20);

    let top = t
        .lookup_relative(rel.as_bytes())
        .expect("the test directory is in the MFT tree");
    assert!(t.path_lossy(top).eq_ignore_ascii_case(&dir));
    assert_eq!(frn.get(record_number(&d)), Some(top));
    let node = |p: &str| {
        let id = t
            .lookup_relative(format!("{rel}\\{p}").as_bytes())
            .unwrap_or_else(|| panic!("{p} is missing"));
        (id, *t.node(id))
    };

    let (a_id, a) = node("a.bin");
    assert_eq!(a.logical(), 10_000);
    assert!(
        a.size() >= 10_000 && a.size() < 10_000 + 65_536 && a.size() % 512 == 0,
        "{}",
        a.size()
    );
    assert!(!a.flags.contains(NodeFlags::HARDLINK));
    assert_eq!(frn.get(record_number(&d.join("a.bin"))), Some(a_id));

    let (_, link) = node("sub\\a-link.bin");
    assert!(link.flags.contains(NodeFlags::HARDLINK));
    assert_eq!((link.size(), link.logical()), (0, 0));

    let (_, tiny) = node("tiny.txt");
    assert_eq!((tiny.size(), tiny.logical()), (0, 100), "resident data");

    let (_, big) = node("sub\\big.bin");
    assert_eq!((big.size(), big.logical()), (1 << 20, 1 << 20));

    let (_, uni) = node("Ünïcødé 照片 🦀.txt");
    assert_eq!(uni.logical(), 5000);

    let (empty_id, empty) = node("empty");
    assert!(empty.is_dir());
    assert_eq!(t.child_count(empty_id), 0);

    let all = t.node(top);
    assert_eq!(t.child_count(top), 5);
    assert_eq!(all.logical(), 10_000 + 100 + (1 << 20) + 5000);
    assert_eq!(all.size(), a.size() + (1 << 20) + uni.size());
}

#[test]
#[ignore = "needs an elevated prompt: set TESSERA_ADMIN_TESTS=1 and pass --include-ignored"]
fn cancelled_mft_scan() {
    if !enabled() {
        return;
    }
    let root = PathBuf::from(std::env::var("SystemDrive").unwrap_or("C:".into()) + "\\");
    let tree = Tree::for_path(&root).into_shared();
    let progress = ScanProgress::new();
    progress.cancel();
    let r = scan::mft::scan(&root, &ScanOptions::default(), &tree, &progress);
    assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
    assert_eq!(tree.read().unwrap().len(), 1, "tree untouched");
}

#[test]
fn folders_are_not_mft_scannable() {
    // No admin rights needed.
    let dir = std::env::temp_dir();
    assert!(!scan::mft::can_scan(&dir));
    let tree = Tree::for_path(&dir).into_shared();
    let r = scan::mft::scan(&dir, &ScanOptions::default(), &tree, &ScanProgress::new());
    assert!(matches!(r, Err(Error::Unsupported(_))), "{r:?}");
}
