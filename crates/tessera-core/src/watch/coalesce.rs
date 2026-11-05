//! Collect and coalesce: the portable half of the watcher.
//!
//! The collector thread turns raw notifications into [`Event`]s. The
//! applier thread feeds them to a [`Coalescer`] for one throttle window and
//! then takes a [`Batch`] in which each entry appears once, in its final
//! state:
//!
//! * path events (`ReadDirectoryChangesW`) are keyed by their case-folded
//!   path, so any number of changes to one path collapse into one entry
//!   holding the last known state;
//! * renames are kept in order, and pending entries below a renamed
//!   directory are moved along with it, so every entry names the path the
//!   item has *after* all renames of the batch;
//! * USN records are keyed by file reference number; reasons accumulate and
//!   the last record gives the final name and parent.

// Without Windows only the unit tests drive this code.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::{BTreeMap, HashMap};

use super::apply::Stat;
use crate::NodeId;
use crate::wtf8;

/// What a notification tells about an entry's state right after a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Info {
    /// Not known: read the entry's attributes before applying.
    Unknown,
    /// The entry no longer exists.
    Gone,
    /// The entry exists with these attributes and sizes.
    Present(Stat),
}

/// One USN change journal record, reduced to what the watcher uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UsnRecord {
    pub frn: u64,
    pub parent_frn: u64,
    pub reason: u32,
    pub attributes: u32,
    /// The entry's own name (not a path), as WTF-8.
    pub name: Vec<u8>,
}

/// A raw change, as reported by a collector (or requested by the app).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// Something happened to the entry at `path` (relative to the watched
    /// root, WTF-8, `\`-separated).
    Changed { path: Vec<u8>, info: Info },
    /// The entry at `from` is now at `to`; `info` describes it at `to`.
    Renamed {
        from: Vec<u8>,
        to: Vec<u8>,
        info: Info,
    },
    /// A USN change journal record.
    Usn(UsnRecord),
    /// Changes were lost below this directory (relative path; empty for
    /// the root): the buffer overflowed or the journal has a gap.
    Overflow(Vec<u8>),
    /// The application asked for this directory to be rescanned.
    Rescan(NodeId),
    /// The collector stopped for good; no more events will come.
    Failed(String),
}

/// The final state of one path after a batch of path events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PathChange {
    /// Relative path after all renames of the batch, as reported.
    pub path: Vec<u8>,
    pub info: Info,
}

/// A rename, in the order in which it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rename {
    pub from: Vec<u8>,
    pub to: Vec<u8>,
}

/// The accumulated USN records of one file reference number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UsnChange {
    pub frn: u64,
    /// Parent and name after the last record of the batch.
    pub parent_frn: u64,
    pub name: Vec<u8>,
    pub attributes: u32,
    /// All reasons of the batch, OR-ed together.
    pub reasons: u32,
    /// The reasons of the last record (a delete shows up here).
    pub last_reason: u32,
    /// Where the entry was before its first rename in the batch, if any.
    pub old_location: Option<(u64, Vec<u8>)>,
}

/// Everything gathered during one throttle window.
#[derive(Debug, Default)]
pub(crate) struct Batch {
    pub paths: Vec<PathChange>,
    pub renames: Vec<Rename>,
    /// In order of first appearance.
    pub usn: Vec<UsnChange>,
    pub overflow: Vec<Vec<u8>>,
    pub rescan: Vec<NodeId>,
    pub failed: Option<String>,
    /// Raw events received.
    pub events: usize,
}

/// Collapses events until the batch is taken.
#[derive(Default)]
pub(crate) struct Coalescer {
    paths: BTreeMap<Vec<u8>, PathChange>,
    renames: Vec<Rename>,
    usn: Vec<UsnChange>,
    usn_pos: HashMap<u64, usize>,
    overflow: Vec<Vec<u8>>,
    rescan: Vec<NodeId>,
    failed: Option<String>,
    events: usize,
}

impl Coalescer {
    pub fn extend(&mut self, events: impl IntoIterator<Item = Event>) {
        for e in events {
            self.push(e);
        }
    }

    pub fn push(&mut self, event: Event) {
        self.events += 1;
        match event {
            Event::Changed { path, info } => self.set(path, info),
            Event::Renamed { from, to, info } => self.rename(from, to, info),
            Event::Usn(r) => self.push_usn(r),
            Event::Overflow(dir) => {
                if !self.overflow.contains(&dir) {
                    self.overflow.push(dir);
                }
            }
            Event::Rescan(id) => {
                if !self.rescan.contains(&id) {
                    self.rescan.push(id);
                }
            }
            Event::Failed(msg) => self.failed = Some(msg),
        }
    }

    /// Hands out everything gathered so far and starts over.
    pub fn take(&mut self) -> Batch {
        let c = std::mem::take(self);
        Batch {
            paths: c.paths.into_values().collect(),
            renames: c.renames,
            usn: c.usn,
            overflow: c.overflow,
            rescan: c.rescan,
            failed: c.failed,
            events: c.events,
        }
    }

    fn set(&mut self, path: Vec<u8>, info: Info) {
        let key = fold_key(&path);
        if key.is_empty() {
            return; // the root itself: nothing to record
        }
        self.paths.insert(key, PathChange { path, info });
    }

    fn rename(&mut self, from: Vec<u8>, to: Vec<u8>, info: Info) {
        let from_key = fold_key(&from);
        let to_key = fold_key(&to);
        if from_key.is_empty() || to_key.is_empty() {
            self.set(from, Info::Gone);
            self.set(to, info);
            return;
        }
        if from_key != to_key {
            // Pending changes below `from` now live below `to`. Keys are
            // `\`-separated, so the descendants of `K` are exactly the keys
            // in `K\` .. `K]` (']' follows '\' in ASCII).
            let mut lo = from_key.clone();
            lo.push(b'\\');
            let mut hi = from_key.clone();
            hi.push(b']');
            let below: Vec<Vec<u8>> = self.paths.range(lo..hi).map(|(k, _)| k.clone()).collect();
            let depth = components(&from).count();
            for k in below {
                let c = self.paths.remove(&k).expect("key listed above");
                let mut p = to.clone();
                p.push(b'\\');
                p.extend_from_slice(skip_components(&c.path, depth));
                self.set(p, c.info);
            }
            // Nothing is left at `from` unless a later event says so. This
            // also removes a node the rename could not find (e.g. because
            // the event used a short 8.3 name).
            self.paths.insert(
                from_key,
                PathChange {
                    path: from.clone(),
                    info: Info::Gone,
                },
            );
        }
        self.renames.push(Rename {
            from,
            to: to.clone(),
        });
        self.set(to, info);
    }

    fn push_usn(&mut self, r: UsnRecord) {
        use super::usn::reason::{RENAME_NEW_NAME, RENAME_OLD_NAME};
        // The record written before a rename carries the old name. Reasons
        // may accumulate into later records, so only a record without the
        // new-name reason is taken as the old location.
        let is_old_name = r.reason & RENAME_OLD_NAME != 0 && r.reason & RENAME_NEW_NAME == 0;
        match self.usn_pos.get(&r.frn) {
            Some(&i) => {
                let e = &mut self.usn[i];
                if is_old_name && e.old_location.is_none() {
                    e.old_location = Some((r.parent_frn, r.name.clone()));
                }
                e.parent_frn = r.parent_frn;
                e.name = r.name;
                e.attributes = r.attributes;
                e.reasons |= r.reason;
                e.last_reason = r.reason;
            }
            None => {
                self.usn_pos.insert(r.frn, self.usn.len());
                let old_location = is_old_name.then(|| (r.parent_frn, r.name.clone()));
                self.usn.push(UsnChange {
                    frn: r.frn,
                    parent_frn: r.parent_frn,
                    name: r.name,
                    attributes: r.attributes,
                    reasons: r.reason,
                    last_reason: r.reason,
                    old_location,
                });
            }
        }
    }
}

/// The non-empty components of a `\`- or `/`-separated path.
pub(crate) fn components(path: &[u8]) -> impl Iterator<Item = &[u8]> {
    path.split(|&b| b == b'\\' || b == b'/')
        .filter(|c| !c.is_empty() && *c != b".")
}

/// Byte ranges of the components of `path`.
pub(crate) fn component_spans(path: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..=path.len() {
        if i == path.len() || path[i] == b'\\' || path[i] == b'/' {
            let c = &path[start..i];
            if !c.is_empty() && c != b"." {
                out.push((start, i));
            }
            start = i + 1;
        }
    }
    out
}

/// What follows the first `n` components of `path` (without the separator).
pub(crate) fn skip_components(path: &[u8], n: usize) -> &[u8] {
    match component_spans(path).get(n) {
        Some(&(start, _)) => &path[start..],
        None => &[],
    }
}

/// Splits a relative path into its parent part and its last component.
pub(crate) fn split_last(path: &[u8]) -> (&[u8], &[u8]) {
    match component_spans(path).last() {
        Some(&(start, end)) => (&path[..start], &path[start..end]),
        None => (&[], &[]),
    }
}

/// A case-folded, `\`-separated key for `path`, equal for two paths exactly
/// when NTFS considers them the same (see [`wtf8::eq_ignore_case`]).
pub(crate) fn fold_key(path: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(path.len());
    for comp in components(path) {
        if !out.is_empty() {
            out.push(b'\\');
        }
        if comp.is_ascii() {
            out.extend(comp.iter().map(|b| b.to_ascii_uppercase()));
            continue;
        }
        for cp in wtf8::code_points(comp) {
            let folded = match char::from_u32(cp) {
                Some(c) => {
                    let mut up = c.to_uppercase();
                    match (up.next(), up.next()) {
                        (Some(u), None) => u as u32,
                        _ => cp,
                    }
                }
                None => cp,
            };
            match char::from_u32(folded) {
                Some(c) => {
                    let mut b = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                }
                // A lone surrogate: three bytes, as WTF-8 encodes it.
                None => out.extend_from_slice(&[
                    0xE0 | (folded >> 12) as u8,
                    0x80 | ((folded >> 6) & 0x3F) as u8,
                    0x80 | (folded & 0x3F) as u8,
                ]),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn present(size: u64) -> Info {
        Info::Present(Stat {
            attributes: 0x20,
            reparse_tag: 0,
            size,
            logical: size,
            mtime: 0,
        })
    }

    fn changed(path: &str, info: Info) -> Event {
        Event::Changed {
            path: path.as_bytes().to_vec(),
            info,
        }
    }

    fn renamed(from: &str, to: &str, info: Info) -> Event {
        Event::Renamed {
            from: from.as_bytes().to_vec(),
            to: to.as_bytes().to_vec(),
            info,
        }
    }

    fn state(b: &Batch) -> Vec<(String, Info)> {
        let mut v: Vec<_> = b
            .paths
            .iter()
            .map(|p| (String::from_utf8(p.path.clone()).unwrap(), p.info))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    #[test]
    fn repeated_changes_collapse_to_the_last_state() {
        let mut c = Coalescer::default();
        for i in 0..1000 {
            c.push(changed("logs\\app.log", present(i)));
        }
        c.push(changed("LOGS\\APP.LOG", present(4242)));
        let b = c.take();
        assert_eq!(b.events, 1001);
        assert_eq!(state(&b), vec![("LOGS\\APP.LOG".into(), present(4242))]);
        assert_eq!(c.take().events, 0);
    }

    #[test]
    fn create_then_delete_leaves_a_gone_entry() {
        let mut c = Coalescer::default();
        c.push(changed("tmp.dat", present(1)));
        c.push(changed("tmp.dat", present(99)));
        c.push(changed("tmp.dat", Info::Gone));
        let b = c.take();
        assert_eq!(state(&b), vec![("tmp.dat".into(), Info::Gone)]);
    }

    #[test]
    fn rename_moves_pending_descendants() {
        let mut c = Coalescer::default();
        c.push(changed("a\\x.txt", present(10)));
        c.push(changed("a\\sub\\y.txt", present(20)));
        c.push(changed("ab.txt", present(30))); // shares a prefix, not a child
        c.push(renamed("A", "b", Info::Unknown));
        c.push(changed("b\\z.txt", present(40)));
        let b = c.take();
        assert_eq!(
            state(&b),
            vec![
                ("A".into(), Info::Gone),
                ("ab.txt".into(), present(30)),
                ("b".into(), Info::Unknown),
                ("b\\sub\\y.txt".into(), present(20)),
                ("b\\x.txt".into(), present(10)),
                ("b\\z.txt".into(), present(40)),
            ]
        );
        assert_eq!(
            b.renames,
            vec![Rename {
                from: b"A".to_vec(),
                to: b"b".to_vec()
            }]
        );
    }

    #[test]
    fn rename_chains_and_swaps() {
        let mut c = Coalescer::default();
        c.push(renamed("a", "t", present(1)));
        c.push(renamed("b", "a", present(2)));
        c.push(renamed("t", "b", present(1)));
        let b = c.take();
        assert_eq!(
            state(&b),
            vec![
                ("a".into(), present(2)),
                ("b".into(), present(1)),
                ("t".into(), Info::Gone),
            ]
        );
        assert_eq!(b.renames.len(), 3);
    }

    #[test]
    fn case_only_rename_keeps_one_entry() {
        let mut c = Coalescer::default();
        c.push(renamed("readme.md", "README.md", present(5)));
        let b = c.take();
        assert_eq!(state(&b), vec![("README.md".into(), present(5))]);
        assert_eq!(b.renames.len(), 1);
    }

    #[test]
    fn overflow_rescan_and_failure_are_kept() {
        let mut c = Coalescer::default();
        c.push(Event::Overflow(Vec::new()));
        c.push(Event::Overflow(Vec::new()));
        c.push(Event::Rescan(NodeId(3)));
        c.push(Event::Rescan(NodeId(3)));
        c.push(Event::Failed("gone".into()));
        let b = c.take();
        assert_eq!(b.overflow, vec![Vec::<u8>::new()]);
        assert_eq!(b.rescan, vec![NodeId(3)]);
        assert_eq!(b.failed.as_deref(), Some("gone"));
    }

    #[test]
    fn usn_records_accumulate_per_file() {
        use super::super::usn::reason::*;
        let rec = |frn, parent, reason, name: &str| {
            Event::Usn(UsnRecord {
                frn,
                parent_frn: parent,
                reason,
                attributes: 0x20,
                name: name.as_bytes().to_vec(),
            })
        };
        let mut c = Coalescer::default();
        c.push(rec(7, 5, DATA_EXTEND, "a.txt"));
        c.push(rec(8, 5, FILE_CREATE, "new.txt"));
        c.push(rec(7, 5, DATA_EXTEND | RENAME_OLD_NAME, "a.txt"));
        c.push(rec(7, 6, DATA_EXTEND | RENAME_NEW_NAME, "b.txt"));
        c.push(rec(8, 5, FILE_CREATE | FILE_DELETE | CLOSE, "new.txt"));
        let b = c.take();
        assert_eq!(b.usn.len(), 2);
        let a = &b.usn[0];
        assert_eq!((a.frn, a.parent_frn, &a.name[..]), (7, 6, &b"b.txt"[..]));
        assert_eq!(a.reasons, DATA_EXTEND | RENAME_OLD_NAME | RENAME_NEW_NAME);
        assert_eq!(a.old_location, Some((5, b"a.txt".to_vec())));
        assert_eq!(b.usn[1].last_reason & FILE_DELETE, FILE_DELETE);
    }

    #[test]
    fn keys_fold_case_and_separators() {
        assert_eq!(fold_key(b"a/b\\\\c"), b"A\\B\\C");
        assert_eq!(
            fold_key("Übung\\straße".as_bytes()),
            "ÜBUNG\\STRAßE".as_bytes()
        );
        assert_eq!(fold_key(b""), b"");
        // Lone surrogates survive folding.
        let lone = wtf8::from_utf16(&[0x61, 0xD800]);
        assert_eq!(fold_key(&lone), wtf8::from_utf16(&[0x41, 0xD800]));
    }

    #[test]
    fn path_helpers() {
        assert_eq!(skip_components(b"a\\b\\c.txt", 1), b"b\\c.txt");
        assert_eq!(skip_components(b"a\\b\\c.txt", 3), b"");
        assert_eq!(skip_components(b"\\a\\b", 0), b"a\\b");
        assert_eq!(split_last(b"a\\b\\c.txt"), (&b"a\\b\\"[..], &b"c.txt"[..]));
        assert_eq!(split_last(b"c.txt"), (&b""[..], &b"c.txt"[..]));
        assert_eq!(component_spans(b"\\a\\\\b"), vec![(1, 2), (4, 5)]);
    }
}
