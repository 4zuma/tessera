//! Tests against real NTFS volumes made by `mkntfs` (Linux only).
//!
//! The volumes are filled through an `ntfs-3g` FUSE mount (directly as
//! root, else through `sudo -n`), walked while mounted to get the expected
//! result, unmounted, and then MFT-scanned from the image file. Tests are
//! skipped with a note when the tools are missing or mounting is not
//! permitted. Images are created at test time in the temp directory.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::build::Built;
use super::reader::{self, Geometry, ReadAt, Tuning};
use crate::scan::ScanProgress;
use crate::tree::{NodeFlags, NodeId, Tree};

fn find_tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/sbin", "/sbin", "/usr/local/sbin", "/usr/bin", "/bin"].map(PathBuf::from))
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Notes why a test cannot run. With `TESSERA_REQUIRE_NTFS_IMAGES=1` (set
/// in CI) that is a failure instead.
fn skip(why: &str) {
    if std::env::var("TESSERA_REQUIRE_NTFS_IMAGES").as_deref() == Ok("1") {
        panic!("NTFS image test cannot run: {why}");
    }
    eprintln!("note: NTFS image test skipped: {why}");
}

/// A temporary directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let p =
            std::env::temp_dir().join(format!("tessera-mft-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Creates an empty NTFS image of `mib` MiB.
fn make_image(path: &Path, mib: u64, cluster: Option<u32>) -> bool {
    let Some(mkntfs) = find_tool("mkntfs") else {
        skip("mkntfs not installed (apt install ntfs-3g)");
        return false;
    };
    fs::File::create(path).unwrap().set_len(mib << 20).unwrap();
    let mut cmd = Command::new(mkntfs);
    cmd.args(["-F", "-f", "-q", "-s", "512"]);
    if let Some(c) = cluster {
        cmd.args(["-c", &c.to_string()]);
    }
    let out = cmd.arg(path).stderr(Stdio::piped()).output().unwrap();
    if !out.status.success() {
        panic!("mkntfs failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    true
}

fn is_root() -> bool {
    fs::metadata("/proc/self")
        .map(|m| m.uid() == 0)
        .unwrap_or(false)
}

/// Runs a privileged command directly as root, else through `sudo -n`.
fn privileged(program: &Path) -> Command {
    if is_root() {
        Command::new(program)
    } else {
        let mut c = Command::new("sudo");
        c.arg("-n").arg(program);
        c
    }
}

/// An `ntfs-3g` mount running in the foreground as a child process.
struct Mount {
    child: Child,
    dir: PathBuf,
    mounted: bool,
}

fn is_mounted(dir: &Path) -> bool {
    let dir = dir.to_string_lossy();
    fs::read_to_string("/proc/mounts")
        .unwrap_or_default()
        .lines()
        .any(|l| l.split(' ').nth(1) == Some(&*dir))
}

impl Mount {
    fn new(image: &Path, dir: &Path) -> Option<Mount> {
        let Some(ntfs3g) = find_tool("ntfs-3g") else {
            skip("ntfs-3g not installed");
            return None;
        };
        if !Path::new("/dev/fuse").exists() {
            skip("/dev/fuse is missing");
            return None;
        }
        fs::create_dir_all(dir).unwrap();
        let me = fs::metadata(dir).unwrap();
        // no_detach keeps ntfs-3g in the foreground, so we can wait for it
        // to finish writing the volume after unmounting.
        let mut opts = String::from("no_detach,streams_interface=xattr");
        if !is_root() {
            opts += &format!(",uid={},gid={}", me.uid(), me.gid());
        }
        let child = privileged(&ntfs3g)
            .args(["-o", &opts])
            .arg(image)
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;
        let mut m = Mount {
            child,
            dir: dir.to_path_buf(),
            mounted: false,
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if is_mounted(dir) {
                m.mounted = true;
                return Some(m);
            }
            if let Ok(Some(status)) = m.child.try_wait() {
                let mut err = String::new();
                if let Some(mut e) = m.child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut e, &mut err);
                }
                skip(&format!(
                    "cannot mount with ntfs-3g ({status}): {}",
                    err.trim()
                ));
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        skip("ntfs-3g mount timed out");
        None
    }

    /// Unmounts and waits until ntfs-3g has written everything back.
    fn unmount(&mut self) {
        if !self.mounted {
            return;
        }
        let ok = find_tool("umount")
            .map(|u| {
                privileged(&u)
                    .arg(&self.dir)
                    .status()
                    .is_ok_and(|s| s.success())
            })
            .unwrap_or(false);
        if !ok {
            let _ = Command::new("fusermount").arg("-u").arg(&self.dir).status();
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                self.mounted = false;
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "ntfs-3g did not exit after unmounting {}",
            self.dir.display()
        );
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        if self.mounted {
            self.unmount();
        }
    }
}

fn set_xattr(path: &Path, name: &str, value: &[u8]) -> bool {
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn setxattr(
            path: *const c_char,
            name: *const c_char,
            value: *const c_void,
            size: usize,
            flags: c_int,
        ) -> c_int;
    }
    let p = CString::new(path.as_os_str().as_bytes()).unwrap();
    let n = CString::new(name).unwrap();
    // SAFETY: valid NUL-terminated strings and a valid value buffer.
    unsafe {
        setxattr(
            p.as_ptr(),
            n.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
        ) == 0
    }
}

fn write_file(path: &Path, len: usize, byte: u8) {
    fs::write(path, vec![byte; len]).unwrap();
}

/// A mount point (junction) reparse buffer pointing at `target`.
fn junction_data(target: &str) -> Vec<u8> {
    let subst: Vec<u16> = format!("\\??\\{target}").encode_utf16().collect();
    let print: Vec<u16> = target.encode_utf16().collect();
    let mut path = Vec::new();
    for u in subst.iter().chain([&0]).chain(&print).chain([&0]) {
        path.extend_from_slice(&u.to_le_bytes());
    }
    let mut v = Vec::new();
    v.extend_from_slice(&0xA000_0003u32.to_le_bytes());
    v.extend_from_slice(&((8 + path.len()) as u16).to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&((subst.len() * 2) as u16).to_le_bytes());
    v.extend_from_slice(&((subst.len() * 2 + 2) as u16).to_le_bytes());
    v.extend_from_slice(&((print.len() * 2) as u16).to_le_bytes());
    v.extend_from_slice(&path);
    v
}

/// What the mounted file system showed for one entry.
#[derive(Debug)]
struct Seen {
    is_dir: bool,
    is_symlink: bool,
    len: u64,
    ino: u64,
    nlink: u64,
}

/// Walks the mounted volume (not following symlinks).
fn walk(root: &Path) -> HashMap<String, Seen> {
    let mut out = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).unwrap() {
            let e = e.unwrap();
            let rel = e
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('/', "\\");
            // ntfs-3g cannot stat a junction to another drive (EIO).
            let Ok(m) = fs::symlink_metadata(e.path()) else {
                let ino = std::os::unix::fs::DirEntryExt::ino(&e);
                let s = Seen {
                    is_dir: false,
                    is_symlink: true,
                    len: 0,
                    ino,
                    nlink: 1,
                };
                out.insert(rel, s);
                continue;
            };
            if m.is_dir() {
                stack.push(e.path());
            }
            out.insert(
                rel,
                Seen {
                    is_dir: m.is_dir(),
                    is_symlink: m.file_type().is_symlink(),
                    len: m.len(),
                    ino: m.ino(),
                    nlink: m.nlink(),
                },
            );
        }
    }
    out
}

fn scan_image(image: &Path, tuning: Tuning) -> (Built, reader::MftLayout) {
    let mut f = fs::File::open(image).unwrap();
    let mut boot = [0u8; 512];
    f.read_exact_at(0, &mut boot).unwrap();
    let g = Geometry::from_boot_sector(&boot).unwrap();
    let layout = reader::load_layout(&mut f, &g).unwrap();
    let progress = ScanProgress::new();
    let built = super::scan_source(&mut f, g, b"N:\\", &progress, &tuning).unwrap();
    (built, layout)
}

fn node(t: &Tree, rel: &str) -> NodeId {
    t.lookup_relative(rel.as_bytes())
        .unwrap_or_else(|| panic!("{rel} missing from the MFT tree"))
}

/// Checks the MFT tree against a walk of the mounted volume.
fn assert_matches_walk(b: &Built, seen: &HashMap<String, Seen>) {
    let t = &b.tree;
    t.check_invariants().unwrap();
    // Every user-visible entry is in the tree, with the same kind and length.
    let mut by_ino: HashMap<u64, Vec<NodeId>> = HashMap::new();
    for (rel, s) in seen {
        let id = node(t, rel);
        let n = t.node(id);
        assert_eq!(t.path_lossy(id), format!("N:\\{rel}"), "exact name");
        assert!(!n.flags.contains(NodeFlags::METAFILE), "{rel}");
        if s.is_symlink {
            assert!(
                n.flags.contains(NodeFlags::REPARSE),
                "{rel} is a reparse point"
            );
            continue;
        }
        assert_eq!(n.is_dir(), s.is_dir, "{rel}");
        if !s.is_dir {
            by_ino.entry(s.ino).or_default().push(id);
            if n.flags.contains(NodeFlags::HARDLINK) {
                assert!(s.nlink > 1, "{rel}");
                assert_eq!((n.size(), n.logical()), (0, 0), "{rel}");
            } else {
                assert_eq!(n.logical(), s.len, "{rel}");
            }
        }
        assert_eq!(b.frn.get(s.ino).map(|p| t.node(p).is_dir()), Some(s.is_dir));
    }
    // Each file is counted once, through the name the FRN index points to.
    for (ino, ids) in &by_ino {
        let primary: Vec<_> = ids
            .iter()
            .filter(|&&i| !t.node(i).flags.contains(NodeFlags::HARDLINK))
            .collect();
        assert_eq!(primary.len(), 1, "record {ino}: {ids:?}");
        assert_eq!(b.frn.get(*ino), Some(*primary[0]));
    }
    // And nothing else is, apart from NTFS metadata files.
    let user_nodes = (1..t.len())
        .map(|i| NodeId(i as u32))
        .filter(|&i| !t.node(i).flags.contains(NodeFlags::METAFILE))
        .count();
    assert_eq!(user_nodes, seen.len());
    assert_eq!(b.skipped, 0);
}

fn assert_metafiles(t: &Tree) {
    for name in [
        "$MFT", "$MFTMirr", "$LogFile", "$Volume", "$AttrDef", "$Bitmap", "$Boot", "$BadClus",
        "$Secure", "$UpCase", "$Extend",
    ] {
        let id = node(t, name);
        assert!(t.node(id).flags.contains(NodeFlags::METAFILE), "{name}");
    }
    for name in ["$Extend\\$Quota", "$Extend\\$ObjId", "$Extend\\$Reparse"] {
        assert!(
            t.node(node(t, name)).flags.contains(NodeFlags::METAFILE),
            "{name}"
        );
    }
    let mft = t.node(node(t, "$MFT"));
    assert!(
        mft.size() >= 16 * 1024 && mft.size() % 512 == 0,
        "$MFT size {}",
        mft.size()
    );
    assert!(t.node(node(t, "$LogFile")).size() > 0);
    assert_eq!(t.node(node(t, "$BadClus")).size(), 0, "$Bad is not counted");
}

#[test]
fn populated_volume_matches_a_walk() {
    let s = Scratch::new("pop");
    let img = s.0.join("vol.img");
    if !make_image(&img, 64, Some(4096)) {
        return;
    }
    let mnt = s.0.join("mnt");
    let Some(mut m) = Mount::new(&img, &mnt) else {
        return;
    };
    let r = mnt.as_path();
    fs::create_dir_all(r.join("docs/sub/deeper")).unwrap();
    write_file(&r.join("docs/readme.txt"), 1234, b'r');
    write_file(&r.join("docs/tiny.txt"), 10, b't');
    write_file(&r.join("docs/empty.txt"), 0, 0);
    write_file(&r.join("docs/sub/deeper/x.bin"), 100_000, b'x');
    fs::create_dir(r.join("Ünïcødé 照片")).unwrap();
    write_file(&r.join("Ünïcødé 照片/🦀 crab.rs"), 5000, b'c');

    // A file with 40 names: its $FILE_NAMEs overflow into extension records.
    fs::create_dir_all(r.join("links/more")).unwrap();
    write_file(&r.join("links/orig.dat"), 10_000, b'o');
    for i in 0..40 {
        fs::hard_link(
            r.join("links/orig.dat"),
            r.join(format!(
                "links/more/a rather long hard link name number {i:02}.dat"
            )),
        )
        .unwrap();
    }

    // Sparse: 1 MiB hole, then 4 KiB of data.
    {
        let mut f = fs::File::create(r.join("sparse.bin")).unwrap();
        f.set_len(1 << 20).unwrap();
        std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(1 << 20)).unwrap();
        f.write_all(&[7u8; 4096]).unwrap();
    }

    // Compressed: files created in a compressed directory are compressed.
    fs::create_dir(r.join("comp")).unwrap();
    let compressed = set_xattr(
        &r.join("comp"),
        "system.ntfs_attrib_be",
        &0x800u32.to_be_bytes(),
    );
    fs::write(r.join("comp/c.txt"), b"abcd".repeat(50_000)).unwrap();

    // A Win32 name with a separate DOS name.
    fs::create_dir(r.join("dos")).unwrap();
    write_file(&r.join("dos/Long File Name.txt"), 3000, b'd');
    let dos = set_xattr(
        &r.join("dos/Long File Name.txt"),
        "system.ntfs_dos_name",
        b"LONGFI~1.TXT",
    );

    // A named stream is not counted.
    write_file(&r.join("streams.txt"), 5000, b's');
    let stream = set_xattr(&r.join("streams.txt"), "user.extra", &[1u8; 20_000]);

    // Hidden + archive.
    write_file(&r.join("hidden.txt"), 600, b'h');
    let hidden = set_xattr(
        &r.join("hidden.txt"),
        "system.ntfs_attrib_be",
        &0x22u32.to_be_bytes(),
    );

    // A junction (mount point reparse point) on an empty directory.
    fs::create_dir(r.join("junction")).unwrap();
    let junction = set_xattr(
        &r.join("junction"),
        "system.ntfs_reparse_data",
        &junction_data("C:\\target"),
    );

    // Enough files for a non-resident directory index and several chunks.
    fs::create_dir(r.join("many")).unwrap();
    for i in 0..1500 {
        write_file(&r.join(format!("many/file-{i:04}.txt")), i % 3000, b'm');
    }

    let seen = walk(r);
    m.unmount();
    eprintln!(
        "image features: compressed={compressed} dos={dos} stream={stream} hidden={hidden} junction={junction}"
    );

    for tuning in [
        Tuning::default(),
        Tuning {
            chunk_bytes: 64 << 10,
            skip_gap_bytes: 16 << 10,
            use_bitmap: true,
        },
        Tuning {
            chunk_bytes: 4096,
            skip_gap_bytes: 0,
            use_bitmap: false,
        },
    ] {
        let (b, _) = scan_image(&img, tuning);
        let t = &b.tree;
        assert_matches_walk(&b, &seen);
        assert_metafiles(t);

        let n = |p: &str| *t.node(node(t, p));
        assert_eq!(
            (
                n("docs\\readme.txt").size(),
                n("docs\\readme.txt").logical()
            ),
            (4096, 1234)
        );
        assert_eq!(
            (n("docs\\tiny.txt").size(), n("docs\\tiny.txt").logical()),
            (0, 10),
            "resident"
        );
        assert_eq!(n("docs\\sub\\deeper\\x.bin").size(), 102_400);
        let docs = n("docs");
        assert_eq!(docs.logical(), 1234 + 10 + 100_000);
        assert_eq!(t.child_count(node(t, "docs")), 4);

        let orig = n("links\\orig.dat");
        assert_eq!((orig.size(), orig.logical()), (12288, 10_000));
        assert!(!orig.flags.contains(NodeFlags::HARDLINK));
        let more = node(t, "links\\more");
        assert_eq!(t.child_count(more), 40);
        assert!(
            t.children(more)
                .all(|c| t.node(c).flags.contains(NodeFlags::HARDLINK))
        );
        assert_eq!(t.node(node(t, "links")).size(), 12288);

        let sparse = n("sparse.bin");
        assert_eq!((sparse.size(), sparse.logical()), (4096, (1 << 20) + 4096));
        assert!(sparse.flags.contains(NodeFlags::COMPRESSED));

        if compressed {
            let c = n("comp\\c.txt");
            assert_eq!(c.logical(), 200_000);
            assert!(c.size() < c.logical() / 2, "compressed size {}", c.size());
            assert!(c.flags.contains(NodeFlags::COMPRESSED));
        }
        if dos {
            assert!(t.lookup_relative(b"dos\\LONGFI~1.TXT").is_none());
            assert_eq!(t.child_count(node(t, "dos")), 1);
        }
        if stream {
            let s = n("streams.txt");
            assert_eq!((s.size(), s.logical()), (8192, 5000));
        }
        if hidden {
            assert!(n("hidden.txt").flags.contains(NodeFlags::HIDDEN));
        }
        if junction {
            let j = n("junction");
            assert!(j.flags.contains(NodeFlags::REPARSE | NodeFlags::DIR));
            assert_eq!(t.child_count(node(t, "junction")), 0);
        }
        assert_eq!(t.child_count(node(t, "many")), 1500);
        assert_eq!(
            t.node(node(t, "many")).logical(),
            (0..1500u64).map(|i| i % 3000).sum::<u64>()
        );
        assert!(n("Ünïcødé 照片\\🦀 crab.rs").logical() == 5000);
    }
}

#[test]
fn fragmented_mft_with_small_clusters_and_free_gaps() {
    let s = Scratch::new("frag");
    let img = s.0.join("vol.img");
    if !make_image(&img, 16, Some(512)) {
        return;
    }
    let mnt = s.0.join("mnt");
    let Some(mut m) = Mount::new(&img, &mnt) else {
        return;
    };
    let r = mnt.as_path();
    // Fill the volume, punch holes, then grow the MFT into the holes.
    fs::create_dir(r.join("fill")).unwrap();
    let mut n = 0;
    while fs::write(r.join(format!("fill/{n:04}")), vec![0xAAu8; 32 << 10]).is_ok() {
        n += 1;
    }
    let _ = fs::remove_file(r.join(format!("fill/{n:04}")));
    for i in (0..n).step_by(2) {
        fs::remove_file(r.join(format!("fill/{i:04}"))).unwrap();
    }
    fs::create_dir(r.join("many")).unwrap();
    for i in 0..2500 {
        if fs::write(r.join(format!("many/{i:04}.txt")), b"0123456789").is_err() {
            break;
        }
    }
    // Free a long run of records in the middle of the MFT.
    for i in 300..2000 {
        let _ = fs::remove_file(r.join(format!("many/{i:04}.txt")));
    }
    let seen = walk(r);
    m.unmount();

    for tuning in [
        Tuning::default(),
        Tuning {
            chunk_bytes: 16 << 10,
            skip_gap_bytes: 8 << 10,
            use_bitmap: true,
        },
        Tuning {
            chunk_bytes: 64 << 10,
            skip_gap_bytes: 0,
            use_bitmap: false,
        },
    ] {
        let (b, layout) = scan_image(&img, tuning);
        assert!(
            layout.runs.len() > 1,
            "expected a fragmented $MFT, got {:?}",
            layout.runs
        );
        assert_eq!(layout.cluster_size, 512);
        assert_matches_walk(&b, &seen);
        assert_metafiles(&b.tree);
        // The skipped free stretch really was skipped.
        let skipped = layout.chunk_pieces(0, layout.records, 8 << 10);
        let read: usize = skipped.iter().map(|p| p.len).sum();
        assert!(
            read < layout.records as usize * layout.record_size,
            "{read}"
        );
    }
}

#[test]
fn image_filled_without_mounting() {
    // Needs only mkntfs and ntfscp, so it also runs where FUSE is not allowed.
    let s = Scratch::new("cp");
    let img = s.0.join("vol.img");
    if !make_image(&img, 8, None) {
        return;
    }
    let Some(ntfscp) = find_tool("ntfscp") else {
        return skip("ntfscp not installed");
    };
    let src = s.0.join("src.bin");
    write_file(&src, 70_000, b'z');
    for name in ["copied.bin", "Ä second.bin"] {
        let st = Command::new(&ntfscp)
            .args(["-f", "-q"])
            .arg(&img)
            .arg(&src)
            .arg(name)
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "ntfscp failed");
    }
    let progress = ScanProgress::new();
    let (t, summary) =
        super::scan_reader(fs::File::open(&img).unwrap(), b"I:\\", &progress).unwrap();
    t.check_invariants().unwrap();
    assert_metafiles(&t);
    assert_eq!(t.name(NodeId::ROOT), b"I:\\");
    for name in ["copied.bin", "Ä second.bin"] {
        let n = t.node(node(&t, name));
        assert_eq!((n.logical(), n.size()), (70_000, 73_728), "{name}");
    }
    assert_eq!(summary.mode, crate::scan::ScanMode::Mft);
    assert_eq!(summary.inaccessible, 0);
    assert!(summary.frn_index.is_some());
    assert_eq!(progress.snapshot().bytes, t.node(NodeId::ROOT).size());
    assert_eq!(summary.files + summary.dirs + 1, t.len() as u64);
}
