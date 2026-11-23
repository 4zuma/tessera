//! Search: which files and folders match a query, kept as one byte per node
//! for the layout to dim everything else.
//!
//! There is no index. A search reads every node once: first the nodes in
//! order, on all cores, to decide which match by themselves (a name is
//! decoded and compared once per distinct name in a stretch of nodes, since
//! many repeat), then the tree from the root, to add up how many of each
//! folder's bytes match. The result is a byte per node, the share of its
//! bytes that match (255 = all), which is what the map needs to dim a tile,
//! a closed folder or a group by how much of it matches. On a drive of 3.3
//! million entries that is 3.3 MB while a search is shown, plus 0.4 MB while
//! one runs.
//!
//! A query is words separated by spaces; a word in double quotes may hold
//! spaces. Every word must hold:
//!
//! * `.iso`, `*.iso`: the name ends in `.iso` (files only). Several of these
//!   mean any of them.
//! * `>1GB`, `<=500MB`, `>=2.5g`: the size, in the size mode searched, in
//!   binary units as the map shows them (`b`, `k`/`kb`, `m`/`mb`, `g`/`gb`,
//!   `t`/`tb`, any case; no unit means bytes).
//! * `>1y`, `<2w`, `>=6mo`, `<3d`: how long ago the file was last written
//!   (files only), in days (`d`, `day`, `days`, `天`), weeks (`w`, `wk`,
//!   `week`, `weeks`, `周`), months of 30 days (`mo`, `month`, `months`,
//!   `月`, `个月`) or years of 365 days (`y`, `yr`, `year`, `years`, `年`),
//!   compared on the start of the file's [`crate::age`] step; a file whose
//!   time is not known never fits. A word that reads as a size is a size
//!   (`>1m` is a megabyte, `>1mo` a month).
//! * anything else: the name contains it, ignoring case as Windows does.
//!
//! A folder matches when the query has words for the name and no extension
//! or age, and the folder's name and size fit; everything in a matching
//! folder is then a match too, but only the folder is counted.

use rayon::prelude::*;

use crate::age;
use crate::layout::SizeMode;
use crate::tree::{NodeId, Tree};
use crate::wtf8;

/// A parsed query; see the module docs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// Name fragments, folded ([`wtf8::push_folded`]); all must appear.
    words: Vec<Vec<u8>>,
    /// Name endings with their dot, folded; one of them must match.
    exts: Vec<Vec<u8>>,
    /// Size bounds; all must hold.
    sizes: Vec<(Cmp, u64)>,
    /// Age bounds in days; all must hold (files only).
    ages: Vec<(Cmp, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmp {
    Gt,
    Ge,
    Lt,
    Le,
}

impl Cmp {
    #[inline]
    fn holds<T: PartialOrd>(self, v: T, bound: T) -> bool {
        match self {
            Cmp::Gt => v > bound,
            Cmp::Ge => v >= bound,
            Cmp::Lt => v < bound,
            Cmp::Le => v <= bound,
        }
    }
}

/// Why a query could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryError {
    /// A size or age word (`>1GB`, `>1y`) whose number or unit is not
    /// understood; holds the word.
    BadSize(String),
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::BadSize(w) => write!(f, "not a size or an age: {w}"),
        }
    }
}

impl std::error::Error for QueryError {}

impl Query {
    /// Reads a query. `Ok(None)` when it has no words at all.
    pub fn parse(text: &str) -> Result<Option<Query>, QueryError> {
        let mut q = Query::default();
        for (word, quoted) in split(text) {
            if quoted {
                q.words.push(folded(word));
            } else if let Some(rest) = word.strip_prefix(['>', '<']) {
                let gt = word.starts_with('>');
                let (eq, num) = match rest.strip_prefix('=') {
                    Some(n) => (true, n),
                    None => (false, rest),
                };
                let cmp = match (gt, eq) {
                    (true, false) => Cmp::Gt,
                    (true, true) => Cmp::Ge,
                    (false, false) => Cmp::Lt,
                    (false, true) => Cmp::Le,
                };
                if let Some(bytes) = parse_size(num) {
                    q.sizes.push((cmp, bytes));
                } else if let Some(days) = parse_age(num) {
                    q.ages.push((cmp, days));
                } else {
                    return Err(QueryError::BadSize(word.into()));
                }
            } else if let Some(ext) = word
                .strip_prefix("*.")
                .or_else(|| word.strip_prefix('.'))
                .filter(|e| !e.is_empty() && !e.starts_with('.'))
            {
                let mut e = vec![b'.'];
                wtf8::push_folded(&mut e, ext.as_bytes());
                q.exts.push(e);
            } else {
                q.words.push(folded(word));
            }
        }
        Ok((!q.is_empty()).then_some(q))
    }

    fn is_empty(&self) -> bool {
        self.words.is_empty()
            && self.exts.is_empty()
            && self.sizes.is_empty()
            && self.ages.is_empty()
    }

    /// The query looks at names at all (else names are never decoded).
    fn reads_names(&self) -> bool {
        !self.words.is_empty() || !self.exts.is_empty()
    }

    /// A folder can match by itself.
    fn matches_folders(&self) -> bool {
        !self.words.is_empty() && self.exts.is_empty() && self.ages.is_empty()
    }

    /// Which age bytes fit every age bound: a byte stands for the start of
    /// its step, and a time not known fits only when there is no bound.
    fn age_table(&self) -> [bool; 256] {
        let mut ok = [true; 256];
        for (b, slot) in ok.iter_mut().enumerate() {
            *slot = match age::days_of(b as u8) {
                None => self.ages.is_empty(),
                Some(d) => self.ages.iter().all(|&(c, bound)| c.holds(d, bound)),
            };
        }
        ok
    }

    #[inline]
    fn size_fits(&self, size: u64) -> bool {
        self.sizes.iter().all(|&(c, b)| c.holds(size, b))
    }

    /// The name part of the query holds for a folded name.
    fn name_fits(&self, folded: &[u8], dir: bool) -> bool {
        (dir || self.exts.is_empty() || self.exts.iter().any(|e| folded.ends_with(e)))
            && self.words.iter().all(|w| contains(folded, w))
    }
}

fn folded(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len());
    wtf8::push_folded(&mut v, s.as_bytes());
    v
}

/// The words of a query, each with whether it was in double quotes.
fn split(text: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return out;
        }
        if let Some(r) = rest.strip_prefix('"') {
            let end = r.find('"').unwrap_or(r.len());
            if !r[..end].is_empty() {
                out.push((&r[..end], true));
            }
            rest = r.get(end + 1..).unwrap_or("");
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            out.push((&rest[..end], false));
            rest = &rest[end..];
        }
    }
}

/// `1.5GB` → bytes, in binary units.
fn parse_size(s: &str) -> Option<u64> {
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let shift = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 0,
        "k" | "kb" | "kib" => 10,
        "m" | "mb" | "mib" => 20,
        "g" | "gb" | "gib" => 30,
        "t" | "tb" | "tib" => 40,
        _ => return None,
    };
    let v = n * (1u64 << shift) as f64;
    (v.is_finite() && v >= 0.0 && v < u64::MAX as f64).then(|| v.round() as u64)
}

/// `6mo` → days; a month is 30 days and a year 365, as the legend's bands
/// count them.
fn parse_age(s: &str) -> Option<f64> {
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let per = match unit.trim().to_ascii_lowercase().as_str() {
        "d" | "day" | "days" | "天" => 1.0,
        "w" | "wk" | "week" | "weeks" | "周" => 7.0,
        "mo" | "month" | "months" | "月" | "个月" => 30.0,
        "y" | "yr" | "year" | "years" | "年" => 365.0,
        _ => return None,
    };
    let d = n * per;
    d.is_finite().then_some(d)
}

#[inline]
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| w == needle)
}

/// The share of `size` that `part` is, as 0 (none) to 255 (all); any part
/// at all is at least 1, so a folder with a single small match still shows
/// that it has one.
#[inline]
pub fn share_of(part: u64, size: u64) -> u8 {
    if part == 0 {
        0
    } else if part >= size {
        255
    } else {
        (part as u128 * 254 / size as u128).max(1) as u8
    }
}

/// What a search found: a byte per node (see [`share_of`]) and the totals.
pub struct Hits {
    /// Echoed in layouts drawn with these hits, so the window can tell
    /// which search they show.
    pub id: u32,
    /// The size mode the shares and `bytes` were counted in.
    pub mode: SizeMode,
    share: Vec<u8>,
    /// Matching files and folders, not counting what is inside a matching
    /// folder.
    pub count: u64,
    /// Their bytes, each counted once.
    pub bytes: u64,
}

impl Hits {
    /// How much of `id` matches: 0 none, 255 all. Nodes added after the
    /// search read as 0.
    #[inline]
    pub fn share(&self, id: NodeId) -> u8 {
        self.share.get(id.index()).copied().unwrap_or(0)
    }

    pub fn heap_bytes(&self) -> usize {
        self.share.capacity()
    }
}

/// Nodes per piece of the first pass, and between two looks at `cancel`.
const PIECE: usize = 1 << 16;
/// Slots of each piece's cache of names already compared.
const CACHE: usize = 1 << 12;

/// Runs `q` over `tree`. Returns `None` when `cancel` said so on one of its
/// looks (about every 65,536 nodes).
pub fn search(
    tree: &Tree,
    q: &Query,
    mode: SizeMode,
    id: u32,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Option<Hits> {
    let n = tree.len();
    let words = n.div_ceil(64);
    let mut bits = vec![0u64; words];
    let reads_names = q.reads_names();
    let folders = q.matches_folders();
    let aged = q.age_table();

    // 1. Which nodes match by themselves.
    let stopped = bits
        .par_chunks_mut(PIECE / 64)
        .enumerate()
        .map(|(p, out)| {
            if cancel() {
                return true;
            }
            // names repeat a lot (index.js, LICENSE, ...): remember the
            // last answer per stored name, keyed by its place in the pool
            let mut cache = vec![(u32::MAX, 0u16, false); CACHE];
            let mut buf = Vec::with_capacity(64);
            let first = p * PIECE;
            let last = (first + PIECE).min(n);
            for i in first..last {
                if i == 0 {
                    continue; // the root's name is the scanned path
                }
                let id = NodeId(i as u32);
                let node = tree.node(id);
                let dir = node.is_dir();
                if node.is_deleted()
                    || (dir && !folders)
                    || !q.size_fits(mode.of(&node))
                    || (!dir && !aged[tree.age(id) as usize])
                {
                    continue;
                }
                let hit = if reads_names {
                    let (off, len) = tree.name_key(id);
                    let slot = &mut cache[(off as usize ^ (off as usize >> 12)) & (CACHE - 1)];
                    if slot.0 == off && slot.1 == len {
                        slot.2
                    } else {
                        buf.clear();
                        wtf8::push_folded(&mut buf, &tree.name(id));
                        // the cached answer is for a file; folders skip
                        // the extension test, and have no extension words
                        // here anyway (see matches_folders)
                        let ok = q.name_fits(&buf, false);
                        *slot = (off, len, ok);
                        ok
                    }
                } else {
                    true
                };
                if hit {
                    out[(i - first) / 64] |= 1 << (i % 64);
                }
            }
            false
        })
        .reduce(|| false, |a, b| a || b);
    if stopped {
        return None;
    }

    // 2. From the root: shares, count and bytes. A frame per open folder:
    // the folder, the next child to look at, the matching bytes so far and
    // whether the folder or one of its ancestors matched.
    let bit = |id: NodeId| bits[id.index() / 64] >> (id.index() % 64) & 1 != 0;
    let mut share = vec![0u8; n];
    let mut count = 0u64;
    let mut stack: Vec<(NodeId, NodeId, u64, bool)> = Vec::with_capacity(64);
    stack.push((NodeId::ROOT, tree.node(NodeId::ROOT).first_child, 0, false));
    let mut seen = 0usize;
    let mut total = 0u64;
    while let Some(top) = stack.last_mut() {
        let (dir, cur, acc, inside) = *top;
        if cur.is_none() {
            stack.pop();
            let size = mode.of(&tree.node(dir));
            let part = if inside { size } else { acc };
            share[dir.index()] = if inside { 255 } else { share_of(acc, size) };
            match stack.last_mut() {
                Some(parent) => parent.2 = parent.2.saturating_add(part),
                None => total = part,
            }
            continue;
        }
        seen += 1;
        if seen % PIECE == 0 && cancel() {
            return None;
        }
        let node = tree.node(cur);
        top.1 = node.next_sibling;
        if node.is_deleted() {
            continue;
        }
        if node.is_dir() {
            let own = !inside && bit(cur);
            count += own as u64;
            stack.push((cur, node.first_child, 0, inside || own));
        } else if inside || bit(cur) {
            count += !inside as u64;
            share[cur.index()] = 255;
            top.2 = acc.saturating_add(mode.of(&node));
        }
    }
    Some(Hits {
        id,
        mode,
        share,
        count,
        bytes: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{EntryBatch, NodeFlags};

    fn q(s: &str) -> Query {
        Query::parse(s).unwrap().unwrap()
    }

    #[test]
    fn parses_words_extensions_and_sizes() {
        assert_eq!(Query::parse("   ").unwrap(), None);
        let a = q(r#"Cache .ISO *.img >1.5GB <=10k "my file""#);
        assert_eq!(a.words, vec![b"CACHE".to_vec(), b"MY FILE".to_vec()]);
        assert_eq!(a.exts, vec![b".ISO".to_vec(), b".IMG".to_vec()]);
        assert_eq!(a.sizes, vec![(Cmp::Gt, 3 << 29), (Cmp::Le, 10 << 10)]);
        assert_eq!(q(">100").sizes, vec![(Cmp::Gt, 100)]);
        assert_eq!(q(">=2 ").sizes, vec![(Cmp::Ge, 2)]);
        // a lone dot or two dots are words, not extensions
        assert_eq!(q(".").words, vec![b".".to_vec()]);
        assert_eq!(q("..").words, vec![b"..".to_vec()]);
        assert_eq!(
            Query::parse(">1XB"),
            Err(QueryError::BadSize(">1XB".into()))
        );
        assert_eq!(Query::parse(">"), Err(QueryError::BadSize(">".into())));
        // an unclosed quote runs to the end
        assert_eq!(q(r#""a b"#).words, vec![b"A B".to_vec()]);
    }

    #[test]
    fn parses_ages() {
        let a = q(">1y <=2W >=6mo <3d >1.5w <2年 >3个月 >=1天 <1周 >2月");
        assert_eq!(
            a.ages,
            vec![
                (Cmp::Gt, 365.0),
                (Cmp::Le, 14.0),
                (Cmp::Ge, 180.0),
                (Cmp::Lt, 3.0),
                (Cmp::Gt, 10.5),
                (Cmp::Lt, 730.0),
                (Cmp::Gt, 90.0),
                (Cmp::Ge, 1.0),
                (Cmp::Lt, 7.0),
                (Cmp::Gt, 60.0),
            ]
        );
        assert!(a.words.is_empty() && a.sizes.is_empty());
        // a unit after a space is a word of its own: `>1.5` is then bytes
        let b = q(">1.5 weeks");
        assert_eq!(
            (b.sizes, b.words),
            (vec![(Cmp::Gt, 2)], vec![b"WEEKS".to_vec()])
        );
        // a size wins where a word reads as both: m is a megabyte
        assert_eq!(q(">1m").sizes, vec![(Cmp::Gt, 1 << 20)]);
        assert_eq!(q(">1mo").ages, vec![(Cmp::Gt, 30.0)]);
        assert_eq!(q(">2y").sizes, vec![]);
        for bad in [">1x", ">y", ">1yy", ">-1d"] {
            assert_eq!(
                Query::parse(bad),
                Err(QueryError::BadSize(bad.into())),
                "{bad}"
            );
        }
    }

    #[test]
    fn shares_round_up_to_one() {
        assert_eq!(share_of(0, 100), 0);
        assert_eq!(share_of(1, 1 << 40), 1);
        assert_eq!(share_of(50, 100), 127);
        assert_eq!(share_of(99, 100), 251);
        assert_eq!(share_of(100, 100), 255);
        assert_eq!(share_of(5, 0), 255);
    }

    /// root
    /// ├─ Games/            setup.iso 4000, readme.txt 100
    /// ├─ iso images/       a.iso 1000, b.IMG 2000
    /// ├─ notes.txt 50
    /// └─ old/ (deleted)    c.iso 700
    fn sample() -> (Tree, [NodeId; 4]) {
        let mut t = Tree::new(b"C:\\");
        let mut b = EntryBatch::new();
        for (name, flags) in [
            (&b"Games"[..], NodeFlags::DIR),
            (b"iso images", NodeFlags::DIR),
            (b"notes.txt", NodeFlags::NONE),
            (b"old", NodeFlags::DIR),
        ] {
            let size = if flags == NodeFlags::DIR { 0 } else { 50 };
            b.push_wtf8(name, size, size, flags);
        }
        let top = t.add_children(NodeId::ROOT, &b);
        let ids: Vec<NodeId> = top.map(NodeId).collect();
        let add = |t: &mut Tree, dir: NodeId, files: &[(&[u8], u64)]| {
            let mut b = EntryBatch::new();
            for &(n, s) in files {
                b.push_wtf8(n, s, s, NodeFlags::NONE);
            }
            t.add_children(dir, &b);
        };
        add(
            &mut t,
            ids[0],
            &[(b"setup.iso", 4000), (b"readme.txt", 100)],
        );
        add(&mut t, ids[1], &[(b"a.iso", 1000), (b"b.IMG", 2000)]);
        add(&mut t, ids[3], &[(b"c.iso", 700)]);
        t.mark_deleted(ids[3]);
        (t, [ids[0], ids[1], ids[2], ids[3]])
    }

    fn run(t: &Tree, s: &str) -> Hits {
        search(t, &q(s), SizeMode::Logical, 7, &|| false).unwrap()
    }

    #[test]
    fn files_by_extension_and_size() {
        let (t, [games, images, notes, _]) = sample();
        let h = run(&t, ".iso");
        assert_eq!((h.count, h.bytes, h.id), (2, 5000, 7));
        assert_eq!(h.share(games), share_of(4000, 4100));
        assert_eq!(h.share(images), share_of(1000, 3000));
        assert_eq!(h.share(notes), 0);
        assert_eq!(h.share(NodeId::ROOT), share_of(5000, 7150));

        let h = run(&t, ".iso .img >1500");
        assert_eq!((h.count, h.bytes), (2, 6000));
        let h = run(&t, "<100");
        assert_eq!((h.count, h.bytes), (1, 50)); // notes.txt; folders need a word
    }

    #[test]
    fn a_matching_folder_lights_everything_in_it() {
        let (t, [games, images, _, _]) = sample();
        let h = run(&t, "ISO");
        // the folder "iso images" counts once; setup.iso is its own match
        assert_eq!((h.count, h.bytes), (2, 7000));
        assert_eq!(h.share(images), 255);
        assert_eq!(h.share(games), share_of(4000, 4100));
        let kids: Vec<NodeId> = t.children(images).collect();
        assert!(kids.iter().all(|&k| h.share(k) == 255));
        // an extension word keeps folders out
        let h = run(&t, "iso .iso");
        assert_eq!((h.count, h.bytes), (2, 5000));
    }

    #[test]
    fn names_ignore_case_and_need_every_word() {
        let (t, _) = sample();
        assert_eq!(run(&t, "SETUP iso").count, 1);
        assert_eq!(run(&t, "setup txt").count, 0);
        assert_eq!(run(&t, "\"me.t\"").count, 1); // readme.txt
        // the root's name (the drive) is not searched
        assert_eq!(run(&t, "C:").count, 0);
    }

    #[test]
    fn deleted_nodes_and_late_nodes_do_not_match() {
        let (mut t, [games, ..]) = sample();
        let h = run(&t, "c.iso");
        assert_eq!(h.count, 0);
        let mut b = EntryBatch::new();
        b.push_wtf8(b"late.iso", 10, 10, NodeFlags::NONE);
        let r = t.add_children(games, &b);
        assert_eq!(h.share(NodeId(r.start)), 0);
    }

    #[test]
    fn ages_match_files_only() {
        use crate::age::now_filetime;
        let mut t = Tree::new(b"C:\\");
        let now = now_filetime();
        let ago = |days: f64| now - (days * 864e9) as u64;
        let old = t.add_child(NodeId::ROOT, b"old 2023", 0, 0, NodeFlags::DIR);
        let new = t.add_child(NodeId::ROOT, b"new", 0, 0, NodeFlags::DIR);
        let file = |t: &mut Tree, dir: NodeId, name: &str, size: u64, days: Option<f64>| {
            let id = t.add_child(dir, name.as_bytes(), size, size, NodeFlags::NONE);
            if let Some(d) = days {
                assert!(t.set_modified(id, ago(d)));
            }
            id
        };
        let setup = file(&mut t, old, "setup.iso", 4000, Some(800.0));
        let log = file(&mut t, old, "2023.log", 100, Some(400.0));
        let clip = file(&mut t, new, "clip.mp4", 2000, Some(3.0));
        let today = file(&mut t, new, "today.txt", 10, Some(0.2));
        let unknown = file(&mut t, new, "unknown.iso", 500, None);

        let h = run(&t, ">1y");
        assert_eq!((h.count, h.bytes), (2, 4100));
        assert_eq!(h.share(old), 255);
        assert_eq!(
            (h.share(setup), h.share(log), h.share(unknown)),
            (255, 255, 0)
        );
        let h = run(&t, ">1y .iso");
        assert_eq!((h.count, h.bytes), (1, 4000));
        let h = run(&t, "<1w");
        assert_eq!((h.count, h.bytes), (2, 2010));
        assert_eq!((h.share(clip), h.share(today)), (255, 255));
        // a day counts from the start of its step: under a day is 0 days
        assert_eq!(run(&t, "<1d").count, 1);
        assert_eq!(run(&t, ">=0d").count, 4);
        // between two bounds; a time not known never fits
        assert_eq!(run(&t, ">1w <2y").count, 1);
        assert_eq!(run(&t, ">=0d .iso").count, 1);
        // with an age word a folder cannot match by name: only files do
        let h = run(&t, "2023 >1w");
        assert_eq!((h.count, h.bytes), (1, 100));
        assert_eq!(h.share(old), share_of(100, 4100));
        assert_eq!(run(&t, "2023").count, 1); // the folder, holding the log
    }

    #[test]
    fn cancel_stops_it() {
        let (t, _) = sample();
        assert!(search(&t, &q("a"), SizeMode::Disk, 1, &|| true).is_none());
    }

    #[test]
    fn many_nodes_use_the_name_cache_and_every_piece() {
        let mut t = Tree::new(b"D:\\");
        let mut b = EntryBatch::new();
        for i in 0..200_000u64 {
            let name = if i % 3 == 0 {
                "index.js".to_string()
            } else {
                format!("f{i}.dat")
            };
            b.push_wtf8(name.as_bytes(), 10, 10, NodeFlags::NONE);
        }
        t.add_children(NodeId::ROOT, &b);
        let h = run(&t, "index");
        assert_eq!(h.count, 66_667);
        assert_eq!(h.bytes, 666_670);
        let h = run(&t, ".dat >5");
        assert_eq!(h.count, 133_333);
    }
}
