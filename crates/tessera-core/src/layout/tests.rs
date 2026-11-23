use std::collections::HashMap;

use super::*;
use crate::EntryBatch;

/// Small deterministic PRNG (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    /// Roughly log-uniform file sizes between 1 B and ~4 GiB.
    fn file_size(&mut self) -> u64 {
        let bits = self.below(33);
        1 + self.below(1u64 << bits)
    }
}

const EXTS: [&str; 8] = ["mp4", "jpg", "mp3", "zip", "exe", "pdf", "rs", "dat"];

/// A random tree: up to `dirs_per_dir` subdirectories and `files_per_dir`
/// files per directory, breadth first, until about `target` nodes; plus
/// `huge` directories of `huge_files` files each under the root.
fn synthetic(
    seed: u64,
    target: usize,
    dirs_per_dir: u64,
    files_per_dir: u64,
    huge: usize,
    huge_files: usize,
) -> Tree {
    use std::fmt::Write;
    let mut rng = Rng(seed | 1);
    let mut t = Tree::new(b"C:\\");
    let mut b = EntryBatch::new();
    let mut name = String::new();
    for h in 0..huge {
        b.clear();
        for i in 0..huge_files {
            let s = rng.file_size() / 64;
            name.clear();
            write!(name, "h{h}-{i}.{}", EXTS[i % EXTS.len()]).unwrap();
            b.push_wtf8(name.as_bytes(), s.div_ceil(4096) * 4096, s, NodeFlags::NONE);
        }
        let d = t.add_child(
            NodeId::ROOT,
            format!("huge{h}").as_bytes(),
            0,
            0,
            NodeFlags::DIR,
        );
        t.add_children(d, &b);
    }
    let mut queue = std::collections::VecDeque::from([NodeId::ROOT]);
    while let Some(dir) = queue.pop_front() {
        if t.len() >= target {
            break;
        }
        b.clear();
        let nd = match rng.below(dirs_per_dir + 1) {
            // The root always branches fully, and the walk never dies out.
            _ if dir == NodeId::ROOT => dirs_per_dir,
            0 if queue.is_empty() => 1,
            n => n,
        };
        let nf = rng.below(files_per_dir + 1);
        for i in 0..nd {
            b.push_wtf8(format!("d{i}").as_bytes(), 0, 0, NodeFlags::DIR);
        }
        for i in 0..nf {
            let s = rng.file_size();
            let flags = match rng.below(50) {
                0 => NodeFlags::HIDDEN,
                1 => NodeFlags::CLOUD,
                2 => NodeFlags::INACCESSIBLE,
                _ => NodeFlags::NONE,
            };
            name.clear();
            write!(name, "f{i}.{}", EXTS[rng.below(8) as usize]).unwrap();
            b.push_wtf8(name.as_bytes(), s.div_ceil(4096) * 4096, s, flags);
        }
        let r = t.add_children(dir, &b);
        queue.extend((r.start..r.start + nd as u32).map(NodeId));
    }
    t
}

fn lay(tree: &Tree, req: &LayoutRequest) -> Decoded {
    let buf = LayoutEngine::new().layout(tree, req, RootExtras::default(), 7);
    let d = decode(&buf).unwrap();
    check(tree, req, &d, 0);
    d
}

/// The category with the most bytes below `dir`, the slow way.
fn main_cat_below(tree: &Tree, dir: NodeId, mode: SizeMode) -> u8 {
    let mut bytes = [0u64; category::COUNT];
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for c in tree.children(d) {
            if tree.node(c).is_dir() {
                stack.push(c);
            } else {
                bytes[category::of_node(tree, c) as usize] += mode.of(&tree.node(c));
            }
        }
    }
    let best = (0..category::COUNT)
        .rev()
        .max_by_key(|&i| bytes[i])
        .unwrap();
    if bytes[best] > 0 {
        best as u8
    } else {
        category::NONE
    }
}

fn inner_of(r: &Record, st: &LayoutStyle) -> (f32, f32, f32, f32) {
    let top = if r.header > 0 { st.header } else { st.pad };
    (
        r.x + st.pad,
        r.y + top,
        r.w - 2.0 * st.pad,
        r.h - top - st.pad,
    )
}

/// Checks every rule that can be checked from the outside. `extras` is
/// the total size of the pseudo tiles at the top level.
fn check(tree: &Tree, req: &LayoutRequest, d: &Decoded, extras: u64) {
    check_level(tree, req, d, extras, None);
}

/// [`check`] for a level (rule 9) when `hide` is given.
fn check_level(tree: &Tree, req: &LayoutRequest, d: &Decoded, extras: u64, hide: Option<&[u32]>) {
    let hidden = |id: u32| hide.is_some_and(|h| h.binary_search(&id).is_ok());
    let st = &req.style;
    let mode = req.size_mode;
    let size = |n: u32| mode.of(&tree.node(NodeId(n)));
    let eps = 0.02f32;
    let mut index: HashMap<u32, usize> = HashMap::new();
    let mut kids: HashMap<u32, Vec<usize>> = HashMap::new();
    let o = st.outer;
    let map = (o, o, req.width - 2.0 * o, req.height - 2.0 * o);
    assert_eq!(d.view, req.view.0);
    for (i, r) in d.records.iter().enumerate() {
        assert!(index.insert(r.id, i).is_none(), "duplicate id {r:?}");
        assert!(r.w >= 1.0 && r.h >= 1.0, "{r:?} below 1 px");
        // minWidth is for folders only (rule 2)
        let folder = r.kind == kind::DIR || r.kind == kind::DIR_LEAF;
        let min_w = if folder {
            st.min_side.max(st.min_width)
        } else {
            st.min_side
        };
        assert!(
            r.w >= min_w - eps && r.h >= st.min_side - eps,
            "{r:?} smaller than minSide / minWidth"
        );
        let (px, py, pw, ph, pdepth) = if r.parent == ids::NONE {
            (map.0, map.1, map.2, map.3, 0)
        } else {
            let p = &d.records[*index.get(&r.parent).expect("parent after child")];
            assert_eq!(p.kind, kind::DIR, "parent of {r:?} is not open");
            let (x, y, w, h) = inner_of(p, st);
            (x, y, w, h, p.depth)
        };
        assert_eq!(r.depth, pdepth + 1);
        assert!(
            r.x >= px - eps
                && r.y >= py - eps
                && r.x + r.w <= px + pw + eps
                && r.y + r.h <= py + ph + eps,
            "{r:?} outside its parent's box {:?}",
            (px, py, pw, ph)
        );
        kids.entry(r.parent).or_default().push(i);
        if r.parent == ids::NONE {
            assert!(!hidden(r.id), "hidden {r:?} drawn");
        }
        // only a level's own group is drawn out of scale
        assert!(
            r.note == 0
                || (r.note == note::NOT_TO_SCALE
                    && r.kind == kind::GROUP
                    && r.parent == ids::NONE
                    && hide.is_some()),
            "{r:?}"
        );
        match r.kind {
            kind::FILE | kind::DIR | kind::DIR_LEAF => {
                assert!(r.id < ids::GROUP_BIT);
                assert!(!tree.is_deleted(NodeId(r.id)));
                assert_eq!(r.size, size(r.id));
                assert!(r.size > 0);
                assert_eq!(r.count, 0);
                let dir = tree.node(NodeId(r.id)).is_dir();
                assert_eq!(dir, r.kind != kind::FILE);
                if dir {
                    let cat = if r.kind == kind::DIR {
                        category::NONE
                    } else {
                        main_cat_below(tree, NodeId(r.id), mode)
                    };
                    assert_eq!(r.cat, cat, "{r:?}");
                    let header = r.h >= st.header * 2.2 && r.w >= st.min_header_w;
                    assert_eq!(r.header, if header { st.header as u8 } else { 0 }, "{r:?}");
                    // The target is the folder itself or a descendant (fold).
                    let t = NodeId(r.target);
                    assert!(t.0 == r.id || tree.ancestors(t).any(|a| a.0 == r.id));
                } else {
                    assert_eq!((r.header, r.target), (0, r.id));
                    assert_eq!(r.cat, category::of_node(tree, NodeId(r.id)));
                }
                let named = r.header > 0 || (r.w >= st.label_min_w && r.h >= st.label_min_h);
                assert_eq!(r.name.is_some(), named, "{r:?}");
            }
            kind::GROUP => {
                assert_eq!(r.id, ids::GROUP_BIT | r.target);
                assert!(tree.node(NodeId(r.target)).is_dir());
                assert!(r.count >= 1 && r.name.is_none() && r.header == 0);
            }
            kind::FREE => assert_eq!(
                (r.id, r.target, r.parent),
                (ids::FREE, ids::FREE, ids::NONE)
            ),
            kind::UNSCANNED => {
                assert_eq!(
                    (r.id, r.target, r.parent),
                    (ids::UNSCANNED, ids::UNSCANNED, ids::NONE)
                )
            }
            k => panic!("bad kind {k}"),
        }
    }
    for r in &d.records {
        let has_kids =
            kids.contains_key(&r.id) && r.kind != kind::FREE && r.kind != kind::UNSCANNED;
        assert_eq!(r.kind == kind::DIR, has_kids, "{r:?}");
        if r.kind == kind::DIR {
            let area = r.w * r.h;
            assert!(r.depth < st.max_depth, "{r:?}");
            assert!(
                r.header > 0 || st.header <= 0.0,
                "open without a header: {r:?}"
            );
            assert!(area > st.min_area * st.recurse, "{r:?}");
            assert!(area > st.open_share * map.2 * map.3, "{r:?}");
        }
    }
    for (&parent, group) in &kids {
        // Siblings don't overlap (a sweep along x).
        let mut by_x = group.clone();
        by_x.sort_by(|&i, &j| d.records[i].x.total_cmp(&d.records[j].x));
        for (a, &i) in by_x.iter().enumerate() {
            let r = &d.records[i];
            for &j in by_x[a + 1..]
                .iter()
                .take_while(|&&j| d.records[j].x < r.x + r.w - eps)
            {
                let s = &d.records[j];
                let ox = (r.x + r.w).min(s.x + s.w) - r.x.max(s.x);
                let oy = (r.y + r.h).min(s.y + s.h) - r.y.max(s.y);
                assert!(ox <= eps || oy <= eps, "{r:?} overlaps {s:?}");
            }
        }
        // Cell areas (rect + gap) are proportional to sizes.
        let (folder, area) = if parent == ids::NONE {
            (req.view, map.2 as f64 * map.3 as f64)
        } else {
            let p = &d.records[index[&parent]];
            let (_, _, w, h) = inner_of(p, st);
            (NodeId(p.target), w as f64 * h as f64)
        };
        let top = parent == ids::NONE;
        let total: u64 = tree
            .children(folder)
            .filter(|c| !(top && hidden(c.0)))
            .map(|c| mode.of(&tree.node(c)))
            .sum::<u64>()
            + if top { extras } else { 0 };
        // With minSide, tiles left out give their room to the others, so
        // the scale is the same for all siblings but can be larger.
        let scale = if st.min_side > 0.0 || st.min_width > 0.0 {
            let Some(r) = group
                .iter()
                .map(|&i| &d.records[i])
                .filter(|r| r.note & note::NOT_TO_SCALE == 0)
                .max_by_key(|r| r.size)
            else {
                continue;
            };
            let s = (r.w + st.gap) as f64 * (r.h + st.gap) as f64 / r.size as f64;
            // (a level's group drawn bigger than its size takes room)
            let off_scale = group
                .iter()
                .any(|&i| d.records[i].note & note::NOT_TO_SCALE != 0);
            assert!(
                off_scale || s >= area / total as f64 * 0.998,
                "{r:?} smaller than its share"
            );
            s
        } else {
            area / total as f64
        };
        for &i in group {
            let r = &d.records[i];
            let cell = (r.w + st.gap) as f64 * (r.h + st.gap) as f64;
            let expect = r.size as f64 * scale;
            if r.note & note::NOT_TO_SCALE != 0 {
                continue;
            }
            assert!(
                (cell - expect).abs() <= 0.002 * expect + 0.05 * (r.w + r.h) as f64,
                "{r:?}: cell {cell} but share {expect}"
            );
        }
    }
}

#[test]
fn buffer_conformance() {
    let mut t = Tree::new(b"C:\\");
    let d = t.add_child(NodeId::ROOT, b"Games", 0, 0, NodeFlags::DIR);
    let f = t.add_child(d, b"big.pak", 600 << 20, 600 << 20, NodeFlags::NONE);
    let year_ago = crate::age::now_filetime() - 365 * 864_000_000_000;
    assert!(t.set_modified(f, year_ago));
    let v = t.add_child(
        NodeId::ROOT,
        b"movie.mkv",
        400 << 20,
        400 << 20,
        NodeFlags::CLOUD,
    );
    let req = LayoutRequest::new(NodeId::ROOT, 800.0, 600.0);
    let buf = LayoutEngine::new().layout(&t, &req, RootExtras::default(), 42);

    // Header.
    assert_eq!(&buf[0..4], b"TSRL");
    assert_eq!(
        u32::from_le_bytes(buf[0..4].try_into().unwrap()),
        0x4C52_5354
    );
    let u16_at = |o: usize| u16::from_le_bytes(buf[o..o + 2].try_into().unwrap());
    let u32_at = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    let f32_at = |o: usize| f32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    assert_eq!(u16_at(4), 1);
    assert_eq!(u16_at(6), 36);
    let count = u32_at(8) as usize;
    assert_eq!(count, 3); // Games (with big.pak) and movie.mkv
    assert_eq!(u32_at(12), 56);
    let names_len = u32_at(16) as usize;
    assert_eq!(buf.len(), 36 + 56 * count + names_len);
    assert_eq!(u32_at(20), 0);
    assert_eq!(u32_at(24), 42);
    let map_area = (800.0 - 12.0) * (600.0 - 12.0);
    assert!((f32_at(28) - (1000u64 << 20) as f32 / map_area).abs() < 1.0);
    assert_eq!(u32_at(32), 0); // no search

    // Records, field by field.
    let rec = |i: usize| 36 + 56 * i;
    let names = &buf[36 + 56 * count..];
    let name_of = |o: usize| {
        let (off, len) = (u32_at(o + 40) as usize, u16_at(o + 44) as usize);
        std::str::from_utf8(&names[off..off + len])
            .unwrap()
            .to_owned()
    };
    let (g, m, p) = (rec(0), rec(1), rec(2));
    assert_eq!(u32_at(g), d.0);
    assert_eq!(u32_at(g + 4), ids::NONE);
    assert_eq!(u32_at(g + 8), d.0);
    assert_eq!(u32_at(g + 12), 0);
    assert_eq!((f32_at(g + 16), f32_at(g + 20)), (7.0, 7.0)); // outer + gap / 2
    assert!(f32_at(g + 24) > 100.0 && f32_at(g + 28) > 100.0);
    assert_eq!(
        u32_at(g + 32) as u64 | (u32_at(g + 36) as u64) << 32,
        600 << 20
    );
    assert_eq!(name_of(g), "Games");
    assert_eq!(buf[g + 46], 1); // depth
    assert_eq!(buf[g + 47], kind::DIR);
    assert_eq!(buf[g + 48], 255);
    assert_eq!(buf[g + 49], 0);
    assert_eq!(buf[g + 50], 21);
    assert_eq!(buf[g + 51], 0);
    // an open folder's age comes from what it holds
    let year = crate::age::byte_of_days(365.0);
    assert_eq!(buf[g + 52], year);
    assert_eq!(&buf[g + 53..g + 56], &[0, 0, 0]);

    assert_eq!(u32_at(m), v.0);
    assert_eq!(buf[m + 47], kind::FILE);
    assert_eq!(buf[m + 48], category::VIDEO);
    assert_eq!(buf[m + 49], flags::CLOUD);
    assert_eq!(buf[m + 52], crate::age::UNKNOWN);
    assert_eq!(name_of(m), "movie.mkv");

    assert_eq!(u32_at(p), f.0);
    assert_eq!(u32_at(p + 4), d.0);
    assert_eq!(buf[p + 46], 2);
    assert_eq!(buf[p + 48], category::GAME);
    assert_eq!(buf[p + 52], year);
    assert_eq!(name_of(p), "big.pak");
    // The child fills the folder's inner box, inset by gap / 2.
    let (gx, gy, gw, gh) = (
        f32_at(g + 16),
        f32_at(g + 20),
        f32_at(g + 24),
        f32_at(g + 28),
    );
    assert_eq!(
        (f32_at(p + 16), f32_at(p + 20)),
        (gx + 3.0 + 1.0, gy + 21.0 + 1.0)
    );
    assert!((f32_at(p + 24) - (gw - 6.0 - 2.0)).abs() < 1e-3);
    assert!((f32_at(p + 28) - (gh - 21.0 - 3.0 - 2.0)).abs() < 1e-3);

    let dec = decode(&buf).unwrap();
    assert_eq!(dec.records.len(), 3);
    assert_eq!(dec.rev, 42);
    check(&t, &req, &dec, 0);
    // Corrupt buffers are rejected.
    assert!(decode(&buf[..35]).is_err());
    assert!(decode(&buf[..buf.len() - 1]).is_err());
    let mut bad = buf.clone();
    bad[0] = b'X';
    assert!(decode(&bad).is_err());
    let mut bad = buf.clone();
    bad[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&bad).is_err());
}

#[test]
fn search_hits_go_into_the_records() {
    use crate::search::{Query, search, share_of};
    let mut t = Tree::new(b"C:\\");
    let d = t.add_child(NodeId::ROOT, b"Games", 0, 0, NodeFlags::DIR);
    let big = t.add_child(d, b"big.iso", 600 << 20, 600 << 20, NodeFlags::NONE);
    t.add_child(d, b"data.pak", 200 << 20, 200 << 20, NodeFlags::NONE);
    // many small files: they end up in a group, one in four matching
    let tiny = t.add_child(NodeId::ROOT, b"tiny", 0, 0, NodeFlags::DIR);
    for i in 0..400u64 {
        let name = if i % 4 == 0 {
            format!("{i}.iso")
        } else {
            format!("{i}.txt")
        };
        t.add_child(tiny, name.as_bytes(), 64 << 10, 64 << 10, NodeFlags::NONE);
    }
    t.add_child(tiny, b"huge.bin", 300 << 20, 300 << 20, NodeFlags::NONE);
    let q = Query::parse(".iso").unwrap().unwrap();
    let hits = search(&t, &q, SizeMode::Disk, 9, &|| false).unwrap();
    let req = LayoutRequest::new(NodeId::ROOT, 800.0, 600.0);
    let mut e = LayoutEngine::new();
    let dec = decode(&e.layout_with(&t, &req, RootExtras::default(), 1, Some(&hits))).unwrap();
    assert_eq!(dec.search, 9);
    let rec = |id: u32| dec.records.iter().find(|r| r.id == id).unwrap();
    assert_eq!(rec(big.0).hit, 255);
    assert_eq!(rec(d.0).hit, share_of(600 << 20, 800 << 20));
    let g = rec(ids::GROUP_BIT | tiny.0);
    assert_eq!(g.kind, kind::GROUP);
    assert!((60..=66).contains(&g.hit), "about a quarter: {}", g.hit);
    // the same picture with other hits is another picture
    let plain = e.layout(&t, &req, RootExtras::default(), 1);
    assert_eq!(decode(&plain).unwrap().search, 0);
    assert!(decode(&plain).unwrap().records.iter().all(|r| r.hit == 0));
    let with = e.layout_with(&t, &req, RootExtras::default(), 1, Some(&hits));
    assert!(!same_picture(&plain, &with, 0.5));
}

#[test]
fn random_trees_are_well_formed() {
    for seed in 1..5u64 {
        let t = synthetic(seed, 4_000, 5, 12, 1, 600);
        for (w, h) in [
            (800.0, 600.0),
            (1920.0, 1080.0),
            (300.0, 900.0),
            (37.0, 23.0),
        ] {
            for mode in [SizeMode::Disk, SizeMode::Logical] {
                let req = LayoutRequest {
                    size_mode: mode,
                    ..LayoutRequest::new(NodeId::ROOT, w, h)
                };
                let a = lay(&t, &req);
                if w > 100.0 {
                    assert!(a.records.len() > 20, "{}", a.records.len());
                }
                // Deterministic, also for a fresh engine.
                assert_eq!(lay(&t, &req), a);
            }
        }
    }
}

#[test]
fn styles_are_respected() {
    let t = synthetic(8, 6_000, 4, 15, 1, 300);
    let style = LayoutStyle {
        outer: 0.0,
        pad: 6.0,
        gap: 0.0,
        header: 30.0,
        min_header_w: 100.0,
        min_area: 400.0,
        min_side: 30.0,
        min_width: 50.0,
        recurse: 1.0,
        open_share: 0.0,
        max_depth: 3,
        max_per_dir: 5,
        label_min_w: 80.0,
        label_min_h: 40.0,
    };
    let req = LayoutRequest {
        style,
        ..LayoutRequest::new(NodeId::ROOT, 1500.0, 1000.0)
    };
    let d = lay(&t, &req);
    assert!(d.records.iter().all(|r| r.depth <= 3));
    let mut per_parent: HashMap<u32, usize> = HashMap::new();
    for r in &d.records {
        if r.kind != kind::GROUP {
            *per_parent.entry(r.parent).or_default() += 1;
        }
    }
    assert!(per_parent.values().all(|&n| n <= 5), "{per_parent:?}");
    assert!(d.records.iter().any(|r| r.header == 30));
    let folder = |r: &Record| r.kind == kind::DIR || r.kind == kind::DIR_LEAF;
    assert!(
        d.records
            .iter()
            .all(|r| r.w >= if folder(r) { 50.0 } else { 30.0 } && r.h >= 30.0)
    );
}

#[test]
fn size_labels_match_the_ui_formatter() {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    // fmtBytes: 1023 字节 / 1 KB / 1023 KB / 1.0 MB / 9.9 MB / 10 MB / 1.0 GB / 99.9 GB / 100 GB / 1.00 TB
    assert_eq!(size_label(1023), (6, 1023));
    assert_eq!(size_label(KB + 500), (5, 1));
    assert_eq!(size_label(KB + 520), (5, 2));
    assert_eq!(size_label(MB - 1), (5, 1024)); // "1024 KB", as fmtBytes shows it
    assert_eq!(size_label(MB), (4, 10));
    assert_eq!(size_label(10 * MB - 1), (4, 100)); // "10.0 MB"
    assert_eq!(size_label(10 * MB), (3, 10));
    assert_eq!(size_label(GB), (2, 10));
    assert_eq!(size_label(100 * GB - GB / 20), (2, 1000));
    assert_eq!(size_label(100 * GB), (1, 100));
    assert_eq!(size_label(1024 * GB), (0, 100));
    // the same label, however the bytes differ
    assert_eq!(size_label(25 * GB + MB), size_label(25 * GB + 40 * MB));
    assert_ne!(size_label(25 * GB), size_label(25 * GB + 60 * MB));
}

#[test]
fn same_picture_ignores_changes_too_small_to_see() {
    let mut t = synthetic(11, 3000, 4, 12, 0, 0);
    let req = LayoutRequest::new(NodeId::ROOT, 1280.0, 800.0);
    let mut eng = LayoutEngine::new();
    let a = eng.layout(&t, &req, RootExtras::default(), 1);
    // the same tree, another revision: the same picture
    let again = eng.layout(&t, &req, RootExtras::default(), 2);
    assert!(same_picture(&a, &again, 0.5));
    assert!(header_bytes_per_px(&a).unwrap() > 0.0);

    // a file that is not drawn on its own grows by a few bytes per px²
    let d = decode(&a).unwrap();
    let shown: std::collections::HashSet<u32> = d.records.iter().map(|r| r.id).collect();
    let bpp = header_bytes_per_px(&a).unwrap() as f64;
    let hidden = (1..t.len() as u32)
        .map(NodeId)
        .find(|&n| {
            !t.node(n).is_dir() && !shown.contains(&n.0) && !shown.contains(&t.node(n).parent.0)
        })
        .expect("a file below what is drawn");
    let n = t.node(hidden);
    let grow = (bpp * 0.2) as u64; // a fifth of a px²
    t.set_file_size(hidden, n.size() + grow, n.logical() + grow);
    let b = eng.layout(&t, &req, RootExtras::default(), 3);
    assert!(same_picture(&a, &b, 0.5), "a fifth of a px² must not count");

    // the biggest named tile doubles: that shows
    let big = d
        .records
        .iter()
        .filter(|r| r.kind == 0 && r.name.is_some() && (r.id as usize) < t.len())
        .max_by_key(|r| r.size)
        .expect("a named file tile");
    let n = t.node(NodeId(big.id));
    t.set_file_size(NodeId(big.id), n.size() * 2, n.logical() * 2);
    let c = eng.layout(&t, &req, RootExtras::default(), 4);
    assert!(!same_picture(&a, &c, 0.5));
    // a differently sized canvas is another picture
    let wide = LayoutRequest::new(NodeId::ROOT, 1300.0, 800.0);
    assert!(!same_picture(
        &c,
        &eng.layout(&t, &wide, RootExtras::default(), 5),
        0.5
    ));
    // garbage never matches
    assert!(!same_picture(&a, &a[..10], 0.5));
}

#[test]
fn hysteresis_keeps_the_order_calm() {
    let mut t = Tree::new(b"R:\\");
    let a = t.add_child(NodeId::ROOT, b"a.bin", 100_000, 100_000, NodeFlags::NONE);
    let b = t.add_child(NodeId::ROOT, b"b.bin", 90_000, 90_000, NodeFlags::NONE);
    let req = LayoutRequest::new(NodeId::ROOT, 400.0, 300.0);
    let mut eng = LayoutEngine::new();
    let first = |t: &Tree, eng: &mut LayoutEngine| {
        let d = decode(&eng.layout(t, &req, RootExtras::default(), 0)).unwrap();
        check(t, &req, &d, 0);
        d.records[0].id
    };
    assert_eq!(first(&t, &mut eng), a.0);
    t.set_file_size(b, 110_000, 110_000); // +22 %: not re-ranked
    assert_eq!(first(&t, &mut eng), a.0);
    t.set_file_size(b, 120_000, 120_000); // +33 % from 90,000: re-ranked
    assert_eq!(first(&t, &mut eng), b.0);
    t.set_file_size(b, 100_001, 100_001); // -17 % from 120,000: stays first
    assert_eq!(first(&t, &mut eng), b.0);
    // A fresh ranking orders by the current size again.
    eng.reset();
    assert_eq!(first(&t, &mut eng), b.0);
    t.set_file_size(b, 99_000, 99_000);
    eng.reset();
    assert_eq!(first(&t, &mut eng), a.0);
}

#[test]
fn a_group_of_folders_is_tinted_by_what_fills_them() {
    // a big file and many small folders: the folders become one group
    let mut t = Tree::new(b"R:\\");
    t.add_child(NodeId::ROOT, b"big.iso", 1 << 30, 1 << 30, NodeFlags::NONE);
    for i in 0..40 {
        let d = t.add_child(
            NodeId::ROOT,
            format!("app{i}").as_bytes(),
            0,
            0,
            NodeFlags::DIR,
        );
        let sub = t.add_child(d, b"bin", 0, 0, NodeFlags::DIR);
        // each folder holds more bytes of programs than of images
        t.add_child(sub, b"tool.exe", 3 << 20, 3 << 20, NodeFlags::NONE);
        t.add_child(d, b"logo.png", 2 << 20, 2 << 20, NodeFlags::NONE);
    }
    let req = LayoutRequest::new(NodeId::ROOT, 800.0, 600.0);
    let d = lay(&t, &req);
    let group = d.records.iter().find(|r| r.kind == kind::GROUP).unwrap();
    assert!(group.count > 1);
    assert_eq!(group.cat, category::APP);
}

#[test]
fn small_and_surplus_children_are_grouped() {
    let mut t = Tree::new(b"R:\\");
    let d = t.add_child(NodeId::ROOT, b"dir", 0, 0, NodeFlags::DIR);
    t.add_child(d, b"big.iso", 100_000, 100_000, NodeFlags::NONE);
    let mut b = EntryBatch::new();
    for i in 0..600 {
        b.push_wtf8(format!("s{i}.mp3").as_bytes(), 10, 10, NodeFlags::NONE);
    }
    for i in 0..400 {
        b.push_wtf8(format!("p{i}.JPG").as_bytes(), 20, 20, NodeFlags::NONE);
    }
    b.push_wtf8(b"empty.txt", 0, 0, NodeFlags::NONE);
    t.add_children(d, &b);
    t.add_child(
        NodeId::ROOT,
        b"other.bin",
        100_000,
        100_000,
        NodeFlags::NONE,
    );
    let mut req = LayoutRequest::new(NodeId::ROOT, 800.0, 600.0);
    req.style.min_side = 0.0;
    req.style.min_width = 0.0;
    let dd = lay(&t, &req);
    let group = dd.records.iter().find(|r| r.kind == kind::GROUP).unwrap();
    assert_eq!(group.id, ids::GROUP_BIT | d.0);
    assert_eq!((group.target, group.parent, group.count), (d.0, d.0, 1000));
    assert_eq!(group.size, 600 * 10 + 400 * 20);
    assert_eq!(group.cat, category::IMAGE); // 8,000 bytes of images vs 6,000 of audio
    // The group comes after the folder's other children.
    let in_dir: Vec<&Record> = dd.records.iter().filter(|r| r.parent == d.0).collect();
    assert_eq!(in_dir.last().unwrap().kind, kind::GROUP);

    // maxPerDir: the surplus joins the group even when big enough.
    let mut t = Tree::new(b"R:\\");
    let mut b = EntryBatch::new();
    for i in 0..50u64 {
        b.push_wtf8(
            format!("f{i}.zip").as_bytes(),
            1000 + i,
            1000 + i,
            NodeFlags::NONE,
        );
    }
    t.add_children(NodeId::ROOT, &b);
    let mut req = LayoutRequest::new(NodeId::ROOT, 1000.0, 800.0);
    req.style.max_per_dir = 10;
    req.style.min_side = 0.0;
    req.style.min_width = 0.0;
    let dd = lay(&t, &req);
    assert_eq!(dd.records.len(), 11);
    let g = dd.records.last().unwrap();
    assert_eq!(
        (g.kind, g.count, g.cat, g.parent),
        (kind::GROUP, 40, category::ARCHIVE, ids::NONE)
    );
    // The ten largest are shown, largest first.
    let shown: Vec<u64> = dd.records[..10].iter().map(|r| r.size).collect();
    assert_eq!(shown, (1040..1050).rev().collect::<Vec<u64>>());
}

#[test]
fn a_lone_small_child_is_shown_as_itself() {
    let mut t = Tree::new(b"R:\\");
    t.add_child(NodeId::ROOT, b"big", 10_000_000, 1, NodeFlags::NONE);
    let small = t.add_child(NodeId::ROOT, b"small", 400, 1, NodeFlags::NONE);
    let req = LayoutRequest::new(NodeId::ROOT, 1000.0, 1000.0);
    let d = lay(&t, &req);
    assert!(d.records.iter().all(|r| r.kind != kind::GROUP));
    // 400 / 10M of ~976,000 px² is 0.04 px²: dropped as below 1 px.
    assert!(d.records.iter().all(|r| r.id != small.0));
    assert_eq!(d.records.len(), 1);
}

#[test]
fn single_folder_chains_fold() {
    let mut t = Tree::new(b"R:\\");
    let a = t.add_child(NodeId::ROOT, b"a", 0, 0, NodeFlags::DIR);
    let b = t.add_child(a, b"b", 0, 0, NodeFlags::DIR);
    let c = t.add_child(b, b"c", 0, 0, NodeFlags::DIR);
    t.add_child(b, b"empty", 0, 0, NodeFlags::NONE); // size 0: invisible
    let mut batch = EntryBatch::new();
    for i in 0..3u64 {
        batch.push_wtf8(
            format!("x{i}.mp4").as_bytes(),
            1 << (20 + i),
            1,
            NodeFlags::NONE,
        );
    }
    let files = t.add_children(c, &batch);
    t.add_child(NodeId::ROOT, b"z.bin", 1 << 21, 1, NodeFlags::NONE);
    let req = LayoutRequest::new(NodeId::ROOT, 900.0, 700.0);
    let d = lay(&t, &req);
    let tile = d.records.iter().find(|r| r.id == a.0).unwrap();
    assert_eq!(tile.kind, kind::DIR);
    assert_eq!(tile.target, c.0);
    assert_eq!(tile.name.as_deref(), Some("a \u{203A} b \u{203A} c"));
    assert!(d.records.iter().all(|r| r.id != b.0 && r.id != c.0));
    let inside: Vec<u32> = d
        .records
        .iter()
        .filter(|r| r.parent == a.0)
        .map(|r| r.id)
        .collect();
    assert_eq!(inside, files.clone().rev().collect::<Vec<u32>>());
    assert!(
        d.records
            .iter()
            .filter(|r| r.parent == a.0)
            .all(|r| r.depth == 2)
    );

    // Navigation skips the folded folders.
    let n = nav(&t, NodeId(files.start), SizeMode::Disk);
    let crumbs: Vec<&str> = n.crumbs.iter().map(|(_, s)| s.as_str()).collect();
    assert_eq!(crumbs, ["R:\\", "c", "x0.mp4"]);
    assert_eq!(n.up, Some(c));
    let n = nav(&t, c, SizeMode::Disk);
    assert_eq!(n.up, Some(NodeId::ROOT));
    assert_eq!(nav(&t, NodeId::ROOT, SizeMode::Disk).up, None);
}

/// A chain of 20 folders, each with a file beside the next folder that
/// takes three quarters of it, so the area quarters at every level.
fn chain_tree() -> (Tree, Vec<NodeId>) {
    let mut t = Tree::new(b"R:\\");
    let mut cur = NodeId::ROOT;
    let mut chain = vec![];
    for i in 0..20 {
        cur = t.add_child(cur, format!("d{i}").as_bytes(), 0, 0, NodeFlags::DIR);
        chain.push(cur);
        // the largest is 3 << 38 bytes: node sizes hold 48 bits
        let size = 3u64 << (2 * (19 - i));
        t.add_child(cur, format!("f{i}").as_bytes(), size, 1, NodeFlags::NONE);
    }
    t.add_child(
        NodeId::ROOT,
        b"tiny-dir-sibling",
        1 << 10,
        1,
        NodeFlags::NONE,
    );
    (t, chain)
}

#[test]
fn folders_open_by_size_and_depth() {
    let (t, chain) = chain_tree();
    let mut req = LayoutRequest::new(NodeId::ROOT, 3000.0, 3000.0);
    req.style.max_depth = 4;
    // (the deepest folders get too small for minSide / minWidth)
    req.style.min_side = 0.0;
    req.style.min_width = 0.0;
    let d = lay(&t, &req);
    let by_id: HashMap<u32, &Record> = d.records.iter().map(|r| (r.id, r)).collect();
    assert_eq!(by_id[&chain[0].0].kind, kind::DIR);
    assert_eq!(by_id[&chain[2].0].kind, kind::DIR);
    assert_eq!(by_id[&chain[3].0].depth, 4);
    assert_eq!(by_id[&chain[3].0].kind, kind::DIR_LEAF, "max depth reached");
    assert!(!by_id.contains_key(&chain[4].0));
    // Deep enough without a depth limit, the folders get too small.
    req.style.max_depth = 255;
    let d = lay(&t, &req);
    let last = d
        .records
        .iter()
        .filter(|r| r.kind == kind::DIR || r.kind == kind::DIR_LEAF)
        .max_by_key(|r| r.depth)
        .unwrap();
    assert_eq!(last.kind, kind::DIR_LEAF);
    assert!(last.depth < 20);
}

#[test]
fn open_share_keeps_the_depth_when_the_window_grows() {
    let (t, chain) = chain_tree();
    let open = |side: f32, share: f32| {
        let mut req = LayoutRequest::new(NodeId::ROOT, side, side);
        req.style.recurse = 1.0;
        req.style.open_share = share;
        req.style.max_depth = 255;
        let d = lay(&t, &req);
        let ids: Vec<u32> = d
            .records
            .iter()
            .filter(|r| r.kind == kind::DIR)
            .map(|r| r.id)
            .collect();
        ids
    };
    // Folders at 100 %, 25 %, 6.25 % and 1.6 % of the map open, the one
    // at 0.4 % does not.
    let small = open(1500.0, 0.01);
    assert_eq!(small, [chain[0].0, chain[1].0, chain[2].0, chain[3].0]);
    assert_eq!(open(6000.0, 0.01), small);
    // Without the share, a bigger window opens more levels.
    assert!(open(6000.0, 0.0).len() > open(1500.0, 0.0).len());
}

#[test]
fn closed_folders_take_their_main_category() {
    let mut t = Tree::new(b"R:\\");
    let media = t.add_child(NodeId::ROOT, b"media", 0, 0, NodeFlags::DIR);
    let deep = t.add_child(media, b"deep", 0, 0, NodeFlags::DIR);
    t.add_child(deep, b"a.mp4", 1000, 10, NodeFlags::NONE);
    let jpg = t.add_child(media, b"b.jpg", 990, 990, NodeFlags::NONE);
    // A tie goes to the lower category (audio before archive).
    let tie = t.add_child(NodeId::ROOT, b"tie", 0, 0, NodeFlags::DIR);
    t.add_child(tie, b"c.zip", 700, 700, NodeFlags::NONE);
    t.add_child(tie, b"d.mp3", 700, 700, NodeFlags::NONE);
    // Too small to open, so both folders are closed.
    let mut req = LayoutRequest::new(NodeId::ROOT, 100.0, 100.0);
    req.style.min_width = 0.0;
    let mut eng = LayoutEngine::new();
    let cat_of = |eng: &mut LayoutEngine, t: &Tree, req: &LayoutRequest, id: NodeId| {
        let d = decode(&eng.layout(t, req, RootExtras::default(), 0)).unwrap();
        let r = d.records.iter().find(|r| r.id == id.0).unwrap();
        assert_eq!(r.kind, kind::DIR_LEAF);
        r.cat
    };
    assert_eq!(cat_of(&mut eng, &t, &req, media), category::VIDEO);
    assert_eq!(cat_of(&mut eng, &t, &req, tie), category::AUDIO);
    // By logical size, the 10-byte video loses.
    let logical = LayoutRequest {
        size_mode: SizeMode::Logical,
        ..req
    };
    assert_eq!(cat_of(&mut eng, &t, &logical, media), category::IMAGE);
    assert_eq!(cat_of(&mut eng, &t, &req, media), category::VIDEO);
    // Within 1/8 of the size it was found at, the answer is reused...
    t.set_file_size(jpg, 1100, 1100);
    assert_eq!(cat_of(&mut eng, &t, &req, media), category::VIDEO);
    // ...beyond that, the folder is walked again.
    t.set_file_size(jpg, 2000, 2000);
    assert_eq!(cat_of(&mut eng, &t, &req, media), category::IMAGE);
    // A fresh engine always walks.
    t.set_file_size(jpg, 1100, 1100);
    assert_eq!(
        cat_of(&mut LayoutEngine::new(), &t, &req, media),
        category::IMAGE
    );
}

#[test]
fn only_folders_with_a_header_open() {
    let t = synthetic(5, 3_000, 4, 10, 0, 0);
    let mut req = LayoutRequest::new(NodeId::ROOT, 1600.0, 1000.0);
    let d = lay(&t, &req);
    assert!(d.records.iter().any(|r| r.kind == kind::DIR));
    // No folder is wide enough for a header, so none opens.
    req.style.min_header_w = 10_000.0;
    let d = lay(&t, &req);
    assert!(
        d.records
            .iter()
            .all(|r| r.depth == 1 && r.kind != kind::DIR)
    );
    // Without header bands at all, the rule is off.
    req.style.header = 0.0;
    let d = lay(&t, &req);
    assert!(
        d.records
            .iter()
            .any(|r| r.kind == kind::DIR && r.header == 0)
    );
}

#[test]
fn pseudo_tiles_at_the_volume_root() {
    let t = synthetic(4, 2_000, 4, 10, 0, 0);
    let req = LayoutRequest::new(NodeId::ROOT, 1200.0, 800.0);
    let used = t.node(NodeId::ROOT).size();
    let extras = RootExtras {
        free: Some(used / 2),
        unscanned: Some(used / 3),
    };
    let mut eng = LayoutEngine::new();
    let d = decode(&eng.layout(&t, &req, extras, 0)).unwrap();
    check(&t, &req, &d, used / 2 + used / 3);
    assert_eq!(d.records[0].kind, kind::UNSCANNED);
    assert_eq!(d.records[0].size, used / 3);
    let top: Vec<&Record> = d.records.iter().filter(|r| r.depth == 1).collect();
    assert_eq!(top.last().unwrap().kind, kind::FREE);
    assert_eq!(top.last().unwrap().size, used / 2);
    let map = (1200.0 - 12.0) * (800.0 - 12.0);
    let bpp = (used + used / 2 + used / 3) as f64 / map;
    assert!((d.bytes_per_px as f64 - bpp).abs() / bpp < 1e-5);

    // Below 64 MiB, UNSCANNED is left out.
    let small = RootExtras {
        free: None,
        unscanned: Some(UNSCANNED_MIN),
    };
    let d = decode(&eng.layout(&t, &req, small, 0)).unwrap();
    check(&t, &req, &d, 0);
    assert!(
        d.records
            .iter()
            .all(|r| r.kind != kind::UNSCANNED && r.kind != kind::FREE)
    );
    // Elsewhere than at the root, never.
    let sub = t
        .children(NodeId::ROOT)
        .find(|&c| t.node(c).is_dir())
        .unwrap();
    let req = LayoutRequest::new(sub, 1200.0, 800.0);
    let d = decode(&eng.layout(&t, &req, extras, 0)).unwrap();
    check(&t, &req, &d, 0);
    assert!(
        d.records
            .iter()
            .all(|r| r.kind != kind::UNSCANNED && r.kind != kind::FREE)
    );
}

#[test]
fn flags_names_and_skipped_nodes() {
    let mut t = Tree::new(b"C:\\");
    let bin = t.add_child(
        NodeId::ROOT,
        b"$RECYCLE.BIN",
        0,
        0,
        NodeFlags::DIR | NodeFlags::HIDDEN,
    );
    t.add_child(bin, b"$R1.dat", 1 << 30, 1 << 30, NodeFlags::NONE);
    let locked = t.add_child(
        NodeId::ROOT,
        b"locked",
        1 << 29,
        1 << 29,
        NodeFlags::INACCESSIBLE,
    );
    let link = t.add_child(NodeId::ROOT, b"link", 1 << 28, 1 << 28, NodeFlags::REPARSE);
    let odd = t.add_child(
        NodeId::ROOT,
        &[b'x', 0xED, 0xA0, 0x80, b'.', b'p', b'd', b'f'],
        1 << 29,
        1,
        NodeFlags::NONE,
    );
    let zero = t.add_child(NodeId::ROOT, b"zero", 0, 0, NodeFlags::NONE);
    let gone = t.add_child(NodeId::ROOT, b"gone", 1 << 29, 1 << 29, NodeFlags::NONE);
    let tiny = t.add_child(NodeId::ROOT, b"tiny", 1 << 21, 1, NodeFlags::NONE);
    assert!(t.mark_deleted(gone));
    let mut req = LayoutRequest::new(NodeId::ROOT, 1000.0, 800.0);
    req.style.min_side = 0.0;
    req.style.min_width = 0.0;
    let d = lay(&t, &req);
    let get = |id: NodeId| d.records.iter().find(|r| r.id == id.0);
    assert_eq!(get(bin).unwrap().flags, flags::RECYCLE_BIN);
    assert_eq!(get(locked).unwrap().flags, flags::NO_ACCESS);
    assert_eq!(get(link).unwrap().flags, flags::REPARSE);
    assert_eq!(get(odd).unwrap().name.as_deref(), Some("x\u{FFFD}.pdf"));
    assert_eq!(get(odd).unwrap().cat, category::DOC);
    assert!(get(zero).is_none() && get(gone).is_none());
    // Too small for a label: no name.
    let tr = get(tiny).unwrap();
    assert!(tr.w < 44.0 || tr.h < 20.0);
    assert_eq!(tr.name, None);
    // Only folders with a header are named when labels need a lot of room.
    req.style.label_min_w = 5000.0;
    let d = lay(&t, &req);
    assert!(d.records.iter().all(|r| r.name.is_some() == (r.header > 0)));
    // A folder below a volume root is not the recycle bin.
    let mut t2 = Tree::new(b"D:\\Data");
    let b2 = t2.add_child(NodeId::ROOT, b"$Recycle.Bin", 0, 0, NodeFlags::DIR);
    t2.add_child(b2, b"x", 1 << 20, 1, NodeFlags::NONE);
    let d = lay(&t2, &LayoutRequest::new(NodeId::ROOT, 500.0, 400.0));
    assert_eq!(d.records[0].flags & flags::RECYCLE_BIN, 0);
}

#[cfg(not(windows))]
#[test]
fn protected_flag() {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return;
    };
    let comps: Vec<String> = home
        .components()
        .skip(1)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if comps.is_empty() {
        return;
    }
    let mut t = Tree::new(b"/");
    let first = t.add_child(NodeId::ROOT, comps[0].as_bytes(), 0, 0, NodeFlags::DIR);
    let mut cur = first;
    for c in &comps[1..] {
        cur = t.add_child(cur, c.as_bytes(), 0, 0, NodeFlags::DIR);
    }
    t.add_child(cur, b"inside.bin", 1 << 30, 1, NodeFlags::NONE);
    let free = t.add_child(
        NodeId::ROOT,
        b"tessera-elsewhere.bin",
        1 << 30,
        1,
        NodeFlags::NONE,
    );
    let d = lay(&t, &LayoutRequest::new(NodeId::ROOT, 800.0, 600.0));
    let get = |id: NodeId| d.records.iter().find(|r| r.id == id.0).unwrap();
    assert_eq!(get(first).flags & flags::PROTECTED, flags::PROTECTED);
    assert_eq!(get(free).flags & flags::PROTECTED, 0);
}

#[cfg(windows)]
#[test]
fn protected_flag() {
    let Some(windir) = std::env::var_os("SystemRoot").map(std::path::PathBuf::from) else {
        return;
    };
    let drive = windir.parent().unwrap().to_string_lossy().into_owned();
    let name = windir.file_name().unwrap().to_string_lossy().into_owned();
    let mut t = Tree::new(drive.as_bytes());
    let w = t.add_child(NodeId::ROOT, name.as_bytes(), 0, 0, NodeFlags::DIR);
    let temp = t.add_child(w, b"Temp", 0, 0, NodeFlags::DIR);
    t.add_child(temp, b"x.tmp", 1 << 30, 1, NodeFlags::NONE);
    t.add_child(w, b"notepad.exe", 1 << 29, 1, NodeFlags::NONE); // no folding
    let other = t.add_child(
        NodeId::ROOT,
        b"tessera-elsewhere",
        1 << 30,
        1,
        NodeFlags::NONE,
    );
    let d = lay(&t, &LayoutRequest::new(NodeId::ROOT, 800.0, 600.0));
    let get = |id: NodeId| d.records.iter().find(|r| r.id == id.0).unwrap();
    assert_eq!(get(w).flags & flags::PROTECTED, flags::PROTECTED);
    assert_eq!(get(temp).flags & flags::PROTECTED, 0);
    assert_eq!(get(other).flags & flags::PROTECTED, 0);
}

#[test]
fn bad_views_give_empty_buffers() {
    let mut t = synthetic(3, 500, 3, 5, 0, 0);
    let file = t
        .children(NodeId::ROOT)
        .find(|&c| !t.node(c).is_dir())
        .unwrap();
    let dir = t
        .children(NodeId::ROOT)
        .find(|&c| t.node(c).is_dir())
        .unwrap();
    t.mark_deleted(dir);
    let mut eng = LayoutEngine::new();
    for (view, w, h) in [
        (file, 800.0, 600.0),
        (dir, 800.0, 600.0),
        (NodeId(u32::MAX - 5), 800.0, 600.0),
        (NodeId::ROOT, 10.0, 600.0),
        (NodeId::ROOT, f32::NAN, 600.0),
    ] {
        let buf = eng.layout(
            &t,
            &LayoutRequest::new(view, w, h),
            RootExtras::default(),
            3,
        );
        let d = decode(&buf).unwrap();
        assert!(d.records.is_empty() && buf.len() == HEADER_SIZE);
        assert_eq!((d.view, d.rev), (view.0, 3));
    }
}

#[test]
fn record_cap_keeps_the_big_structure() {
    let mut t = Tree::new(b"R:\\");
    let mut b = EntryBatch::new();
    for i in 0..60_000u64 {
        b.push_wtf8(format!("f{i}").as_bytes(), 1000 + i % 7, 1, NodeFlags::NONE);
    }
    for d in 0..3 {
        let dir = t.add_child(
            NodeId::ROOT,
            format!("d{d}").as_bytes(),
            0,
            0,
            NodeFlags::DIR,
        );
        t.add_children(dir, &b);
    }
    let mut req = LayoutRequest::new(NodeId::ROOT, 8000.0, 8000.0);
    req.style.min_area = 1.0;
    req.style.min_side = 0.0;
    req.style.min_width = 0.0;
    req.style.max_per_dir = u32::MAX;
    let d = lay(&t, &req);
    assert!(d.records.len() <= MAX_RECORDS);
    let top: Vec<&Record> = d.records.iter().filter(|r| r.depth == 1).collect();
    assert_eq!(top.len(), 3);
    assert_eq!(top.iter().filter(|r| r.kind == kind::DIR).count(), 1);
    assert_eq!(top.iter().filter(|r| r.kind == kind::DIR_LEAF).count(), 2);

    // A flat view over the cap: the surplus goes into the group.
    let mut t = Tree::new(b"R:\\");
    b.clear();
    for i in 0..120_000u64 {
        b.push_wtf8(format!("f{i}").as_bytes(), 1000 + i % 7, 1, NodeFlags::NONE);
    }
    t.add_children(NodeId::ROOT, &b);
    let d = lay(&t, &req);
    assert!(d.records.len() <= MAX_RECORDS);
    let g = d.records.last().unwrap();
    assert_eq!(g.kind, kind::GROUP);
    assert_eq!(g.count as usize, 120_000 - (MAX_RECORDS - 3));
}

/// The relayout budget is 500 ms; this must stay in the low
/// milliseconds. Run with
/// `cargo test -p tessera-core --release -- --ignored --nocapture layout_bench`.
#[test]
#[ignore = "benchmark; run in release mode"]
fn layout_bench() {
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    // ~3 million nodes: ~300k directories about 11 levels deep with up
    // to 40 files each, and five directories of 100,000 files at the root.
    let t = synthetic(42, 3_000_000, 6, 40, 5, 100_000);
    let dirs = (0..t.len() as u32)
        .filter(|&i| t.node(NodeId(i)).is_dir())
        .count();
    let mut max_depth = 0;
    for i in (0..t.len() as u32).step_by(997) {
        max_depth = max_depth.max(t.ancestors(NodeId(i)).count());
    }
    println!(
        "tree: {} nodes, {dirs} directories, depth >= {max_depth}, built in {:?}",
        t.len(),
        t0.elapsed()
    );
    let huge = t
        .children(NodeId::ROOT)
        .find(|&c| t.name(c) == b"huge0")
        .unwrap();
    // Six levels down the chain of largest directories.
    let mut deep = NodeId::ROOT;
    for _ in 0..6 {
        if let Some(c) = t
            .children(deep)
            .filter(|&c| t.node(c).is_dir() && !t.name(c).starts_with(b"huge"))
            .max_by_key(|&c| t.node(c).size())
        {
            deep = c;
        }
    }
    let mut worst = Duration::ZERO;
    for (what, view) in [
        ("root", NodeId::ROOT),
        ("100k-file dir", huge),
        ("deep dir", deep),
    ] {
        for (w, h) in [(1920.0, 1080.0), (3840.0, 2160.0)] {
            let req = LayoutRequest::new(view, w, h);
            let mut eng = LayoutEngine::new();
            let s = Instant::now();
            let cold = eng.layout(&t, &req, RootExtras::default(), 0);
            let cold_t = s.elapsed();
            let mut best = Duration::MAX;
            for _ in 0..5 {
                let s = Instant::now();
                let buf = eng.layout(&t, &req, RootExtras::default(), 0);
                best = best.min(s.elapsed());
                assert_eq!(buf, cold);
            }
            worst = worst.max(best);
            let d = decode(&cold).unwrap();
            let groups = d.records.iter().filter(|r| r.kind == kind::GROUP).count();
            let named = d.records.iter().filter(|r| r.name.is_some()).count();
            println!(
                "layout {what:>13} at {w}x{h}: {:>7.3} ms (first {:>7.3} ms), {} records \
                 ({groups} groups, {named} named), {} KiB",
                best.as_secs_f64() * 1e3,
                cold_t.as_secs_f64() * 1e3,
                d.records.len(),
                cold.len() / 1024
            );
        }
    }
    if !cfg!(debug_assertions) {
        assert!(worst < Duration::from_millis(50), "layout took {worst:?}");
    }
}

#[test]
fn min_width_is_for_folders() {
    // Big files, then folders and files of middling sizes side by side.
    let mut t = Tree::new(b"R:\\");
    let d = t.add_child(NodeId::ROOT, b"d", 0, 0, NodeFlags::DIR);
    for (i, s) in [500u64, 200].into_iter().enumerate() {
        let name = format!("big{i}.mkv");
        t.add_child(d, name.as_bytes(), s << 20, s << 20, NodeFlags::NONE);
    }
    for i in 0..80u64 {
        let size = (2 + i % 7) << 20;
        let sub = format!("sub{i}");
        let f = t.add_child(d, sub.as_bytes(), 0, 0, NodeFlags::DIR);
        t.add_child(f, b"a.bin", size, size, NodeFlags::NONE);
        let file = format!("file{i}.iso");
        t.add_child(d, file.as_bytes(), size, size, NodeFlags::NONE);
    }
    let mut req = LayoutRequest::new(d, 900.0, 640.0);
    req.style.min_width = 0.0;
    let side = lay(&t, &req);
    let thin = |dd: &Decoded, k: u8| {
        dd.records
            .iter()
            .filter(|r| r.kind == k && r.w < 44.0)
            .count()
    };
    assert!(thin(&side, kind::DIR_LEAF) > 0);
    req.style.min_width = 44.0;
    let wide = lay(&t, &req); // `check` asserts every folder is 44 wide
    assert_eq!(thin(&wide, kind::DIR_LEAF), 0);
    // so every folder tile is named, while narrow files stay
    assert!(
        wide.records
            .iter()
            .all(|r| r.kind != kind::DIR_LEAF || r.name.is_some())
    );
    // Only the narrow folders joined the group, not every sibling ranked
    // after the first of them: files smaller than a grouped folder remain.
    let shown: Vec<u32> = wide.records.iter().map(|r| r.id).collect();
    let grouped_dir = t
        .children(d)
        .filter(|&c| t.node(c).is_dir() && !shown.contains(&c.0))
        .map(|c| SizeMode::Disk.of(&t.node(c)))
        .max()
        .unwrap();
    let smallest_file = wide
        .records
        .iter()
        .filter(|r| r.kind == kind::FILE)
        .map(|r| r.size)
        .min()
        .unwrap();
    assert!(smallest_file < grouped_dir);
    assert!(thin(&wide, kind::FILE) > 0);
}

#[test]
fn min_side_keeps_every_tile_wide_enough() {
    // A folder with a few big files, a long tail of middling ones and
    // many tiny ones: without minSide the tail comes out as slivers.
    let mut t = Tree::new(b"R:\\");
    let d = t.add_child(NodeId::ROOT, b"d", 0, 0, NodeFlags::DIR);
    let mut b = EntryBatch::new();
    for (i, s) in [500u64, 200, 50].into_iter().enumerate() {
        b.push_wtf8(
            format!("big{i}.mkv").as_bytes(),
            s << 20,
            s << 20,
            NodeFlags::NONE,
        );
    }
    for i in 0..60u64 {
        b.push_wtf8(
            format!("mid{i}.iso").as_bytes(),
            (3 + i % 5) << 20,
            (3 + i % 5) << 20,
            NodeFlags::NONE,
        );
    }
    for i in 0..400u64 {
        b.push_wtf8(
            format!("tiny{i}.txt").as_bytes(),
            300 << 10,
            300 << 10,
            NodeFlags::NONE,
        );
    }
    t.add_children(d, &b);
    let mut req = LayoutRequest::new(d, 900.0, 640.0);
    req.style.min_side = 0.0;
    req.style.min_width = 0.0;
    let off = lay(&t, &req);
    assert!(off.records.iter().any(|r| r.w.min(r.h) < 24.0));
    req.style.min_side = 24.0;
    let on = lay(&t, &req); // `check` asserts every tile is at least 24 x 24
    assert!(on.records.len() < off.records.len());
    assert!(on.records.iter().any(|r| r.w < 44.0));
    req.style.min_width = 44.0;
    // minWidth is for folders: files keep their tiles down to 24 x 24
    assert_eq!(lay(&t, &req).records, on.records);
    let g = on
        .records
        .iter()
        .position(|r| r.kind == kind::GROUP)
        .unwrap();
    let group = &on.records[g];
    assert_eq!(
        group.size
            + on.records
                .iter()
                .filter(|r| r.kind == kind::FILE)
                .map(|r| r.size)
                .sum::<u64>(),
        SizeMode::Disk.of(&t.node(d))
    );
    // The group goes in by its size, not last.
    assert!(on.records[g + 1..].iter().all(|r| r.size <= group.size));
    assert!(on.records[..g].iter().all(|r| r.size >= group.size));
    assert!(g < on.records.len() - 1);

    // Free space too thin for a tile is left out; the files take its room.
    let mut t = Tree::new(b"R:\\");
    t.add_child(NodeId::ROOT, b"all.bin", 1 << 40, 1 << 40, NodeFlags::NONE);
    let req = LayoutRequest::new(NodeId::ROOT, 800.0, 600.0);
    let extras = RootExtras {
        free: Some(1 << 29),
        unscanned: None,
    };
    let buf = LayoutEngine::new().layout(&t, &req, extras, 1);
    let dd = decode(&buf).unwrap();
    check(&t, &req, &dd, 1 << 29);
    assert_eq!(dd.records.len(), 1);
    assert_eq!((dd.records[0].w, dd.records[0].h), (786.0, 586.0));
}

#[test]
fn ages_go_into_the_records() {
    use crate::age::{Mean, byte_of_days, now_filetime};
    let mut t = Tree::new(b"C:\\");
    let now = now_filetime();
    let ago = |days: u64| now - days * 864_000_000_000;
    let file = |t: &mut Tree, parent: NodeId, name: &str, size: u64, days: u64| {
        let id = t.add_child(parent, name.as_bytes(), size, size, NodeFlags::NONE);
        t.set_modified(id, ago(days));
        id
    };
    // a folder that stays closed (too deep), one that opens, small files that group
    let open = t.add_child(NodeId::ROOT, b"open", 0, 0, NodeFlags::DIR);
    let shut = t.add_child(open, b"shut", 0, 0, NodeFlags::DIR);
    let inner = t.add_child(shut, b"inner", 0, 0, NodeFlags::DIR);
    file(&mut t, shut, "old.iso", 300 << 20, 1000);
    file(&mut t, inner, "new.iso", 100 << 20, 10);
    file(&mut t, open, "mid.zip", 200 << 20, 100);
    for i in 0..60 {
        file(&mut t, NodeId::ROOT, &format!("tiny{i}.txt"), 1 << 20, 3);
    }
    let no_time = t.add_child(
        NodeId::ROOT,
        b"no-time.bin",
        50 << 20,
        50 << 20,
        NodeFlags::NONE,
    );
    let mut req = LayoutRequest::new(NodeId::ROOT, 800.0, 600.0);
    req.style.max_depth = 2;
    let d = lay(&t, &req);
    check(&t, &req, &d, 0);
    let by_id = |id: NodeId| d.records.iter().find(|r| r.id == id.0).unwrap();

    let mut m = Mean::default();
    m.add(byte_of_days(1000.0), 300 << 20);
    m.add(byte_of_days(10.0), 100 << 20);
    let shut_age = m.byte();
    assert_eq!(by_id(shut).kind, kind::DIR_LEAF);
    assert_eq!(by_id(shut).age, shut_age);
    // the open folder: from its tiles, the closed folder and the zip
    m.add(byte_of_days(100.0), 200 << 20);
    assert_eq!(by_id(open).kind, kind::DIR);
    assert!(by_id(open).age.abs_diff(m.byte()) <= 1);
    assert_eq!(by_id(no_time).age, crate::age::UNKNOWN);
    let group = d.records.iter().find(|r| r.kind == kind::GROUP).unwrap();
    assert_eq!(group.age, byte_of_days(3.0));
    // the age goes into the comparison of pictures
    let mut t2 = t;
    let f = t2.lookup_relative(b"open\\mid.zip").unwrap();
    t2.set_modified(f, ago(2000));
    let mut e = LayoutEngine::new();
    let a = e.layout(&t2, &req, RootExtras::default(), 1);
    t2.set_modified(f, ago(100));
    let b = e.layout(&t2, &req, RootExtras::default(), 2);
    assert!(!same_picture(&a, &b, 0.5));

    // a write to an old file in a closed folder that keeps its size: the
    // folder's cached tint is dropped, so its age follows
    let old = t2.lookup_relative(b"open\\shut\\old.iso").unwrap();
    let seq = t2.redated_seq();
    assert!(t2.set_modified(old, now));
    assert_eq!(t2.redated_since(seq).unwrap().collect::<Vec<_>>(), [shut]);
    let c = e.layout(&t2, &req, RootExtras::default(), 3);
    let rec = decode(&c).unwrap();
    let shut_now = rec.records.iter().find(|r| r.id == shut.0).unwrap().age;
    let mut m = Mean::default();
    m.add(1, 300 << 20);
    m.add(byte_of_days(10.0), 100 << 20);
    assert_eq!(shut_now, m.byte());
    assert_ne!(shut_now, shut_age);
    // a new file's first time is not a re-dating
    let seq = t2.redated_seq();
    let fresh = t2.add_child(shut, b"fresh.txt", 1, 1, NodeFlags::NONE);
    assert!(t2.set_modified(fresh, now));
    assert_eq!(t2.redated_seq(), seq);
}

/// Many files of about the same size (a folder of photos or screenshots):
/// a bigger window must not fold them all into the group. Squarify used to
/// line the first ones up as slivers beside the group, so the first sibling
/// came out too thin and every one after it joined the group.
#[test]
fn similar_files_stay_tiles_in_bigger_windows() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for n in [300u64, 2000] {
        let mut t = Tree::new(b"R:\\");
        let d = t.add_child(NodeId::ROOT, b"d", 0, 0, NodeFlags::DIR);
        for i in 0..n {
            let size = 100_000 + rnd() % 1_500_000;
            let name = format!("shot{i}.png");
            t.add_child(d, name.as_bytes(), size, size, NodeFlags::NONE);
        }
        let mut most = 0;
        for w in (640..=1920).step_by(16) {
            let w = w as f32;
            let req = LayoutRequest::new(d, w, (w * 0.68).round());
            let files = lay(&t, &req)
                .records
                .iter()
                .filter(|r| r.kind == kind::FILE)
                .count();
            assert!(
                files * 2 >= most,
                "{n} files at {w} px wide: {files} tiles, {most} in a smaller window"
            );
            most = most.max(files);
        }
        assert!(most > 0);
    }
}

/// The ids drawn at the top of a layout with tiles of their own.
fn shown_top(d: &Decoded) -> Vec<u32> {
    d.records
        .iter()
        .filter(|r| r.parent == ids::NONE && r.id < ids::GROUP_BIT)
        .map(|r| r.id)
        .collect()
}

/// A folder of `n` files of about the same size (screenshots), a few big
/// files, a few small folders and a few tiny files.
fn screenshots(n: u64) -> (Tree, NodeId) {
    let mut seed = 0x2545_f491_4f6c_dd1du64 ^ n;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut t = Tree::new(b"R:\\");
    let d = t.add_child(NodeId::ROOT, b"Screenshots", 0, 0, NodeFlags::DIR);
    for i in 0..n {
        let size = 400_000 + rnd() % 900_000;
        let name = format!("shot{i}.png");
        t.add_child(d, name.as_bytes(), size, size, NodeFlags::NONE);
    }
    for i in 0..3u64 {
        let size = 900_000_000 + i * 100_000_000;
        t.add_child(
            d,
            format!("video{i}.mp4").as_bytes(),
            size,
            size,
            NodeFlags::NONE,
        );
    }
    for i in 0..12u64 {
        let f = t.add_child(d, format!("folder{i}").as_bytes(), 0, 0, NodeFlags::DIR);
        for j in 0..1 + i % 4 {
            let size = 200_000 + rnd() % 2_000_000;
            t.add_child(
                f,
                format!("f{j}.jpg").as_bytes(),
                size,
                size,
                NodeFlags::NONE,
            );
        }
    }
    for i in 0..5u64 {
        t.add_child(
            d,
            format!("tiny{i}.txt").as_bytes(),
            3 + i,
            3 + i,
            NodeFlags::NONE,
        );
    }
    (t, d)
}

/// Rule 9: opening the group level after level reaches every item, each
/// level shows tiles of its own beside a group of at most a third of the
/// map, and hidden items stay out.
#[test]
fn levels_reach_every_small_item() {
    for (n, w, h) in [
        (713u64, 924.0f32, 640.0f32),
        (390, 700.0, 500.0),
        (2000, 524.0, 356.0),
        (6000, 1920.0, 1040.0),
    ] {
        let (t, d) = screenshots(n);
        let req = LayoutRequest::new(d, w, h);
        let st = &req.style;
        let map = (w - 2.0 * st.outer) as f64 * (h - 2.0 * st.outer) as f64;
        let mut all: Vec<u32> = t
            .children(d)
            .filter(|&c| t.node(c).size() > 0)
            .map(|c| c.0)
            .collect();
        all.sort_unstable();
        let mut engine = LayoutEngine::new();
        let mut hide: Vec<u32> = Vec::new();
        let mut level: Option<Vec<u32>> = None;
        let mut levels = 0;
        loop {
            let buf =
                engine.layout_level(&t, &req, level.as_deref(), RootExtras::default(), 1, None);
            let dec = decode(&buf).unwrap();
            check_level(&t, &req, &dec, 0, level.as_deref());
            let tiles = shown_top(&dec);
            assert!(
                !tiles.is_empty(),
                "{n} files, level {levels}: no tile of its own"
            );
            let group = dec
                .records
                .iter()
                .find(|r| r.parent == ids::NONE && r.kind == kind::GROUP);
            if level.is_some()
                && let Some(g) = group
            {
                let cell = (g.w + st.gap) as f64 * (g.h + st.gap) as f64;
                assert!(
                    cell <= map * LEVEL_GROUP_SHARE * 1.01,
                    "{n} files: {g:?} too big"
                );
            }
            hide.extend(&tiles);
            hide.sort_unstable();
            let Some(g) = group else { break };
            let left = all.len() - hide.len();
            assert_eq!(g.count as usize, left, "{n} files, level {levels}: {g:?}");
            level = Some(hide.clone());
            levels += 1;
            assert!(levels < 200, "{n} files: no end");
        }
        // every item had a tile of its own on one level, and only on one
        let mut seen = hide.clone();
        seen.dedup();
        assert_eq!(seen.len(), hide.len(), "{n} files: an item shown twice");
        assert_eq!(seen, all, "{n} files: not every item reached");
        assert!(levels >= 1, "{n} files: no group in the folder");
    }
}

/// A level has no pseudo tiles, does not change with the window size, and
/// a group of a level is never a group of one turned into a tile too
/// thin to point at.
#[test]
fn levels_keep_their_items() {
    let (t, d) = screenshots(713);
    let mut engine = LayoutEngine::new();
    let first = lay(&t, &LayoutRequest::new(d, 924.0, 640.0));
    let mut hide = shown_top(&first);
    hide.sort_unstable();
    let extras = RootExtras {
        free: Some(1 << 40),
        unscanned: Some(1 << 36),
    };
    // the same items whatever the size: what is shown plus what is grouped
    for (w, h) in [(924.0, 640.0), (600.0, 420.0), (1600.0, 900.0)] {
        let req = LayoutRequest::new(d, w, h);
        let dec = decode(&engine.layout_level(&t, &req, Some(&hide), extras, 1, None)).unwrap();
        check_level(&t, &req, &dec, 0, Some(&hide));
        let top: Vec<_> = dec
            .records
            .iter()
            .filter(|r| r.parent == ids::NONE)
            .collect();
        assert!(
            top.iter()
                .all(|r| r.kind != kind::FREE && r.kind != kind::UNSCANNED)
        );
        let count: u32 = top
            .iter()
            .map(|r| if r.kind == kind::GROUP { r.count } else { 1 })
            .sum();
        let left = t.children(d).filter(|&c| t.node(c).size() > 0).count() - hide.len();
        assert_eq!(count as usize, left, "at {w} x {h}");
    }
    // the root as a level: no pseudo tiles either
    let req = LayoutRequest::new(NodeId::ROOT, 800.0, 600.0);
    let dec = decode(&engine.layout_level(&t, &req, Some(&[]), extras, 1, None)).unwrap();
    assert!(
        dec.records
            .iter()
            .all(|r| r.kind != kind::FREE && r.kind != kind::UNSCANNED)
    );
    assert_eq!(dec.records[0].id, d.0);
    // everything hidden: an empty level
    let mut all: Vec<u32> = t.children(d).map(|c| c.0).collect();
    all.sort_unstable();
    let req = LayoutRequest::new(d, 800.0, 600.0);
    let dec = decode(&engine.layout_level(&t, &req, Some(&all), extras, 1, None)).unwrap();
    assert!(dec.records.is_empty());
}

/// A level's group of a few tiny files beside big ones is drawn big enough
/// to point at, marked not to scale, and opening it shows them.
#[test]
fn a_tiny_group_in_a_level_is_still_drawn() {
    let mut t = Tree::new(b"R:\\");
    let d = t.add_child(NodeId::ROOT, b"d", 0, 0, NodeFlags::DIR);
    let mut big = Vec::new();
    for i in 0..6u64 {
        let s = 1_000_000_000 + i;
        big.push(
            t.add_child(d, format!("big{i}").as_bytes(), s, s, NodeFlags::NONE)
                .0,
        );
    }
    let mut tiny = Vec::new();
    for i in 0..3u64 {
        tiny.push(
            t.add_child(
                d,
                format!("tiny{i}").as_bytes(),
                10 + i,
                10 + i,
                NodeFlags::NONE,
            )
            .0,
        );
    }
    let req = LayoutRequest::new(d, 800.0, 600.0);
    // in the folder the group of three tiny files is too small to draw
    let first = lay(&t, &req);
    assert!(first.records.iter().all(|r| r.kind != kind::GROUP));
    // as a level with one big file hidden, the group is drawn anyway
    let mut engine = LayoutEngine::new();
    let hide = vec![big[0]];
    let dec = decode(&engine.layout_level(&t, &req, Some(&hide), RootExtras::default(), 1, None))
        .unwrap();
    check_level(&t, &req, &dec, 0, Some(&hide));
    let g = dec
        .records
        .iter()
        .find(|r| r.kind == kind::GROUP)
        .expect("a group");
    assert_eq!((g.count, g.note), (3, note::NOT_TO_SCALE));
    // opening it shows the three
    let mut hide = big.clone();
    hide.sort_unstable();
    let dec = decode(&engine.layout_level(&t, &req, Some(&hide), RootExtras::default(), 1, None))
        .unwrap();
    check_level(&t, &req, &dec, 0, Some(&hide));
    let mut ids = shown_top(&dec);
    ids.sort_unstable();
    assert_eq!(ids, tiny);
}
