//! Recursive totals below a folder (files, folders, bytes per file
//! category), for the details card and the legend by type.
//!
//! Counting a whole system drive walks millions of nodes (about 0.1 s), and
//! the UI asks again after every live change it draws. [`TotalsCache`]
//! keeps the totals of each folder with at least [`MEMO_MIN`] entries below
//! it, and forgets a folder's totals, and its ancestors', when the tree's
//! change journal ([`Tree::touched_since`]) says something in it changed.
//! After a small change only the folders on the changed path are counted
//! again, and inside them only what is not a big unchanged subfolder.

use std::collections::{HashMap, HashSet};

use crate::category;
use crate::layout::SizeMode;
use crate::{NodeId, Tree};

/// What is below a folder (not counting the folder itself).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub files: u64,
    pub dirs: u64,
    /// Bytes on disk of the files, per category.
    pub disk: [u64; category::COUNT],
    /// File lengths, per category.
    pub logical: [u64; category::COUNT],
}

impl Totals {
    /// Bytes per category in `mode`.
    pub fn bytes(&self, mode: SizeMode) -> &[u64; category::COUNT] {
        match mode {
            SizeMode::Disk => &self.disk,
            SizeMode::Logical => &self.logical,
        }
    }

    fn add(&mut self, o: &Totals) {
        self.files += o.files;
        self.dirs += o.dirs;
        for c in 0..category::COUNT {
            self.disk[c] += o.disk[c];
            self.logical[c] += o.logical[c];
        }
    }
}

/// Folders with at least this many entries below them keep their totals
/// (on a full system drive a few thousand folders, well under 1 MB).
pub const MEMO_MIN: u64 = 4096;

/// [`Totals`] of big folders, kept valid through the tree's change journal.
/// Use one cache per tree.
#[derive(Default)]
pub struct TotalsCache {
    memo: HashMap<u32, Totals>,
    /// The journal position the memo is valid at.
    seq: u64,
}

impl TotalsCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The totals below `dir` as the tree is now.
    pub fn get(&mut self, tree: &Tree, dir: NodeId) -> Totals {
        self.catch_up(tree);
        self.walk(tree, dir)
    }

    /// Number of folders whose totals are kept.
    pub fn len(&self) -> usize {
        self.memo.len()
    }

    pub fn is_empty(&self) -> bool {
        self.memo.is_empty()
    }

    /// Forgets the totals of every folder the journal lists since the last
    /// call, and of their ancestors.
    fn catch_up(&mut self, tree: &Tree) {
        let now = tree.change_seq();
        if now == self.seq {
            return;
        }
        match tree.touched_since(self.seq) {
            Some(_) if self.memo.is_empty() => {}
            Some(dirs) => {
                let mut seen = HashSet::new();
                for d in dirs {
                    let mut cur = d;
                    // an ancestor chain already cleared ends the climb
                    while !cur.is_none() && cur.index() < tree.len() && seen.insert(cur) {
                        self.memo.remove(&cur.0);
                        cur = tree.node(cur).parent;
                    }
                }
            }
            None => self.memo.clear(),
        }
        self.seq = now;
    }

    /// Adds up the subtree of `dir` depth first, taking kept totals for
    /// big folders as they are and keeping new ones.
    fn walk(&mut self, tree: &Tree, dir: NodeId) -> Totals {
        if let Some(t) = self.memo.get(&dir.0) {
            return *t;
        }
        struct Frame {
            dir: NodeId,
            next: NodeId,
            acc: Totals,
        }
        let mut stack = vec![Frame {
            dir,
            next: tree.node(dir).first_child,
            acc: Totals::default(),
        }];
        loop {
            let top = stack.last_mut().expect("the stack ends with the walk");
            let c = top.next;
            if c.is_none() {
                let done = stack.pop().expect("looked at it just now");
                if done.acc.files + done.acc.dirs >= MEMO_MIN {
                    self.memo.insert(done.dir.0, done.acc);
                }
                match stack.last_mut() {
                    Some(parent) => parent.acc.add(&done.acc),
                    None => return done.acc,
                }
                continue;
            }
            let n = tree.node(c);
            top.next = n.next_sibling;
            if n.is_deleted() {
                continue;
            }
            if n.is_dir() {
                top.acc.dirs += 1;
                if let Some(m) = self.memo.get(&c.0) {
                    top.acc.add(m);
                    continue;
                }
                stack.push(Frame {
                    dir: c,
                    next: n.first_child,
                    acc: Totals::default(),
                });
            } else {
                top.acc.files += 1;
                let cat = category::of_node(tree, c) as usize;
                if cat < category::COUNT {
                    top.acc.disk[cat] += n.size();
                    top.acc.logical[cat] += n.logical();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeFlags;

    /// The totals the slow way: every node below `dir`.
    fn brute(tree: &Tree, dir: NodeId) -> Totals {
        let mut t = Totals::default();
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            for c in tree.children(d) {
                let n = tree.node(c);
                if n.is_dir() {
                    t.dirs += 1;
                    stack.push(c);
                } else {
                    t.files += 1;
                    let cat = category::of_node(tree, c) as usize;
                    t.disk[cat] += n.size();
                    t.logical[cat] += n.logical();
                }
            }
        }
        t
    }

    fn file(t: &mut Tree, parent: NodeId, name: &str, size: u64) -> NodeId {
        t.add_child(parent, name.as_bytes(), size, size, NodeFlags::NONE)
    }

    fn dir(t: &mut Tree, parent: NodeId, name: &str) -> NodeId {
        t.add_child(parent, name.as_bytes(), 0, 0, NodeFlags::DIR)
    }

    /// A root with `big` (enough files below it to be kept, in two
    /// subfolders) and a small folder.
    fn sample() -> (Tree, NodeId, NodeId, NodeId, NodeId) {
        let mut t = Tree::new(b"C:\\");
        let big = dir(&mut t, NodeId::ROOT, "big");
        let a = dir(&mut t, big, "a");
        let b = dir(&mut t, big, "b");
        for i in 0..MEMO_MIN {
            file(
                &mut t,
                if i % 2 == 0 { a } else { b },
                &format!("f{i}.mp4"),
                1000 + i,
            );
        }
        let small = dir(&mut t, NodeId::ROOT, "small");
        file(&mut t, small, "notes.txt", 10);
        (t, big, a, b, small)
    }

    fn check(c: &mut TotalsCache, t: &Tree, dirs: &[NodeId]) {
        for &d in dirs {
            assert_eq!(c.get(t, d), brute(t, d), "totals of {}", t.name_lossy(d));
        }
    }

    #[test]
    fn keeps_big_folders_only() {
        let (t, big, a, b, small) = sample();
        let mut c = TotalsCache::new();
        let all = [NodeId::ROOT, big, a, b, small];
        check(&mut c, &t, &all);
        // root and big hold MEMO_MIN + entries; a, b and small hold fewer
        assert_eq!(c.len(), 2);
        let r = c.get(&t, NodeId::ROOT);
        assert_eq!((r.files, r.dirs), (MEMO_MIN + 1, 4));
        assert_eq!(r.bytes(SizeMode::Disk)[category::DOC as usize], 10);
        assert!(r.bytes(SizeMode::Logical)[category::VIDEO as usize] > MEMO_MIN * 1000);
    }

    #[test]
    fn follows_every_kind_of_change() {
        let (mut t, big, a, b, small) = sample();
        let mut c = TotalsCache::new();
        let all = [NodeId::ROOT, big, a, b, small];
        check(&mut c, &t, &all);

        // a file grows
        let f0 = t.find_child(a, b"f0.mp4").unwrap();
        t.set_file_size(f0, 5_000_000, 5_000_000);
        check(&mut c, &t, &all);
        // an empty file appears: no bytes move, the count does
        file(&mut t, b, "empty.log", 0);
        check(&mut c, &t, &all);
        // a rename changes the category, not the bytes
        assert!(t.move_node(f0, a, b"f0.zip"));
        check(&mut c, &t, &all);
        // a move between folders
        let f1 = t.find_child(b, b"f1.mp4").unwrap();
        assert!(t.move_node(f1, small, b"f1.mp4"));
        check(&mut c, &t, &all);
        // a folder moves out of a big one
        assert!(t.move_node(b, NodeId::ROOT, b"b"));
        check(&mut c, &t, &all);
        // deleted, then restored
        assert!(t.mark_deleted(a));
        check(&mut c, &t, &[NodeId::ROOT, big, b, small]);
        assert!(t.restore_deleted(a));
        check(&mut c, &t, &all);
        // new flags make a file an NTFS metafile (category system)
        let f2 = t.find_child(a, b"f2.mp4").unwrap();
        t.set_flags(f2, NodeFlags::METAFILE);
        check(&mut c, &t, &all);
    }

    #[test]
    fn starts_over_when_the_journal_overflows() {
        let (mut t, big, a, b, small) = sample();
        let mut c = TotalsCache::new();
        let all = [NodeId::ROOT, big, a, b, small];
        check(&mut c, &t, &all);
        let seq = t.change_seq();
        // more changes, in different folders, than the journal holds
        for i in 0..=crate::tree::JOURNAL_LEN {
            file(
                &mut t,
                if i % 2 == 0 { a } else { small },
                &format!("n{i}.txt"),
                1,
            );
        }
        assert!(t.touched_since(seq).is_none());
        check(&mut c, &t, &all);
    }

    #[test]
    fn a_change_deep_below_kept_folders_reaches_them() {
        let (mut t, big, a, _, _) = sample();
        let deep = dir(&mut t, a, "deep");
        let mut c = TotalsCache::new();
        let before = c.get(&t, NodeId::ROOT);
        file(&mut t, deep, "x.iso", 7);
        let after = c.get(&t, NodeId::ROOT);
        assert_eq!(after.files, before.files + 1);
        assert_eq!(after.disk[category::VDISK as usize], 7);
        assert_eq!(c.get(&t, big), brute(&t, big));
    }
}
