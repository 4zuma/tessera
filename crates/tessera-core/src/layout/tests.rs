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
            b.push_wtf8(
                name.as_bytes(),
                s.div_ceil(4096) * 4096,
                s,
                NodeFlags::NONE,
                0,
            );
        }
        let d = t.add_child(
            NodeId::ROOT,
            format!("huge{h}").as_bytes(),
            0,
            0,
            NodeFlags::DIR,
            0,
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
            b.push_wtf8(format!("d{i}").as_bytes(), 0, 0, NodeFlags::DIR, 0);
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
            b.push_wtf8(name.as_bytes(), s.div_ceil(4096) * 4096, s, flags, 0);
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
    let st = &req.style;
    let mode = req.size_mode;
    let size = |n: u32| mode.of(tree.node(NodeId(n)));
    let eps = 0.02f32;
    let mut index: HashMap<u32, usize> = HashMap::new();
    let mut kids: HashMap<u32, Vec<usize>> = HashMap::new();
    let o = st.outer;
    let map = (o, o, req.width - 2.0 * o, req.height - 2.0 * o);
    assert_eq!(d.view, req.view.0);
    for (i, r) in d.records.iter().enumerate() {
        assert!(index.insert(r.id, i).is_none(), "duplicate id {r:?}");
        assert!(r.w >= 1.0 && r.h >= 1.0, "{r:?} below 1 px");
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
                    assert_eq!(r.cat, 255);
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
        let total: u64 = tree
            .children(folder)
            .map(|c| mode.of(tree.node(c)))
            .sum::<u64>()
            + if parent == ids::NONE { extras } else { 0 };
        for &i in group {
            let r = &d.records[i];
            let cell = (r.w + st.gap) as f64 * (r.h + st.gap) as f64;
            let expect = r.size as f64 * area / total as f64;
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
    let d = t.add_child(NodeId::ROOT, b"Games", 0, 0, NodeFlags::DIR, 0);
    let f = t.add_child(d, b"big.pak", 600 << 20, 600 << 20, NodeFlags::NONE, 0);
    let v = t.add_child(
        NodeId::ROOT,
        b"movie.mkv",
        400 << 20,
        400 << 20,
        NodeFlags::CLOUD,
        0,
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
    assert_eq!(u16_at(6), 32);
    let count = u32_at(8) as usize;
    assert_eq!(count, 3); // Games (with big.pak) and movie.mkv
    assert_eq!(u32_at(12), 52);
    let names_len = u32_at(16) as usize;
    assert_eq!(buf.len(), 32 + 52 * count + names_len);
    assert_eq!(u32_at(20), 0);
    assert_eq!(u32_at(24), 42);
    let map_area = (800.0 - 12.0) * (600.0 - 12.0);
    assert!((f32_at(28) - (1000u64 << 20) as f32 / map_area).abs() < 1.0);

    // Records, field by field.
    let rec = |i: usize| 32 + 52 * i;
    let names = &buf[32 + 52 * count..];
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

    assert_eq!(u32_at(m), v.0);
    assert_eq!(buf[m + 47], kind::FILE);
    assert_eq!(buf[m + 48], category::VIDEO);
    assert_eq!(buf[m + 49], flags::CLOUD);
    assert_eq!(name_of(m), "movie.mkv");

    assert_eq!(u32_at(p), f.0);
    assert_eq!(u32_at(p + 4), d.0);
    assert_eq!(buf[p + 46], 2);
    assert_eq!(buf[p + 48], category::GAME);
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
    assert!(decode(&buf[..31]).is_err());
    assert!(decode(&buf[..buf.len() - 1]).is_err());
    let mut bad = buf.clone();
    bad[0] = b'X';
    assert!(decode(&bad).is_err());
    let mut bad = buf.clone();
    bad[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&bad).is_err());
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
}

#[test]
fn hysteresis_keeps_the_order_calm() {
    let mut t = Tree::new(b"R:\\");
    let a = t.add_child(NodeId::ROOT, b"a.bin", 100_000, 100_000, NodeFlags::NONE, 0);
    let b = t.add_child(NodeId::ROOT, b"b.bin", 90_000, 90_000, NodeFlags::NONE, 0);
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
fn small_and_surplus_children_are_grouped() {
    let mut t = Tree::new(b"R:\\");
    let d = t.add_child(NodeId::ROOT, b"dir", 0, 0, NodeFlags::DIR, 0);
    t.add_child(d, b"big.iso", 100_000, 100_000, NodeFlags::NONE, 0);
    let mut b = EntryBatch::new();
    for i in 0..600 {
        b.push_wtf8(format!("s{i}.mp3").as_bytes(), 10, 10, NodeFlags::NONE, 0);
    }
    for i in 0..400 {
        b.push_wtf8(format!("p{i}.JPG").as_bytes(), 20, 20, NodeFlags::NONE, 0);
    }
    b.push_wtf8(b"empty.txt", 0, 0, NodeFlags::NONE, 0);
    t.add_children(d, &b);
    t.add_child(
        NodeId::ROOT,
        b"other.bin",
        100_000,
        100_000,
        NodeFlags::NONE,
        0,
    );
    let req = LayoutRequest::new(NodeId::ROOT, 800.0, 600.0);
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
            0,
        );
    }
    t.add_children(NodeId::ROOT, &b);
    let mut req = LayoutRequest::new(NodeId::ROOT, 1000.0, 800.0);
    req.style.max_per_dir = 10;
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
    t.add_child(NodeId::ROOT, b"big", 10_000_000, 1, NodeFlags::NONE, 0);
    let small = t.add_child(NodeId::ROOT, b"small", 400, 1, NodeFlags::NONE, 0);
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
    let a = t.add_child(NodeId::ROOT, b"a", 0, 0, NodeFlags::DIR, 0);
    let b = t.add_child(a, b"b", 0, 0, NodeFlags::DIR, 0);
    let c = t.add_child(b, b"c", 0, 0, NodeFlags::DIR, 0);
    t.add_child(b, b"empty", 0, 0, NodeFlags::NONE, 0); // size 0: invisible
    let mut batch = EntryBatch::new();
    for i in 0..3u64 {
        batch.push_wtf8(
            format!("x{i}.mp4").as_bytes(),
            1 << (20 + i),
            1,
            NodeFlags::NONE,
            0,
        );
    }
    let files = t.add_children(c, &batch);
    t.add_child(NodeId::ROOT, b"z.bin", 1 << 21, 1, NodeFlags::NONE, 0);
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
        cur = t.add_child(cur, format!("d{i}").as_bytes(), 0, 0, NodeFlags::DIR, 0);
        chain.push(cur);
        let size = 3u64 << (2 * (19 - i) + 10);
        t.add_child(cur, format!("f{i}").as_bytes(), size, 1, NodeFlags::NONE, 0);
    }
    t.add_child(
        NodeId::ROOT,
        b"tiny-dir-sibling",
        1 << 20,
        1,
        NodeFlags::NONE,
        0,
    );
    (t, chain)
}

#[test]
fn folders_open_by_size_and_depth() {
    let (t, chain) = chain_tree();
    let mut req = LayoutRequest::new(NodeId::ROOT, 3000.0, 3000.0);
    req.style.max_depth = 4;
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
    let used = t.node(NodeId::ROOT).size;
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
        0,
    );
    t.add_child(bin, b"$R1.dat", 1 << 30, 1 << 30, NodeFlags::NONE, 0);
    let locked = t.add_child(
        NodeId::ROOT,
        b"locked",
        1 << 29,
        1 << 29,
        NodeFlags::INACCESSIBLE,
        0,
    );
    let link = t.add_child(
        NodeId::ROOT,
        b"link",
        1 << 28,
        1 << 28,
        NodeFlags::REPARSE,
        0,
    );
    let odd = t.add_child(
        NodeId::ROOT,
        &[b'x', 0xED, 0xA0, 0x80, b'.', b'p', b'd', b'f'],
        1 << 29,
        1,
        NodeFlags::NONE,
        0,
    );
    let zero = t.add_child(NodeId::ROOT, b"zero", 0, 0, NodeFlags::NONE, 0);
    let gone = t.add_child(NodeId::ROOT, b"gone", 1 << 29, 1 << 29, NodeFlags::NONE, 0);
    let tiny = t.add_child(NodeId::ROOT, b"tiny", 1 << 21, 1, NodeFlags::NONE, 0);
    assert!(t.mark_deleted(gone));
    let req = LayoutRequest::new(NodeId::ROOT, 1000.0, 800.0);
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
    let mut req = req;
    req.style.label_min_w = 5000.0;
    let d = lay(&t, &req);
    assert!(d.records.iter().all(|r| r.name.is_some() == (r.header > 0)));
    // A folder below a volume root is not the recycle bin.
    let mut t2 = Tree::new(b"D:\\Data");
    let b2 = t2.add_child(NodeId::ROOT, b"$Recycle.Bin", 0, 0, NodeFlags::DIR, 0);
    t2.add_child(b2, b"x", 1 << 20, 1, NodeFlags::NONE, 0);
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
    let first = t.add_child(NodeId::ROOT, comps[0].as_bytes(), 0, 0, NodeFlags::DIR, 0);
    let mut cur = first;
    for c in &comps[1..] {
        cur = t.add_child(cur, c.as_bytes(), 0, 0, NodeFlags::DIR, 0);
    }
    t.add_child(cur, b"inside.bin", 1 << 30, 1, NodeFlags::NONE, 0);
    let free = t.add_child(
        NodeId::ROOT,
        b"tessera-elsewhere.bin",
        1 << 30,
        1,
        NodeFlags::NONE,
        0,
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
    let w = t.add_child(NodeId::ROOT, name.as_bytes(), 0, 0, NodeFlags::DIR, 0);
    let temp = t.add_child(w, b"Temp", 0, 0, NodeFlags::DIR, 0);
    t.add_child(temp, b"x.tmp", 1 << 30, 1, NodeFlags::NONE, 0);
    t.add_child(w, b"notepad.exe", 1 << 29, 1, NodeFlags::NONE, 0); // no folding
    let other = t.add_child(
        NodeId::ROOT,
        b"tessera-elsewhere",
        1 << 30,
        1,
        NodeFlags::NONE,
        0,
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
        b.push_wtf8(
            format!("f{i}").as_bytes(),
            1000 + i % 7,
            1,
            NodeFlags::NONE,
            0,
        );
    }
    for d in 0..3 {
        let dir = t.add_child(
            NodeId::ROOT,
            format!("d{d}").as_bytes(),
            0,
            0,
            NodeFlags::DIR,
            0,
        );
        t.add_children(dir, &b);
    }
    let mut req = LayoutRequest::new(NodeId::ROOT, 8000.0, 8000.0);
    req.style.min_area = 1.0;
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
        b.push_wtf8(
            format!("f{i}").as_bytes(),
            1000 + i % 7,
            1,
            NodeFlags::NONE,
            0,
        );
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
            .max_by_key(|&c| t.node(c).size)
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
