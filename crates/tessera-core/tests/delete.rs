//! Deleting through the Windows shell (`IFileOperation`): `purge`,
//! `recycle`, `restore` and `empty_recycle_bin`, plus the protection list.
//!
//! Every test works on its own folder below the user's temp directory and
//! runs with `silent: true`, so no dialog can block an unattended run.
//!
//! Recycle-bin runs: the temp directory is on the system drive, which has a
//! recycle bin for every user profile, including CI runner accounts, so the
//! recycle tests expect the items to land there and come back.
//! `empty_recycle_bin` empties the whole bin of the drive, so its test only
//! runs on GitHub Actions runners (`GITHUB_ACTIONS=true`), never on a
//! developer's machine.
//!
//! Wine's shell does not implement `IFileOperation`, so under wine the
//! shell-based tests print a note and return early. They must pass on real
//! Windows.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tessera_core::delete::{
    DeleteMode, DeleteOptions, Failure, UndoStore, delete_paths, empty_recycle_bin, is_protected,
    lock_reason, purge, reasons, recycle, restore,
};
use tessera_core::{EntryBatch, NodeFlags, NodeId, SharedTree, Tree};

fn under_wine() -> bool {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::core::{s, w};
    unsafe {
        GetModuleHandleW(w!("ntdll.dll"))
            .map(|ntdll| GetProcAddress(ntdll, s!("wine_get_version")).is_some())
            .unwrap_or(false)
    }
}

/// A fresh directory below the temp directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("tessera-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    fn file(&self, rel: &str, len: usize) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, contents(rel, len)).unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Distinct contents per file, to check that a restore brings back the
/// right bytes.
fn contents(rel: &str, len: usize) -> Vec<u8> {
    rel.bytes().cycle().take(len).collect()
}

/// A tree of `root` read with `std::fs`, so these tests do not depend on
/// the scanner. On-disk sizes are rounded up to 4 KiB clusters.
fn tree_of(root: &Path) -> SharedTree {
    fn fill(t: &mut Tree, dir: NodeId, path: &Path) {
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        let mut b = EntryBatch::new();
        for e in &entries {
            let md = e.metadata().unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            if md.is_dir() {
                b.push_wtf8(name.as_bytes(), 0, 0, NodeFlags::DIR, 0);
            } else {
                let len = md.len();
                b.push_wtf8(
                    name.as_bytes(),
                    len.div_ceil(4096) * 4096,
                    len,
                    NodeFlags::NONE,
                    0,
                );
            }
        }
        let ids = t.add_children(dir, &b);
        for (e, id) in entries.iter().zip(ids) {
            if e.file_type().unwrap().is_dir() {
                fill(t, NodeId(id), &e.path());
            }
        }
    }
    let mut t = Tree::for_path(root);
    fill(&mut t, NodeId::ROOT, root);
    t.check_invariants().unwrap();
    t.into_shared()
}

fn find(tree: &SharedTree, rel: &str) -> NodeId {
    tree.read()
        .unwrap()
        .lookup_relative(rel.as_bytes())
        .unwrap_or_else(|| panic!("{rel} not in tree"))
}

fn silent() -> DeleteOptions {
    DeleteOptions {
        owner_hwnd: 0,
        silent: true,
    }
}

/// Deletes a probe file and reports whether the shell can delete at all
/// here; prints a note and returns false under wine.
fn shell_works() -> bool {
    if !under_wine() {
        return true;
    }
    let dir = TempDir::new("probe");
    let probe = dir.file("probe.txt", 1);
    match delete_paths(&[probe], DeleteMode::Permanent, &silent()) {
        Ok(gone) if gone == [true] => true,
        other => {
            println!("note: skipped, wine's IFileOperation does not delete ({other:?})");
            false
        }
    }
}

/// Serialises the tests that use the recycle bin, so emptying it cannot
/// take another test's items.
fn bin_lock() -> MutexGuard<'static, ()> {
    static BIN: Mutex<()> = Mutex::new(());
    BIN.lock().unwrap_or_else(PoisonError::into_inner)
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

#[test]
fn protected_folders() {
    let windir = env_path("SystemRoot").expect("SystemRoot");
    let drive = PathBuf::from(format!("{}\\", std::env::var("SystemDrive").unwrap()));
    let profile = env_path("USERPROFILE").expect("USERPROFILE");
    let users = profile.parent().unwrap().to_path_buf();

    // The reasons shown to the user.
    let expect: Vec<(PathBuf, &str)> = [
        (Some(drive.clone()), reasons::DRIVE_ROOT),
        (Some(windir.clone()), reasons::WINDOWS),
        (Some(windir.join("System32")), reasons::WINDOWS),
        (env_path("ProgramFiles"), reasons::PROGRAMS),
        (env_path("ProgramData"), reasons::APP_DATA),
        (env_path("LOCALAPPDATA"), reasons::APP_DATA),
        (env_path("APPDATA"), reasons::APP_DATA),
        (Some(users.clone()), reasons::PROFILES),
        (Some(profile.clone()), reasons::PROFILE),
        (Some(std::env::temp_dir()), reasons::TEMP),
        // AppData holds Roaming and Local.
        (
            env_path("APPDATA").map(|p| p.parent().unwrap().to_path_buf()),
            reasons::CONTAINS,
        ),
    ]
    .into_iter()
    .filter_map(|(p, r)| p.map(|p| (p, r)))
    .collect();
    for (p, r) in &expect {
        assert_eq!(lock_reason(p), Some(*r), "{}", p.display());
    }

    let mut must: Vec<PathBuf> = vec![PathBuf::from(std::env::var("SystemDrive").unwrap())];
    for var in [
        "ProgramFiles(x86)",
        "ProgramW6432",
        "CommonProgramFiles",
        "PUBLIC",
    ] {
        if let Some(p) = env_path(var) {
            must.push(p);
        }
    }
    // Spelling variants.
    let w = windir.to_string_lossy().into_owned();
    must.push(PathBuf::from(w.to_lowercase()));
    must.push(PathBuf::from(w.to_uppercase() + "\\"));
    must.push(PathBuf::from(w.replace('\\', "/")));
    must.push(PathBuf::from(format!(r"\\?\{w}")));
    must.push(PathBuf::from(format!(r"{w}\System32\..")));
    must.push(PathBuf::from(format!(r"{w}\.")));
    if let Some(short) = short_name(&env_path("ProgramFiles").unwrap()) {
        must.push(short);
    }
    for p in &must {
        assert!(is_protected(p), "{} should be protected", p.display());
    }
    assert_eq!(lock_reason(Path::new("")), Some(reasons::DRIVE_ROOT));

    // Contents of protected folders are not protected.
    let tmp = TempDir::new("protected");
    let file = tmp.file("x.txt", 1);
    let free: Vec<PathBuf> = vec![
        tmp.0.clone(),
        file,
        windir.join("Temp"),
        windir
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts"),
        profile.join("tessera-no-such-file.txt"),
        drive.join("tessera-no-such-dir"),
    ];
    for p in &free {
        assert_eq!(lock_reason(p), None, "{}", p.display());
    }
}

/// The 8.3 short name of `p`, if the volume has them and it differs.
fn short_name(p: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows::Win32::Storage::FileSystem::GetShortPathNameW;
    use windows::core::PCWSTR;
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buf)) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let short = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
    (short != p).then_some(short)
}

#[test]
fn purge_updates_tree() {
    if !shell_works() {
        return;
    }
    let dir = TempDir::new("purge");
    dir.file("a.txt", 1000);
    dir.file("folder/b.bin", 5000);
    dir.file("folder/sub/c.bin", 3000);
    dir.file("keep.txt", 10);
    let tree = tree_of(&dir.0);
    let a = find(&tree, "a.txt");
    let folder = find(&tree, "folder");
    let c = find(&tree, r"folder\sub\c.bin");
    let keep = find(&tree, "keep.txt");
    assert_eq!(tree.read().unwrap().node(NodeId::ROOT).logical, 9010);

    let res = purge(&tree, &[c, a, folder, NodeId::ROOT, a], &silent()).unwrap();
    // `c` goes with `folder`; the duplicate `a` is reported once.
    assert_eq!(res.ok, vec![c, a, folder], "{res:?}");
    assert_eq!(
        res.failed,
        vec![Failure {
            id: NodeId::ROOT,
            reason: reasons::SCAN_ROOT.into()
        }]
    );
    assert_eq!(res.bytes, 4 * 4096);
    assert_eq!(res.undo, None);

    assert!(!dir.0.join("a.txt").exists());
    assert!(!dir.0.join("folder").exists());
    assert!(dir.0.join("keep.txt").exists());
    let t = tree.read().unwrap();
    assert_eq!(t.children(NodeId::ROOT).collect::<Vec<_>>(), vec![keep]);
    assert_eq!(t.node(NodeId::ROOT).logical, 10);
    assert_eq!(t.node(NodeId::ROOT).size, 4096);
    assert!(t.is_deleted(c));
    t.check_invariants().unwrap();
    drop(t);

    // Already deleted: nothing to do, nothing reported.
    let again = purge(&tree, &[a], &silent()).unwrap();
    assert!(again.ok.is_empty() && again.failed.is_empty() && again.bytes == 0);
}

#[test]
fn recycle_and_restore() {
    let _bin = bin_lock();
    if !shell_works() {
        return;
    }
    let dir = TempDir::new("recycle");
    dir.file("doc.txt", 1000);
    dir.file("folder/inner.bin", 5000);
    dir.file("keep.txt", 10);
    let tree = tree_of(&dir.0);
    let doc = find(&tree, "doc.txt");
    let folder = find(&tree, "folder");
    let inner = find(&tree, r"folder\inner.bin");
    let before = *tree.read().unwrap().node(NodeId::ROOT);
    let kids_before = tree.read().unwrap().child_count(NodeId::ROOT);
    let undo = UndoStore::new();

    let res = recycle(&tree, &[doc, folder], &silent(), &undo).unwrap();
    assert_eq!(res.ok, vec![doc, folder], "{res:?}");
    assert!(res.failed.is_empty());
    assert_eq!(res.bytes, 4096 + 8192);
    let token = res.undo.expect("an undo token for recycled items");
    assert!(undo.contains(token));
    assert!(!dir.0.join("doc.txt").exists() && !dir.0.join("folder").exists());
    {
        let t = tree.read().unwrap();
        assert!(t.is_deleted(doc) && t.is_deleted(inner));
        assert_eq!(t.node(NodeId::ROOT).logical, 10);
        t.check_invariants().unwrap();
    }

    let back = restore(&tree, token, &silent(), &undo).unwrap();
    assert_eq!(back.ok, vec![doc, folder], "{back:?}");
    assert!(back.failed.is_empty());
    assert_eq!(back.bytes, 4096 + 8192);
    assert!(!undo.contains(token), "a fully restored token is used up");
    assert_eq!(
        std::fs::read(dir.0.join("doc.txt")).unwrap(),
        contents("doc.txt", 1000)
    );
    assert_eq!(
        std::fs::read(dir.0.join(r"folder\inner.bin")).unwrap(),
        contents(r"folder/inner.bin", 5000)
    );
    let t = tree.read().unwrap();
    assert!(!t.is_deleted(doc) && !t.is_deleted(inner));
    let after = t.node(NodeId::ROOT);
    assert_eq!((after.size, after.logical), (before.size, before.logical));
    assert_eq!(t.child_count(NodeId::ROOT), kids_before);
    t.check_invariants().unwrap();
    drop(t);
    assert!(
        restore(&tree, token, &silent(), &undo).is_err(),
        "unknown token"
    );
}

#[test]
fn restore_never_overwrites() {
    let _bin = bin_lock();
    if !shell_works() {
        return;
    }
    let dir = TempDir::new("taken");
    let x = dir.file("x.txt", 100);
    let tree = tree_of(&dir.0);
    let id = find(&tree, "x.txt");
    let undo = UndoStore::new();
    let token = recycle(&tree, &[id], &silent(), &undo)
        .unwrap()
        .undo
        .expect("recycled");
    // Something new took the name in the meantime.
    std::fs::write(&x, b"new").unwrap();
    let res = restore(&tree, token, &silent(), &undo).unwrap();
    assert!(res.ok.is_empty());
    assert_eq!(
        res.failed,
        vec![Failure {
            id,
            reason: reasons::NAME_TAKEN.into()
        }]
    );
    assert_eq!(std::fs::read(&x).unwrap(), b"new");
    assert!(undo.contains(token), "kept for another try");
    assert!(tree.read().unwrap().is_deleted(id));

    // Once the name is free again, the same token works.
    std::fs::remove_file(&x).unwrap();
    let res = restore(&tree, token, &silent(), &undo).unwrap();
    assert_eq!(res.ok, vec![id], "{res:?}");
    assert_eq!(std::fs::read(&x).unwrap(), contents("x.txt", 100));
    assert!(!tree.read().unwrap().is_deleted(id));
}

#[test]
fn nested_and_missing_paths() {
    if !shell_works() {
        return;
    }
    let dir = TempDir::new("nested");
    let f = dir.file("d/inner/f.txt", 10);
    let d = dir.0.join("d");
    let missing = dir.0.join("never-existed");
    // The nested file is left to its folder's deletion; a missing path is
    // simply gone. Neither may make the operation fail.
    let gone = delete_paths(&[f, d.clone(), missing], DeleteMode::Permanent, &silent()).unwrap();
    assert_eq!(gone, vec![true, true, true]);
    assert!(!d.exists());
    let none = delete_paths(&[], DeleteMode::Permanent, &silent()).unwrap();
    assert_eq!(none, Vec::<bool>::new());
}

#[test]
fn works_from_com_initialised_threads() {
    let _bin = bin_lock();
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
    };
    if !shell_works() {
        return;
    }
    for coinit in [COINIT_MULTITHREADED, COINIT_APARTMENTTHREADED] {
        std::thread::spawn(move || {
            unsafe { CoInitializeEx(None, coinit).ok().unwrap() };
            let dir = TempDir::new("apartment");
            dir.file("f.txt", 10);
            dir.file("g.txt", 10);
            let tree = tree_of(&dir.0);
            let (f, g) = (find(&tree, "f.txt"), find(&tree, "g.txt"));
            let undo = UndoStore::new();
            let res = recycle(&tree, &[f], &silent(), &undo).unwrap();
            assert_eq!(res.ok, vec![f], "{coinit:?}");
            let back = restore(&tree, res.undo.unwrap(), &silent(), &undo).unwrap();
            assert_eq!(back.ok, vec![f], "{coinit:?}");
            assert_eq!(purge(&tree, &[g], &silent()).unwrap().ok, vec![g]);
            // The thread's own COM initialisation is still intact.
            unsafe { CoUninitialize() };
        })
        .join()
        .unwrap();
    }
}

#[test]
fn tree_lock_is_free_while_the_shell_works() {
    if !shell_works() {
        return;
    }
    let dir = TempDir::new("lock");
    dir.file("x/y.txt", 100);
    let tree = tree_of(&dir.0);
    let x = find(&tree, "x");
    // Hold a read lock while a background thread deletes: the folder must
    // still go, so the shell cannot be waiting for the lock.
    let guard = tree.read().unwrap();
    let worker = {
        let tree = tree.clone();
        std::thread::spawn(move || purge(&tree, &[x], &silent()))
    };
    // Watch for the delete by listing the scan root, never by opening a
    // path inside `x`: if the shell deletes `x/y.txt` while a check holds it
    // open (`Path::exists` opens a handle), `x` fails to go and `purge`
    // reports nothing deleted.
    let deadline = Instant::now() + Duration::from_secs(60);
    while std::fs::read_dir(&dir.0).unwrap().next().is_some() {
        if Instant::now() >= deadline {
            drop(guard);
            panic!("the delete never happened: {:?}", worker.join().unwrap());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!guard.is_deleted(x), "the tree waits for the write lock");
    drop(guard);
    let res = worker.join().unwrap().unwrap();
    assert_eq!(res.ok, vec![x]);
    assert!(tree.read().unwrap().is_deleted(x));
    assert_eq!(tree.read().unwrap().node(NodeId::ROOT).logical, 0);
}

/// Empties the whole recycle bin of the system drive, so it only runs on
/// GitHub Actions.
#[test]
fn empty_bin_on_ci_only() {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        println!("note: skipped, empties the recycle bin; runs on GitHub Actions only");
        return;
    }
    let _bin = bin_lock();
    if !shell_works() {
        return;
    }
    let dir = TempDir::new("empty");
    dir.file("gone.bin", 5000);
    let tree = tree_of(&dir.0);
    let id = find(&tree, "gone.bin");
    let undo = UndoStore::new();
    let token = recycle(&tree, &[id], &silent(), &undo)
        .unwrap()
        .undo
        .expect("recycled");
    let drive = PathBuf::from(format!("{}\\", std::env::var("SystemDrive").unwrap()));
    let res = empty_recycle_bin(&tree, &drive, &silent()).unwrap();
    assert!(res.bytes >= 5000, "{res:?}");
    // The tree is the temp folder, not the drive: nothing in it changes.
    assert!(res.ok.is_empty());
    // The recycled item is gone for good, so the undo fails, item by item.
    let back = restore(&tree, token, &silent(), &undo).unwrap();
    assert!(back.ok.is_empty());
    assert_eq!(back.failed.len(), 1, "{back:?}");
    assert!(!dir.0.join("gone.bin").exists());
}
