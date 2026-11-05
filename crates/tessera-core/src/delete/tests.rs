use super::*;
use crate::EntryBatch;

fn k(s: &str) -> Vec<u16> {
    path_key(&s.encode_utf16().collect::<Vec<_>>())
}

fn s(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

#[test]
fn keys_normalise() {
    assert_eq!(s(&k(r"C:\Windows\")), r"C:\WINDOWS");
    assert_eq!(s(&k(r"c:/windows/system32/")), r"C:\WINDOWS\SYSTEM32");
    assert_eq!(s(&k(r"\\?\C:\Windows")), r"C:\WINDOWS");
    assert_eq!(s(&k(r"\??\C:\Windows")), r"C:\WINDOWS");
    assert_eq!(s(&k(r"C:\")), "C:");
    assert_eq!(s(&k(r"C:\Users\.\me\..\me\\Docs")), r"C:\USERS\ME\DOCS");
    assert_eq!(s(&k(r"C:\..\..")), "C:");
    assert_eq!(s(&k(r"\\?\UNC\srv\share\x")), r"\\SRV\SHARE\X");
    assert_eq!(s(&k(r"\\srv\share\..\..")), r"\\SRV\SHARE");
    assert_eq!(s(&k("C:\\Users\\Ärger")), "C:\\USERS\\ÄRGER");
    assert_eq!(s(&k("/home/me")), r"\HOME\ME");
    // An unpaired surrogate survives unchanged.
    let odd = path_key(&[b'C' as u16, b':' as u16, BS, 0xD800, b'a' as u16]);
    assert_eq!(odd, vec![b'C' as u16, b':' as u16, BS, 0xD800, b'A' as u16]);
}

#[test]
fn ancestry_and_roots() {
    assert!(is_ancestor_or_self(&k(r"C:\Users"), &k(r"c:\users")));
    assert!(is_ancestor_or_self(&k(r"C:\Users"), &k(r"C:\Users\me")));
    assert!(is_ancestor_or_self(&k(r"C:\"), &k(r"C:\Users\me")));
    assert!(!is_ancestor_or_self(&k(r"C:\Users"), &k(r"C:\UsersX")));
    assert!(!is_ancestor_or_self(&k(r"C:\Users\me"), &k(r"C:\Users")));
    assert!(!is_ancestor_or_self(&k(r"D:\"), &k(r"C:\Users")));
    assert!(is_root_key(&k(r"C:\")));
    assert!(is_root_key(&k(r"d:")));
    assert!(is_root_key(&k(r"\\srv\share\")));
    assert!(is_root_key(&k(r"\\?\UNC\srv\share")));
    assert!(is_root_key(&k("/")));
    assert!(!is_root_key(&k(r"\\srv\share\dir")));
    assert!(!is_root_key(&k(r"C:\x")));
    assert!(is_root_name(b"C:\\"));
    assert!(is_root_name(b"/"));
    assert!(!is_root_name(b"D:\\Games"));
}

#[test]
fn covered_paths() {
    let keys: Vec<Vec<u16>> = [
        r"C:\a\b",
        r"C:\a",
        r"C:\a b",
        r"c:\A\b\c.txt",
        r"C:\a",
        r"C:\ab\c",
    ]
    .iter()
    .map(|p| k(p))
    .collect();
    assert_eq!(covered(&keys), vec![true, false, false, true, true, false]);
}

#[test]
fn restore_deleted_reverses_mark_deleted() {
    let mut t = Tree::new(b"R:\\");
    let d = t.add_child(NodeId::ROOT, b"d", 0, 0, NodeFlags::DIR, 0);
    let mut b = EntryBatch::new();
    b.push_wtf8(b"a", 4096, 100, NodeFlags::NONE, 0);
    b.push_wtf8(b"b", 8192, 5000, NodeFlags::NONE, 0);
    let r = t.add_children(d, &b);
    let (a, bb) = (NodeId(r.start), NodeId(r.start + 1));
    t.add_child(NodeId::ROOT, b"keep", 4096, 1, NodeFlags::NONE, 0);

    assert!(t.mark_deleted(a)); // deleted before its folder
    assert!(t.mark_deleted(d));
    assert_eq!(t.node(NodeId::ROOT).size, 4096);
    assert!(
        !t.restore_deleted(bb),
        "only the deleted node itself can be restored"
    );
    assert!(!t.restore_deleted(NodeId::ROOT));
    assert!(t.restore_deleted(d));
    assert!(!t.restore_deleted(d));
    t.check_invariants().unwrap();
    assert_eq!(t.node(NodeId::ROOT).size, 4096 + 8192);
    assert!(t.is_deleted(a) && !t.is_deleted(bb));
    assert!(t.restore_deleted(a));
    t.check_invariants().unwrap();
    assert_eq!(t.node(NodeId::ROOT).logical, 5101);
    assert_eq!(t.child_count(d), 2);
}

#[test]
fn undo_store_keeps_recent_tokens() {
    let store = UndoStore::new();
    let item = |n| UndoItem {
        node: NodeId(n),
        path: PathBuf::from("x"),
        item: vec![1, 2],
    };
    let first = store.put(vec![item(1)]);
    assert!(store.contains(first));
    let mut last = first;
    for i in 0..UndoStore::CAPACITY as u32 {
        last = store.put(vec![item(i)]);
    }
    assert!(!store.contains(first), "the oldest token is dropped");
    assert!(store.contains(last));
    assert_eq!(
        store.take(last).unwrap()[0].node,
        NodeId(UndoStore::CAPACITY as u32 - 1)
    );
    assert!(store.take(last).is_none());
    store.put_back(last, vec![item(7)]);
    assert!(store.contains(last));
    store.clear();
    assert!(!store.contains(last));
}

#[cfg(not(windows))]
mod unix {
    use super::*;

    #[test]
    fn protection() {
        assert_eq!(lock_reason(Path::new("/")), Some(reasons::DRIVE_ROOT));
        assert_eq!(lock_reason(Path::new("")), Some(reasons::DRIVE_ROOT));
        // A bare drive letter is the drive, not its current directory.
        assert_eq!(lock_reason(Path::new("C:")), Some(reasons::DRIVE_ROOT));
        assert_eq!(lock_reason(Path::new("d:")), Some(reasons::DRIVE_ROOT));
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        if home.parent().is_none() {
            return;
        }
        assert_eq!(lock_reason(&home), Some(reasons::PROFILE));
        assert_eq!(
            lock_reason(&home.join("x").join("..")),
            Some(reasons::PROFILE)
        );
        if let Some(parent) = home.parent().filter(|p| p.parent().is_some()) {
            assert_eq!(lock_reason(parent), Some(reasons::CONTAINS));
        }
        assert!(is_protected(&home));
        assert!(!is_protected(&home.join("some-file-in-home")));

        // The same, as node ids of a tree rooted at `/`.
        let mut t = Tree::new(b"/");
        let mut cur = NodeId::ROOT;
        let mut chain = vec![];
        for comp in home.components().skip(1) {
            let name = comp.as_os_str().to_string_lossy().into_owned();
            cur = t.add_child(cur, name.as_bytes(), 0, 0, NodeFlags::DIR, 0);
            chain.push(cur.0);
        }
        let inside = t.add_child(cur, b"file", 1, 1, NodeFlags::NONE, 0);
        let other = t.add_child(NodeId::ROOT, b"tessera-elsewhere", 1, 1, NodeFlags::NONE, 0);
        let p = protected_nodes(&t);
        assert!(p.contains(&0), "the volume root");
        for id in chain {
            assert!(p.contains(&id), "{id} on the way to HOME");
        }
        assert!(!p.contains(&inside.0) && !p.contains(&other.0));
    }

    fn temp(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("tessera-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// Exercises the tree bookkeeping through the `std::fs` fallback.
    #[test]
    fn purge_updates_tree() {
        let base = temp("purge-unit");
        std::fs::create_dir_all(base.join("dir/sub")).unwrap();
        std::fs::write(base.join("a.bin"), [0u8; 10]).unwrap();
        std::fs::write(base.join("dir/b.bin"), [0u8; 20]).unwrap();
        std::fs::write(base.join("dir/sub/c.bin"), [0u8; 30]).unwrap();
        std::fs::write(base.join("keep.bin"), [0u8; 40]).unwrap();

        let mut t = Tree::new(base.to_str().unwrap().as_bytes());
        let mut b = EntryBatch::new();
        b.push_wtf8(b"a.bin", 4096, 10, NodeFlags::NONE, 0);
        b.push_wtf8(b"dir", 0, 0, NodeFlags::DIR, 0);
        b.push_wtf8(b"keep.bin", 4096, 40, NodeFlags::NONE, 0);
        b.push_wtf8(b"ghost.bin", 4096, 50, NodeFlags::NONE, 0); // not on disk
        let r = t.add_children(NodeId::ROOT, &b);
        let (a, dir, keep, ghost) = (
            NodeId(r.start),
            NodeId(r.start + 1),
            NodeId(r.start + 2),
            NodeId(r.start + 3),
        );
        b.clear();
        b.push_wtf8(b"b.bin", 4096, 20, NodeFlags::NONE, 0);
        b.push_wtf8(b"sub", 0, 0, NodeFlags::DIR, 0);
        let r = t.add_children(dir, &b);
        let sub = NodeId(r.start + 1);
        b.clear();
        b.push_wtf8(b"c.bin", 4096, 30, NodeFlags::NONE, 0);
        let c = NodeId(t.add_children(sub, &b).start);
        let tree = t.into_shared();

        let opts = DeleteOptions {
            silent: true,
            ..Default::default()
        };
        let res = purge(&tree, &[c, a, dir, NodeId::ROOT, a, ghost], &opts).unwrap();
        assert_eq!(res.ok, vec![c, a, dir, ghost]);
        assert_eq!(
            res.failed,
            vec![Failure {
                id: NodeId::ROOT,
                reason: reasons::SCAN_ROOT.into()
            }]
        );
        assert_eq!(res.bytes, 4096 * 4);
        assert_eq!(res.undo, None);
        assert!(!base.join("a.bin").exists() && !base.join("dir").exists());
        assert!(base.join("keep.bin").exists());
        let t = tree.read().unwrap();
        assert_eq!(t.node(NodeId::ROOT).size, 4096);
        assert_eq!(t.node(NodeId::ROOT).logical, 40);
        assert_eq!(t.children(NodeId::ROOT).collect::<Vec<_>>(), vec![keep]);
        t.check_invariants().unwrap();
        drop(t);

        // The recycle bin is Windows only, and nothing changes.
        let undo = UndoStore::new();
        let err = recycle(&tree, &[keep], &opts, &undo);
        assert!(matches!(err, Err(crate::Error::Unsupported(_))));
        assert!(base.join("keep.bin").exists());
        assert!(restore(&tree, 1, &opts, &undo).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn partial_delete_is_reconciled() {
        let base = temp("partial-unit");
        std::fs::create_dir_all(base.join("d")).unwrap();
        std::fs::write(base.join("d/stays.bin"), [0u8; 1]).unwrap();
        let mut t = Tree::new(base.to_str().unwrap().as_bytes());
        let d = t.add_child(NodeId::ROOT, b"d", 0, 0, NodeFlags::DIR, 0);
        let mut b = EntryBatch::new();
        b.push_wtf8(b"stays.bin", 4096, 1, NodeFlags::NONE, 0);
        b.push_wtf8(b"vanished.bin", 8192, 5000, NodeFlags::NONE, 0);
        b.push_wtf8(b"gone-dir", 0, 0, NodeFlags::DIR, 0);
        let r = t.add_children(d, &b);
        t.add_child(
            NodeId(r.start + 2),
            b"deep.bin",
            4096,
            7,
            NodeFlags::NONE,
            0,
        );
        let tree = t.into_shared();

        // `d` survives (as if the user skipped `stays.bin`), so its
        // contents are checked one by one.
        let mut gone = Vec::new();
        let mut budget = 10;
        assert!(reconcile(&tree, d, base.join("d"), &mut gone, &mut budget));
        assert_eq!(gone, vec![NodeId(r.start + 1), NodeId(r.start + 2)]);
        let mut budget = 2;
        assert!(!reconcile(
            &tree,
            d,
            base.join("d"),
            &mut Vec::new(),
            &mut budget
        ));
        let _ = std::fs::remove_dir_all(&base);
    }
}
