//! Unit tests for the walk internals that the public API cannot reach:
//! path normalization, the fallback query levels, forced hard-link modes.
//! The end-to-end tests are in `tests/walk.rs`.

use std::path::{Path, PathBuf};

use super::*;
use crate::Tree;

fn nt(path: &str) -> String {
    String::from_utf16(&nt::root_path(Path::new(path)).unwrap()).unwrap()
}

#[test]
fn root_paths_become_nt_paths() {
    assert_eq!(nt(r"C:\"), r"\??\C:\");
    assert_eq!(nt(r"C:\Windows\"), r"\??\C:\Windows");
    assert_eq!(nt(r"C:/Windows/System32"), r"\??\C:\Windows\System32");
    assert_eq!(nt(r"\\?\C:\Windows"), r"\??\C:\Windows");
    assert_eq!(nt(r"\\?\C:"), r"\??\C:\");
    assert_eq!(nt(r"\\?\C:\"), r"\??\C:\");
    assert_eq!(nt(r"\\server\share\dir"), r"\??\UNC\server\share\dir");
    assert_eq!(
        nt(r"\\?\UNC\server\share\dir\"),
        r"\??\UNC\server\share\dir"
    );
    assert_eq!(
        nt(r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\"),
        r"\??\Volume{01234567-89ab-cdef-0123-456789abcdef}\"
    );
    // Relative paths are made absolute.
    assert!(nt("some-dir").starts_with(r"\??\"));
    assert!(nt("some-dir").ends_with(r"\some-dir"));

    let child = |p: &str, n: &str| {
        let p: Vec<u16> = p.encode_utf16().collect();
        let n: Vec<u16> = n.encode_utf16().collect();
        String::from_utf16(&nt::child_path(&p, &n)).unwrap()
    };
    assert_eq!(child(r"\??\C:\", "Users"), r"\??\C:\Users");
    assert_eq!(child(r"\??\C:\Users", "me"), r"\??\C:\Users\me");
}

struct TempTree(PathBuf);

impl TempTree {
    fn new(tag: &str) -> TempTree {
        let p =
            std::env::temp_dir().join(format!("tessera-walk-unit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempTree(p)
    }

    fn file(&self, rel: &str, len: usize) {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![7u8; len]).unwrap();
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(root: &Path, threads: usize, tweaks: Tweaks) -> (Tree, ScanSummary, WalkStats) {
    let tree = Tree::for_path(root).into_shared();
    let opts = ScanOptions {
        mode: ScanMode::Walk,
        threads,
    };
    let (summary, stats) = walk(
        &tree,
        NodeId::ROOT,
        root,
        &opts,
        &ScanProgress::new(),
        tweaks,
    )
    .expect("walk failed");
    let tree = std::sync::Arc::try_unwrap(tree)
        .ok()
        .expect("tree still shared")
        .into_inner()
        .unwrap();
    tree.check_invariants().unwrap();
    (tree, summary, stats)
}

/// Sorted (path, logical, flags) of every node.
fn listing(t: &Tree) -> Vec<(String, u64, u16)> {
    let mut v: Vec<_> = (1..t.len() as u32)
        .map(|i| {
            let n = t.node(NodeId(i));
            (t.path_lossy(NodeId(i)), n.logical(), n.flags.0)
        })
        .collect();
    v.sort();
    v
}

#[test]
fn every_query_level_gives_the_same_tree() {
    let tmp = TempTree::new("levels");
    for i in 0..30 {
        tmp.file(&format!("d{}/e{}/f{i}.bin", i % 4, i % 3), i * 1000 + 1);
    }
    tmp.file("top.txt", 12_345);
    tmp.file("empty", 0);
    std::fs::create_dir(tmp.0.join("empty-dir")).unwrap();

    let (base, base_summary, stats) = run(&tmp.0, 3, Tweaks::default());
    assert_eq!(stats.level, Level::IdFull, "fell back on a local volume");
    assert_eq!(base_summary.files, 32);
    for level in [Level::Full, Level::Find] {
        let (t, summary, stats) = run(&tmp.0, 3, Tweaks { level, links: None });
        assert_eq!(stats.level, level);
        assert_eq!(listing(&t), listing(&base), "{level:?}");
        assert_eq!(
            (summary.files, summary.dirs),
            (base_summary.files, base_summary.dirs)
        );
        if level == Level::Find {
            // Size on disk is estimated: whole clusters, at least the length.
            let top = t.node(t.lookup_relative(b"top.txt").unwrap());
            assert!(
                top.size() >= 12_345 && top.size() % 512 == 0,
                "{}",
                top.size()
            );
        }
    }
}

#[test]
fn forced_link_modes_count_a_hard_link_once() {
    let tmp = TempTree::new("links");
    tmp.file("a/original", 10_000);
    tmp.file("b/other", 3);
    std::fs::hard_link(tmp.0.join("a/original"), tmp.0.join("b/link")).unwrap();
    for mode in [LinkMode::Records, LinkMode::Ids] {
        for threads in [1, 4] {
            let (t, _, stats) = run(
                &tmp.0,
                threads,
                Tweaks {
                    level: Level::IdFull,
                    links: Some(mode),
                },
            );
            assert_eq!(stats.link_mode, mode);
            assert_eq!(stats.threads, threads);
            assert!(stats.link_set_bytes > 0);
            assert_eq!(t.node(NodeId::ROOT).logical(), 10_003, "{mode:?}");
            let flagged = (1..t.len() as u32)
                .filter(|&i| t.node(NodeId(i)).flags.contains(NodeFlags::HARDLINK))
                .count();
            assert_eq!(flagged, 1, "{mode:?}");
        }
    }
    // Without file ids nothing can be deduplicated.
    let (t, _, _) = run(
        &tmp.0,
        2,
        Tweaks {
            level: Level::Full,
            links: Some(LinkMode::Records),
        },
    );
    assert_eq!(t.node(NodeId::ROOT).logical(), 20_003);
}

/// Feeds hand-made listing entries through the same code a real listing
/// goes through: wine has no reparse points or cloud files, so this is
/// where their handling is checked off Windows.
#[test]
fn entries_are_classified() {
    use nt::*;
    const DIR: u32 = FILE_ATTRIBUTE_DIRECTORY;
    const REPARSE: u32 = FILE_ATTRIBUTE_REPARSE_POINT;
    const NORMAL: u32 = 0x80;

    let tree = Tree::new(br"X:\").into_shared();
    let progress = ScanProgress::new();
    let ctx = Ctx::new(&tree, &progress, LinkMode::Records, Level::IdFull, 4096, 1);
    let job = Job {
        node: NodeId::ROOT,
        path: r"\??\X:\".encode_utf16().collect(),
        open: Open::Follow,
    };
    // (name, attributes, logical, allocated, file id, reparse tag)
    type Listed = (&'static str, u32, u64, Option<u64>, u64, u32);
    let listing: &[Listed] = &[
        (".", DIR, 0, Some(0), 5, 0),
        ("..", DIR, 0, Some(0), 5, 0),
        (
            "plain.txt",
            NORMAL,
            100,
            Some(4096),
            0x0001_0000_0000_0040,
            0,
        ),
        (
            "second link.txt",
            NORMAL,
            100,
            Some(4096),
            0x0001_0000_0000_0040,
            0,
        ),
        ("junction", DIR | REPARSE, 0, Some(0), 70, 0xA000_0003),
        ("symlinked dir", DIR | REPARSE, 0, Some(0), 71, 0xA000_000C),
        ("file symlink", REPARSE, 0, Some(0), 72, 0xA000_000C),
        (
            "OneDrive",
            DIR | REPARSE | FILE_ATTRIBUTE_RECALL_ON_OPEN,
            0,
            Some(0),
            73,
            0x9000_701A,
        ),
        ("unknown tag", DIR | REPARSE, 0, Some(0), 74, 0),
        (
            "online-only.docx",
            FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | REPARSE,
            1_000_000,
            Some(0),
            75,
            0x9000_701A,
        ),
        ("offline.dat", FILE_ATTRIBUTE_OFFLINE, 50, Some(0), 76, 0),
        (
            "compact.dll",
            REPARSE,
            50_000,
            Some(20_480),
            77,
            IO_REPARSE_TAG_WOF,
        ),
        (
            "compressed.log",
            FILE_ATTRIBUTE_COMPRESSED,
            90_000,
            Some(8192),
            78,
            0,
        ),
        (
            "sparse.vhdx",
            FILE_ATTRIBUTE_SPARSE_FILE,
            1 << 30,
            Some(1 << 20),
            79,
            0,
        ),
        (
            "hidden system",
            FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM,
            1,
            Some(8),
            80,
            0,
        ),
        ("sub", DIR, 0, Some(0), 81, 0),
        ("no allocation", NORMAL, 5000, None, 0, 0),
        ("no id", NORMAL, 7, Some(4096), 0, 0),
        ("no id again", NORMAL, 7, Some(4096), 0, 0),
    ];
    let mut spawned: Vec<Job> = Vec::new();
    let mut pending = Pending::default();
    {
        let mut spawn = |j: Job| spawned.push(j);
        let mut sink = Sink {
            ctx: &ctx,
            job: &job,
            pending: &mut pending,
            spawn: &mut spawn,
        };
        for &(name, attributes, logical, allocated, file_id, reparse_tag) in listing {
            let name: Vec<u16> = name.encode_utf16().collect();
            sink.push(&Entry {
                name: &name,
                attributes,
                logical,
                allocated,
                file_id,
                reparse_tag,
            });
        }
        sink.flush();
    }

    let t = tree.read().unwrap();
    t.check_invariants().unwrap();
    assert_eq!(t.len(), listing.len() - 2 + 1, "'.' and '..' are skipped");
    let get = |name: &str| *t.node(t.lookup_relative(name.as_bytes()).unwrap());
    let flags = |name: &str| get(name).flags;
    use NodeFlags as F;

    assert_eq!(flags("plain.txt"), F::NONE);
    assert_eq!(get("plain.txt").size(), 4096);
    // Same MFT record: the second sighting is the extra link.
    assert_eq!(flags("second link.txt"), F::HARDLINK);
    assert_eq!(
        (
            get("second link.txt").size(),
            get("second link.txt").logical()
        ),
        (0, 0)
    );
    assert_eq!(flags("junction"), F::DIR | F::REPARSE);
    assert_eq!(flags("symlinked dir"), F::DIR | F::REPARSE);
    assert_eq!(flags("file symlink"), F::REPARSE);
    assert_eq!(flags("OneDrive"), F::DIR | F::CLOUD);
    assert_eq!(flags("unknown tag"), F::DIR);
    assert_eq!(flags("online-only.docx"), F::CLOUD);
    assert_eq!(get("online-only.docx").size(), 0);
    assert_eq!(get("online-only.docx").logical(), 1_000_000);
    assert_eq!(flags("offline.dat"), F::CLOUD);
    assert_eq!(flags("compact.dll"), F::COMPRESSED);
    assert_eq!(get("compact.dll").size(), 20_480);
    assert_eq!(flags("compressed.log"), F::COMPRESSED);
    assert_eq!(flags("sparse.vhdx"), F::COMPRESSED);
    assert_eq!(flags("hidden system"), F::HIDDEN | F::SYSTEM);
    assert_eq!(flags("sub"), F::DIR);
    // FindFirstFileExW reports no allocation: whole clusters are assumed.
    assert_eq!(get("no allocation").size(), 8192);
    // Files without an id are never taken for links of each other.
    assert_eq!(flags("no id again"), F::NONE);
    assert_eq!(get("no id again").logical(), 7);

    // Links and mount points are not entered; everything else is, and an
    // unreported tag is checked by the child's own task.
    let mut entered: Vec<(String, String, Open)> = spawned
        .iter()
        .map(|j| {
            (
                t.path_lossy(j.node),
                String::from_utf16(&j.path).unwrap(),
                j.open,
            )
        })
        .collect();
    entered.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        entered,
        [
            (
                r"X:\OneDrive".into(),
                r"\??\X:\OneDrive".into(),
                Open::NoFollow
            ),
            (r"X:\sub".into(), r"\??\X:\sub".into(), Open::NoFollow),
            (
                r"X:\unknown tag".into(),
                r"\??\X:\unknown tag".into(),
                Open::NoFollowReadTag
            ),
        ]
    );
    let snap = progress.snapshot();
    assert_eq!((snap.files, snap.dirs), (12, 5));
    assert_eq!(snap.bytes, t.node(NodeId::ROOT).size());
}

#[test]
fn volume_caps_of_temp_dir() {
    let tmp = TempTree::new("caps");
    let mut long = tmp.0.clone();
    for i in 0..5 {
        long.push(format!("{i}-{}", "v".repeat(60)));
    }
    std::fs::create_dir_all(&long).unwrap();
    for dir in [&tmp.0, &long] {
        let caps = nt::volume_caps(&nt::root_path(dir).unwrap()).expect("volume_caps failed");
        assert!(!caps.file_system.is_empty());
        assert!(
            caps.cluster >= 512 && caps.cluster.is_power_of_two(),
            "{caps:?}"
        );
        assert!(super::super::volume_info(&nt::win32_root(&nt::root_path(dir).unwrap())).is_ok());
    }
}

/// Memory of the hard-link sets for a C:-sized volume (3 million files).
#[test]
#[ignore = "measurement"]
fn link_set_memory() {
    const FILES: u64 = 3_000_000;
    let records = links::LinkSet::records();
    let ids = links::LinkSet::ids();
    for r in 0..FILES {
        // MFT records are dense, with a few gaps; sequence numbers vary.
        let id = (r + r / 8 + 16) | (r % 7) << 48;
        assert!(records.first_sighting(id, false));
        assert!(ids.first_sighting(id, false));
    }
    println!(
        "{FILES} files: record bitmap {:.2} MiB, id hash set {:.1} MiB",
        records.heap_bytes() as f64 / 1048576.0,
        ids.heap_bytes() as f64 / 1048576.0
    );
}

/// Walks C:\Windows (or TESSERA_BENCH_PATH) and prints the internals.
#[test]
#[ignore = "benchmark"]
fn walk_stats_for_windows_directory() {
    let path = std::env::var_os("TESSERA_BENCH_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let (t, summary, stats) = run(&path, 0, Tweaks::default());
    println!(
        "{}: {} nodes in {:.3} s, {stats:?}, tree {} KiB",
        path.display(),
        t.len(),
        summary.elapsed.as_secs_f64(),
        t.memory().total() / 1024
    );
}
