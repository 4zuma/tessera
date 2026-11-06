//! The commands and events of docs/ipc.md, backed by `tessera-core`.
//!
//! One scan session at a time: its tree, progress counters and watcher live
//! here, in Rust. The WebView only ever receives layout buffers (the rects on
//! screen) and small records about single nodes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, ipc::Response};
use tessera_core::layout::{self, LayoutEngine, LayoutRequest, LayoutStyle, RootExtras, SizeMode};
use tessera_core::scan::{self, ScanOptions, ScanProgress};
use tessera_core::watch::{BatchReport, WatchOptions, Watcher};
use tessera_core::{NodeFlags, NodeId, SharedTree, Tree, category, delete, tree::mtime_to_unix_ms};

use crate::volumes;

/// How often `scan-progress` is sent while scanning.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// State

/// Managed by Tauri: the current scan session and what outlives it.
pub struct Core {
    session: RwLock<Option<Arc<Session>>>,
    engine: Mutex<LayoutEngine>,
    undo: Arc<delete::UndoStore>,
    next_scan: AtomicU32,
    /// The main window, owner of the shell's progress and error dialogs.
    pub owner_hwnd: AtomicIsize,
}

impl Core {
    pub fn new() -> Self {
        Core {
            session: RwLock::new(None),
            engine: Mutex::new(LayoutEngine::new()),
            undo: Arc::new(delete::UndoStore::new()),
            next_scan: AtomicU32::new(0),
            owner_hwnd: AtomicIsize::new(0),
        }
    }

    fn current(&self) -> Result<Arc<Session>, String> {
        self.session
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| "还没有开始扫描".to_string())
    }

    fn is_current(&self, id: u32) -> bool {
        self.session
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|s| s.id == id)
    }

    fn delete_opts(&self) -> delete::DeleteOptions {
        delete::DeleteOptions {
            owner_hwnd: self.owner_hwnd.load(Relaxed),
            silent: false,
        }
    }
}

struct Session {
    id: u32,
    root: PathBuf,
    /// The scanned path as given, e.g. `C:\`.
    root_text: String,
    /// Volume label, for the root's name in the details card.
    label: String,
    /// The root is a volume root, so FREE and UNSCANNED tiles apply.
    is_volume: bool,
    tree: SharedTree,
    progress: ScanProgress,
    capacity: u64,
    used: AtomicU64,
    free: AtomicU64,
    started: Instant,
    elapsed_ms: AtomicU64,
    done: AtomicBool,
    /// Tree revision: bumped by scan ticks, watcher batches and deletes.
    rev: AtomicU32,
    /// Serialises progress events, so `done: true` is always the last one.
    emit_lock: Mutex<()>,
    watcher: Mutex<Option<Watcher>>,
    /// Root size when `tree-changed` was last sent.
    last_root: AtomicU64,
    /// Recursive file and folder counts per folder, with the rev they are for.
    counts: Mutex<HashMap<u32, (u32, u64, u64)>>,
}

impl Session {
    fn new(id: u32, root: &str, tree: Tree, vol: Option<&volumes::Volume>) -> Self {
        Session {
            id,
            root: PathBuf::from(root),
            root_text: root.to_string(),
            label: vol.map(|v| v.label.clone()).unwrap_or_default(),
            is_volume: vol.is_some(),
            tree: tree.into_shared(),
            progress: ScanProgress::new(),
            capacity: vol.map_or(0, |v| v.capacity),
            used: AtomicU64::new(vol.map_or(0, |v| v.capacity.saturating_sub(v.free))),
            free: AtomicU64::new(vol.map_or(0, |v| v.free)),
            started: Instant::now(),
            elapsed_ms: AtomicU64::new(0),
            done: AtomicBool::new(false),
            rev: AtomicU32::new(1),
            emit_lock: Mutex::new(()),
            watcher: Mutex::new(None),
            last_root: AtomicU64::new(0),
            counts: Mutex::new(HashMap::new()),
        }
    }

    fn tree(&self) -> RwLockReadGuard<'_, Tree> {
        self.tree.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn root_size(&self) -> u64 {
        self.tree().node(NodeId::ROOT).size
    }

    fn refresh_volume(&self) {
        if let Ok(v) = scan::volume_info(&self.root) {
            self.free.store(v.free_bytes, Relaxed);
            self.used
                .store(v.total_bytes.saturating_sub(v.free_bytes), Relaxed);
        }
    }

    fn elapsed_ms(&self) -> u64 {
        if self.done.load(Relaxed) {
            self.elapsed_ms.load(Relaxed)
        } else {
            self.started.elapsed().as_millis() as u64
        }
    }

    fn progress_event(&self, done: bool) -> ProgressEvent {
        let p = self.progress.snapshot();
        ProgressEvent {
            scan_id: self.id,
            files: p.files,
            dirs: p.dirs,
            scanned: if done { self.root_size() } else { p.bytes },
            used: self.used.load(Relaxed),
            elapsed_ms: self.elapsed_ms(),
            done,
            rev: self.rev.load(Relaxed),
            errors: p.inaccessible,
        }
    }

    /// Takes the watcher out (so no lock is held while it stops) and stops it.
    fn stop_watcher(&self) {
        let w = lock(&self.watcher).take();
        if let Some(w) = w {
            w.stop();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Wire types (camelCase, as in docs/ipc.md)

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    scan_id: u32,
    files: u64,
    dirs: u64,
    scanned: u64,
    used: u64,
    elapsed_ms: u64,
    done: bool,
    rev: u32,
    errors: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TreeChangedEvent {
    rev: u32,
    bytes_delta: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStarted {
    scan_id: u32,
    root_id: u32,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Disk,
    Logical,
}

impl From<Mode> for SizeMode {
    fn from(m: Mode) -> Self {
        match m {
            Mode::Disk => SizeMode::Disk,
            Mode::Logical => SizeMode::Logical,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutReq {
    view: u32,
    width: f32,
    height: f32,
    size_mode: Mode,
    style: StyleReq,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StyleReq {
    outer: f32,
    pad: f32,
    gap: f32,
    header: f32,
    min_header_w: f32,
    min_area: f32,
    recurse: f32,
    open_share: f32,
    max_depth: u8,
    max_per_dir: u32,
    label_min_w: f32,
    label_min_h: f32,
}

impl From<&LayoutReq> for LayoutRequest {
    fn from(r: &LayoutReq) -> Self {
        let s = &r.style;
        LayoutRequest {
            view: NodeId(r.view),
            width: r.width,
            height: r.height,
            size_mode: r.size_mode.into(),
            style: LayoutStyle {
                outer: s.outer,
                pad: s.pad,
                gap: s.gap,
                header: s.header,
                min_header_w: s.min_header_w,
                min_area: s.min_area,
                recurse: s.recurse,
                open_share: s.open_share,
                max_depth: s.max_depth,
                max_per_dir: s.max_per_dir,
                label_min_w: s.label_min_w,
                label_min_h: s.label_min_h,
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Details {
    id: u32,
    kind: &'static str,
    name: String,
    path: String,
    size: u64,
    disk: u64,
    logical: u64,
    share: f64,
    files: u64,
    dirs: u64,
    modified: Option<f64>,
    cat: u8,
    flags: u8,
    lock_reason: Option<String>,
    hint: Option<String>,
}

#[derive(Serialize)]
pub struct Crumb {
    id: u32,
    name: String,
}

#[derive(Serialize)]
pub struct NavInfo {
    crumbs: Vec<Crumb>,
    up: Option<u32>,
}

#[derive(Serialize)]
pub struct CatStat {
    cat: u8,
    bytes: u64,
}

#[derive(Serialize)]
pub struct Failure {
    id: u32,
    reason: String,
}

#[derive(Serialize)]
pub struct DeleteResult {
    ok: Vec<u32>,
    failed: Vec<Failure>,
    bytes: u64,
    undo: Option<u32>,
    rev: u32,
}

// ---------------------------------------------------------------------------
// Scanning

#[tauri::command]
pub async fn list_volumes() -> Vec<volumes::Volume> {
    volumes::list()
}

#[tauri::command]
pub async fn scan_start(
    app: AppHandle,
    core: State<'_, Core>,
    root: String,
) -> Result<ScanStarted, String> {
    let path = PathBuf::from(&root);
    if !path.is_dir() {
        return Err(format!("找不到 {root}"));
    }
    let vol = volumes::describe(&root);
    let id = core.next_scan.fetch_add(1, Relaxed) + 1;
    let s = Arc::new(Session::new(id, &root, Tree::for_path(&path), vol.as_ref()));

    // Replace the old session; it stops in the background.
    let old = core
        .session
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .replace(s.clone());
    lock(&core.engine).reset();
    core.undo.clear();
    if let Some(old) = old {
        old.progress.cancel();
        std::thread::spawn(move || old.stop_watcher());
    }

    let (s2, app2) = (s.clone(), app.clone());
    std::thread::Builder::new()
        .name("tessera-scan".into())
        .spawn(move || run_scan(app2, s2))
        .map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("tessera-progress".into())
        .spawn(move || run_progress(app, s))
        .map_err(|e| e.to_string())?;
    Ok(ScanStarted {
        scan_id: id,
        root_id: NodeId::ROOT.0,
    })
}

#[tauri::command]
pub fn scan_cancel(core: State<'_, Core>) {
    if let Ok(s) = core.current() {
        s.progress.cancel();
    }
}

fn run_progress(app: AppHandle, s: Arc<Session>) {
    let mut last = (u64::MAX, u64::MAX);
    loop {
        std::thread::sleep(PROGRESS_EVERY);
        let _g = lock(&s.emit_lock);
        if s.done.load(Relaxed) {
            return;
        }
        let p = s.progress.snapshot();
        if (p.bytes, p.files) != last {
            last = (p.bytes, p.files);
            s.rev.fetch_add(1, Relaxed);
        }
        let _ = app.emit("scan-progress", s.progress_event(false));
    }
}

fn run_scan(app: AppHandle, s: Arc<Session>) {
    let res = scan::scan(&s.root, &ScanOptions::default(), &s.tree, &s.progress);
    {
        let _g = lock(&s.emit_lock);
        s.elapsed_ms
            .store(s.started.elapsed().as_millis() as u64, Relaxed);
        s.done.store(true, Relaxed);
        s.rev.fetch_add(1, Relaxed);
        s.refresh_volume();
        s.last_root.store(s.root_size(), Relaxed);
        let _ = app.emit("scan-progress", s.progress_event(true));
    }
    let summary = match res {
        Ok(summary) => summary,
        Err(tessera_core::Error::Cancelled) => return,
        Err(e) => {
            eprintln!("scan of {} failed: {e}", s.root_text);
            return;
        }
    };
    let core = app.state::<Core>();
    if !core.is_current(s.id) {
        return;
    }
    let usn = summary.frn_index.zip(summary.usn_checkpoint);
    let weak = Arc::downgrade(&s);
    let app2 = app.clone();
    match Watcher::start(
        &s.root,
        s.tree.clone(),
        WatchOptions::default(),
        usn,
        move |rep| on_batch(&app2, &weak, rep),
    ) {
        Ok(w) => {
            *lock(&s.watcher) = Some(w);
            // a new scan may have started meanwhile
            if !core.is_current(s.id) {
                s.stop_watcher();
            }
        }
        Err(e) => eprintln!("live updates for {} are off: {e}", s.root_text),
    }
}

/// Runs on the watcher's thread after each batch that changed the tree.
fn on_batch(app: &AppHandle, s: &Weak<Session>, rep: &BatchReport) {
    let Some(s) = s.upgrade() else { return };
    // a batch can still be in flight while a new scan replaces this session;
    // its delta belongs to the old drive
    if !app.state::<Core>().is_current(s.id) {
        return;
    }
    let rev = s.rev.fetch_add(1, Relaxed) + 1;
    let root = s.root_size();
    let before = s.last_root.swap(root, Relaxed);
    s.refresh_volume();
    if !rep.rescan_needed.is_empty() {
        // never block here: stopping the watcher waits for this thread
        if let Ok(w) = s.watcher.try_lock()
            && let Some(w) = w.as_ref()
        {
            for &d in &rep.rescan_needed {
                w.rescan(d);
            }
        }
    }
    let _ = app.emit(
        "tree-changed",
        TreeChangedEvent {
            rev,
            bytes_delta: root as i64 - before as i64,
        },
    );
}

// ---------------------------------------------------------------------------
// Layout and node records

#[tauri::command]
pub async fn layout(core: State<'_, Core>, req: LayoutReq) -> Result<Response, String> {
    let s = core.current()?;
    let mut req = LayoutRequest::from(&req);
    let t = s.tree();
    // the layout echoes the view it drew, so the UI follows when it falls back
    req.view = live_view(&t, req.view);
    let extras = if s.is_volume && req.view == NodeId::ROOT {
        RootExtras {
            free: Some(s.free.load(Relaxed)),
            unscanned: (!s.done.load(Relaxed)).then(|| {
                s.used
                    .load(Relaxed)
                    .saturating_sub(t.node(NodeId::ROOT).size)
            }),
        }
    } else {
        RootExtras::default()
    };
    let rev = s.rev.load(Relaxed);
    let buf = lock(&core.engine).layout(&t, &req, extras, rev);
    Ok(Response::new(buf))
}

/// The folder to show for a requested view: itself, or its nearest live
/// folder ancestor when it was deleted meanwhile (e.g. in Explorer) or is
/// not a folder.
fn live_view(t: &Tree, v: NodeId) -> NodeId {
    if v.index() >= t.len() {
        return NodeId::ROOT;
    }
    std::iter::once(v)
        .chain(t.ancestors(v))
        .find(|&n| !t.is_deleted(n) && t.node(n).is_dir())
        .unwrap_or(NodeId::ROOT)
}

#[tauri::command]
pub async fn details(
    core: State<'_, Core>,
    id: u32,
    view: u32,
    size_mode: Mode,
) -> Result<Option<Details>, String> {
    let _ = view;
    let s = core.current()?;
    Ok(node_details(&s, id, size_mode.into()))
}

fn node_details(s: &Session, id: u32, mode: SizeMode) -> Option<Details> {
    let blank =
        |id: u32, kind: &'static str, name: &str, path: String, size: u64, hint: &str| Details {
            id,
            kind,
            name: name.into(),
            path,
            size,
            disk: 0,
            logical: 0,
            share: if s.capacity > 0 {
                size as f64 / s.capacity as f64
            } else {
                0.0
            },
            files: 0,
            dirs: 0,
            modified: None,
            cat: category::NONE,
            flags: 0,
            lock_reason: None,
            hint: Some(hint.into()),
        };
    if id == layout::ids::FREE {
        return Some(blank(
            id,
            "free",
            "可用空间",
            s.root_text.clone(),
            s.free.load(Relaxed),
            "卷上还没被使用的空间",
        ));
    }
    if id == layout::ids::UNSCANNED {
        let u = s.used.load(Relaxed).saturating_sub(s.root_size());
        return Some(blank(
            id,
            "unscanned",
            "尚未扫描",
            s.root_text.clone(),
            u,
            "已用空间里还没扫描到的部分，会随扫描缩小",
        ));
    }
    let t = s.tree();
    let valid = |n: NodeId| n.index() < t.len() && !t.is_deleted(n);
    if id != layout::ids::NONE && id & layout::ids::GROUP_BIT != 0 {
        let folder = NodeId(id & !layout::ids::GROUP_BIT);
        if !valid(folder) {
            return None;
        }
        let mut d = blank(
            id,
            "group",
            "小项合并显示",
            t.path_lossy(folder),
            0,
            "太小、画不下的项目合并成一块；放大上级文件夹可以看到它们",
        );
        d.share = 0.0;
        return Some(d);
    }
    let nid = NodeId(id);
    if !valid(nid) {
        return None;
    }
    let n = *t.node(nid);
    let path = t.path(nid);
    let size = mode.of(&n);
    let share = if nid == NodeId::ROOT {
        if s.capacity > 0 {
            size as f64 / s.capacity as f64
        } else {
            1.0
        }
    } else {
        size as f64 / mode.of(t.node(n.parent)).max(1) as f64
    };
    let (files, dirs) = if n.is_dir() {
        counts(s, &t, nid)
    } else {
        (0, 0)
    };
    let bin = s
        .is_volume
        .then(|| t.find_child(NodeId::ROOT, b"$Recycle.Bin"))
        .flatten();
    let mut flags = 0u8;
    let lock_reason = if bin == Some(nid) {
        flags |= layout::flags::RECYCLE_BIN;
        Some("请使用“清空回收站”".to_string())
    } else {
        delete::lock_reason(&path).map(zh)
    };
    if lock_reason.is_some() && bin != Some(nid) {
        flags |= layout::flags::PROTECTED;
    }
    for (nf, lf) in [
        (NodeFlags::INACCESSIBLE, layout::flags::NO_ACCESS),
        (NodeFlags::CLOUD, layout::flags::CLOUD),
        (NodeFlags::HARDLINK, layout::flags::HARDLINK),
        (NodeFlags::REPARSE, layout::flags::REPARSE),
    ] {
        if n.flags.contains(nf) {
            flags |= lf;
        }
    }
    let name = if nid == NodeId::ROOT && s.is_volume {
        format!("{} ({})", s.label, drive_name(&s.root_text))
    } else {
        t.name_lossy(nid)
    };
    Some(Details {
        id,
        kind: if n.is_dir() { "dir" } else { "file" },
        hint: hint_for(&name, &n),
        name,
        path: path.to_string_lossy().into_owned(),
        size,
        disk: n.size,
        logical: n.logical,
        share,
        files,
        dirs,
        modified: mtime_to_unix_ms(n.mtime).map(|ms| ms as f64),
        cat: category::of_node(&t, nid),
        flags,
        lock_reason,
    })
}

/// Recursive file and folder counts, cached per tree revision. While the
/// root is still being scanned the scanner's own counters are used.
fn counts(s: &Session, t: &Tree, id: NodeId) -> (u64, u64) {
    if id == NodeId::ROOT && !s.done.load(Relaxed) {
        let p = s.progress.snapshot();
        return (p.files, p.dirs);
    }
    let rev = s.rev.load(Relaxed);
    if let Some(&(r, f, d)) = lock(&s.counts).get(&id.0)
        && r == rev
    {
        return (f, d);
    }
    let (mut files, mut dirs) = (0u64, 0u64);
    let mut stack = vec![id];
    while let Some(d) = stack.pop() {
        for c in t.children(d) {
            if t.node(c).is_dir() {
                dirs += 1;
                stack.push(c);
            } else {
                files += 1;
            }
        }
    }
    let mut cache = lock(&s.counts);
    if cache.len() > 256 {
        cache.clear();
    }
    cache.insert(id.0, (rev, files, dirs));
    (files, dirs)
}

fn hint_for(name: &str, n: &tessera_core::Node) -> Option<String> {
    let h = match name.to_ascii_lowercase().as_str() {
        "hiberfil.sys" => "可以在管理员终端运行 powercfg /h off 关闭休眠来释放它",
        "pagefile.sys" | "swapfile.sys" => "在“高级系统设置 › 性能 › 虚拟内存”里调整",
        "windows.old" => "旧版 Windows 的备份，用系统的“磁盘清理”删除最稳妥",
        _ if n.flags.contains(NodeFlags::CLOUD) => "云端文件：只算本机实际占用的部分",
        _ if n.flags.contains(NodeFlags::INACCESSIBLE) => {
            "没有权限读取，以管理员身份运行可以看到里面"
        }
        _ => return None,
    };
    Some(h.into())
}

/// `C:\` -> `C:`
fn drive_name(root: &str) -> &str {
    root.trim_end_matches(['\\', '/'])
}

#[tauri::command]
pub async fn nav(core: State<'_, Core>, id: u32) -> Result<NavInfo, String> {
    let s = core.current()?;
    Ok(nav_of(&s, id))
}

fn nav_of(s: &Session, id: u32) -> NavInfo {
    let t = s.tree();
    let n = layout::nav(&t, NodeId(id), SizeMode::Disk);
    NavInfo {
        crumbs: n
            .crumbs
            .into_iter()
            .map(|(id, name)| Crumb {
                id: id.0,
                name: if id == NodeId::ROOT {
                    drive_name(&s.root_text).to_string()
                } else {
                    name
                },
            })
            .collect(),
        up: n.up.map(|u| u.0),
    }
}

#[tauri::command]
pub async fn cat_stats(
    core: State<'_, Core>,
    view: u32,
    size_mode: Mode,
) -> Result<Vec<CatStat>, String> {
    let s = core.current()?;
    Ok(cat_stats_of(&s.tree(), NodeId(view), size_mode.into()))
}

/// Bytes per file category below `view`, largest first.
fn cat_stats_of(t: &Tree, view: NodeId, mode: SizeMode) -> Vec<CatStat> {
    if view.index() >= t.len() || t.is_deleted(view) {
        return Vec::new();
    }
    let mut bytes = [0u64; category::COUNT];
    let mut stack = vec![view];
    while let Some(d) = stack.pop() {
        for c in t.children(d) {
            let n = t.node(c);
            if n.is_dir() {
                stack.push(c);
            } else if let Some(b) = bytes.get_mut(category::of_node(t, c) as usize) {
                *b += mode.of(n);
            }
        }
    }
    let mut out: Vec<CatStat> = bytes
        .iter()
        .enumerate()
        .filter(|&(_, &b)| b > 0)
        .map(|(c, &b)| CatStat {
            cat: c as u8,
            bytes: b,
        })
        .collect();
    out.sort_by_key(|c| std::cmp::Reverse(c.bytes));
    out
}

// ---------------------------------------------------------------------------
// Deleting

/// Runs a blocking shell operation off the async runtime, then bumps the
/// revision and re-reads the volume's free space.
async fn shell_op(
    s: Arc<Session>,
    op: impl FnOnce(&Session) -> tessera_core::Result<delete::DeleteResult> + Send + 'static,
) -> Result<DeleteResult, String> {
    let (s, res) = tauri::async_runtime::spawn_blocking(move || {
        let r = op(&s);
        (s, r)
    })
    .await
    .map_err(|e| e.to_string())?;
    let r = res.map_err(|e| error_text(&e))?;
    let rev = s.rev.fetch_add(1, Relaxed) + 1;
    s.refresh_volume();
    s.last_root.store(s.root_size(), Relaxed);
    Ok(DeleteResult {
        ok: r.ok.iter().map(|n| n.0).collect(),
        failed: r
            .failed
            .into_iter()
            .map(|f| Failure {
                id: f.id.0,
                reason: zh(&f.reason),
            })
            .collect(),
        bytes: r.bytes,
        undo: r.undo,
        rev,
    })
}

fn node_ids(ids: &[u32]) -> Vec<NodeId> {
    ids.iter().map(|&i| NodeId(i)).collect()
}

#[tauri::command]
pub async fn recycle(core: State<'_, Core>, ids: Vec<u32>) -> Result<DeleteResult, String> {
    let (s, opts, undo) = (core.current()?, core.delete_opts(), core.undo.clone());
    shell_op(s, move |s| {
        delete::recycle(&s.tree, &node_ids(&ids), &opts, &undo)
    })
    .await
}

#[tauri::command]
pub async fn restore(core: State<'_, Core>, token: u32) -> Result<DeleteResult, String> {
    let (s, opts, undo) = (core.current()?, core.delete_opts(), core.undo.clone());
    shell_op(s, move |s| delete::restore(&s.tree, token, &opts, &undo)).await
}

#[tauri::command]
pub async fn purge(core: State<'_, Core>, ids: Vec<u32>) -> Result<DeleteResult, String> {
    let (s, opts) = (core.current()?, core.delete_opts());
    shell_op(s, move |s| delete::purge(&s.tree, &node_ids(&ids), &opts)).await
}

#[tauri::command]
pub async fn empty_recycle_bin(
    core: State<'_, Core>,
    root: String,
) -> Result<DeleteResult, String> {
    let (s, opts) = (core.current()?, core.delete_opts());
    shell_op(s, move |s| {
        delete::empty_recycle_bin(&s.tree, Path::new(&root), &opts)
    })
    .await
}

#[tauri::command]
pub async fn reveal(core: State<'_, Core>, id: u32) -> Result<(), String> {
    let s = core.current()?;
    let path = {
        let t = s.tree();
        if id as usize >= t.len() {
            return Err("项目不存在".into());
        }
        t.path(NodeId(id))
    };
    open_in_explorer(&path)
}

#[cfg(windows)]
fn open_in_explorer(path: &Path) -> Result<(), String> {
    use std::ffi::OsString;
    use std::os::windows::process::CommandExt;
    // explorer parses its own command line: /select,"<path>" must reach it unquoted as a whole
    let mut arg = OsString::from("/select,\"");
    arg.push(path.as_os_str());
    arg.push("\"");
    std::process::Command::new("explorer.exe")
        .raw_arg(arg)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn open_in_explorer(_path: &Path) -> Result<(), String> {
    Err("只在 Windows 上可用".into())
}

// ---------------------------------------------------------------------------
// Text

/// The core's user-facing reasons are English; the UI speaks Chinese.
/// Anything else (e.g. a message from the shell) is passed through.
fn zh(reason: &str) -> String {
    use delete::reasons::*;
    let s = match reason {
        DRIVE_ROOT => "这是整个磁盘的根目录。",
        SCAN_ROOT => "这是正在扫描的根文件夹。",
        WINDOWS => "Windows 需要这个文件夹。",
        PROGRAMS => "已安装的程序放在这个文件夹里。",
        APP_DATA => "Windows 和各个应用的数据存放在这里。",
        PROFILES => "这里存放着所有用户的个人文件夹。",
        PROFILE => "这是一个用户的个人文件夹。",
        // shown next to a lock: say why the folder stays, not that something may be deleted
        PERSONAL => "这是你的个人文件夹，Windows 和应用要用到它。",
        TEMP => "Windows 和应用把临时文件放在这个文件夹里。",
        CONTAINS => "这个文件夹里有 Windows 或应用需要的文件夹。",
        STILL_THERE => "没能删除这个项目。",
        NOT_RESTORED => "没能还原这个项目。",
        NAME_TAKEN => "原位置已经有同名的项目。",
        CANCELLED => "已取消。",
        SKIPPED => "已跳过。",
        other => return other.to_string(),
    };
    s.to_string()
}

fn error_text(e: &tessera_core::Error) -> String {
    match e {
        tessera_core::Error::Cancelled => "已取消。".into(),
        tessera_core::Error::Unsupported("unknown undo token") => "这次操作已经不能撤销了。".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_are_translated() {
        assert_eq!(zh(delete::reasons::WINDOWS), "Windows 需要这个文件夹。");
        assert_eq!(zh("Access is denied."), "Access is denied.");
    }

    #[test]
    fn layout_request_parses_the_contract() {
        let r: LayoutReq = serde_json::from_str(
            r#"{"view":0,"width":800,"height":600,"sizeMode":"logical","style":{"outer":6,"pad":3,"gap":2,"header":21,"minHeaderW":60,"minArea":110,"recurse":60,"openShare":0.0025,"maxDepth":14,"maxPerDir":400,"labelMinW":44,"labelMinH":20}}"#,
        )
        .unwrap();
        let q = LayoutRequest::from(&r);
        assert_eq!(q.size_mode, SizeMode::Logical);
        assert_eq!(q.style, LayoutStyle::default());
    }

    /// C:\ with Users\me\{a.mp4, b.ts (TypeScript), Docs\c.pdf} and pagefile.sys
    fn sample() -> Session {
        let mut t = Tree::new(b"C:\\");
        let users = t.add_child(NodeId::ROOT, b"Users", 0, 0, NodeFlags::DIR, 0);
        let me = t.add_child(users, b"me", 0, 0, NodeFlags::DIR, 0);
        t.add_child(me, b"a.mp4", 4096 * 100, 400_000, NodeFlags::NONE, 0);
        t.add_child(me, b"b.ts", 4096, 900, NodeFlags::NONE, 0);
        let docs = t.add_child(me, b"Docs", 0, 0, NodeFlags::DIR, 0);
        t.add_child(docs, b"c.pdf", 8192, 8000, NodeFlags::NONE, 0);
        t.add_child(
            NodeId::ROOT,
            b"pagefile.sys",
            4096 * 1000,
            4096 * 1000,
            NodeFlags::NONE,
            0,
        );
        let vol = volumes::Volume {
            root: "C:\\".into(),
            label: "Windows".into(),
            fs: "NTFS".into(),
            capacity: 100 << 20,
            free: 40 << 20,
        };
        Session::new(1, "C:\\", t, Some(&vol))
    }

    fn id_of(s: &Session, path: &[&[u8]]) -> NodeId {
        let t = s.tree();
        path.iter()
            .fold(NodeId::ROOT, |at, name| t.find_child(at, name).unwrap())
    }

    #[test]
    fn details_of_nodes_and_pseudo_tiles() {
        let s = sample();
        let me = id_of(&s, &[b"Users", b"me"]);
        let d = node_details(&s, me.0, SizeMode::Disk).unwrap();
        assert_eq!((d.kind, d.files, d.dirs), ("dir", 3, 1));
        assert_eq!(d.size, 4096 * 100 + 4096 + 8192);
        assert_eq!(d.cat, category::NONE);
        assert!((d.share - 1.0).abs() < 1e-9, "me is all of Users");

        let ts = id_of(&s, &[b"Users", b"me", b"b.ts"]);
        let d = node_details(&s, ts.0, SizeMode::Logical).unwrap();
        assert_eq!((d.kind, d.size, d.cat), ("file", 900, category::DEV));

        let pf = id_of(&s, &[b"pagefile.sys"]);
        assert!(
            node_details(&s, pf.0, SizeMode::Disk)
                .unwrap()
                .hint
                .is_some()
        );

        let root = node_details(&s, 0, SizeMode::Disk).unwrap();
        assert_eq!(root.name, "Windows (C:)");

        let free = node_details(&s, layout::ids::FREE, SizeMode::Disk).unwrap();
        assert_eq!((free.kind, free.size), ("free", 40 << 20));
        let un = node_details(&s, layout::ids::UNSCANNED, SizeMode::Disk).unwrap();
        assert_eq!(un.size, (60 << 20) - s.root_size());
        let g = node_details(&s, layout::ids::GROUP_BIT | me.0, SizeMode::Disk).unwrap();
        assert_eq!(g.kind, "group");

        assert!(node_details(&s, 9999, SizeMode::Disk).is_none());
    }

    #[test]
    fn categories_below_a_view() {
        let s = sample();
        let stats = cat_stats_of(&s.tree(), NodeId::ROOT, SizeMode::Disk);
        let cats: Vec<u8> = stats.iter().map(|c| c.cat).collect();
        assert_eq!(
            cats,
            [
                category::SYSTEM,
                category::VIDEO,
                category::DOC,
                category::DEV
            ]
        );
        let me = id_of(&s, &[b"Users", b"me"]);
        assert_eq!(cat_stats_of(&s.tree(), me, SizeMode::Disk).len(), 3);
    }

    #[test]
    fn crumbs_name_the_drive() {
        let s = sample();
        let docs = id_of(&s, &[b"Users", b"me", b"Docs"]);
        let n = nav_of(&s, docs.0);
        let names: Vec<&str> = n.crumbs.iter().map(|c| c.name.as_str()).collect();
        // Users holds only "me", so it is folded away (rule 4)
        assert_eq!(names, ["C:", "me", "Docs"]);
        assert_eq!(n.up, Some(id_of(&s, &[b"Users", b"me"]).0));
    }

    #[test]
    fn deleted_view_falls_back_to_a_live_folder() {
        let s = sample();
        let me = id_of(&s, &[b"Users", b"me"]);
        let docs = id_of(&s, &[b"Users", b"me", b"Docs"]);
        let a = id_of(&s, &[b"Users", b"me", b"a.mp4"]);
        let mut t = s.tree.write().unwrap();
        assert_eq!(live_view(&t, docs), docs);
        assert_eq!(live_view(&t, a), me, "a file is not a view");
        assert!(t.mark_deleted(docs));
        assert_eq!(live_view(&t, docs), me);
        assert!(t.mark_deleted(me));
        assert_eq!(live_view(&t, docs), id_of_root_child(&t, b"Users"));
        assert_eq!(live_view(&t, NodeId(9999)), NodeId::ROOT);
    }

    fn id_of_root_child(t: &Tree, name: &[u8]) -> NodeId {
        t.find_child(NodeId::ROOT, name).unwrap()
    }

    #[test]
    fn drive_names() {
        assert_eq!(drive_name("C:\\"), "C:");
        assert_eq!(drive_name("D:\\Games"), "D:\\Games");
    }
}
