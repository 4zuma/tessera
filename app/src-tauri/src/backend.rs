//! The commands and events of docs/ipc.md, backed by `tessera-core`.
//!
//! One scan session at a time: its tree, progress counters and watcher live
//! here, in Rust. The WebView only ever receives layout buffers (the rects on
//! screen) and small records about single nodes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, ipc::Response};
use tessera_core::layout::{self, LayoutEngine, LayoutRequest, LayoutStyle, RootExtras, SizeMode};
use tessera_core::scan::{self, ScanOptions, ScanProgress};
use tessera_core::totals::{Totals, TotalsCache};
use tessera_core::watch::{BatchReport, WatchOptions, Watcher};
use tessera_core::{NodeFlags, NodeId, SharedTree, Tree, category, delete};

use crate::i18n::{self, text};
use crate::{shell_menu, volumes};

/// How often `scan-progress` is sent while scanning.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// The watcher's batch window while the window is focused, in the
/// background, and minimized. A longer window wakes the watcher less often
/// on a busy disk and reads a file that keeps changing once per window.
const PACE_ACTIVE: Duration = Duration::from_millis(500);
const PACE_BACKGROUND: Duration = Duration::from_secs(2);
const PACE_MINIMIZED: Duration = Duration::from_secs(10);

/// Live changes adding up to less than this many px² of the shown view
/// cannot move anything visibly, so they are not even laid out.
const REDRAW_PX2: f64 = 1.0;
/// A laid-out change is only sent to the window when some edge moves by
/// more than this (CSS px) or a label, colour or tile changes.
const REDRAW_TOLERANCE_PX: f32 = 0.5;

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
    /// The main window has the focus / is minimized (see [`window_state`]).
    focused: AtomicBool,
    minimized: AtomicBool,
}

impl Core {
    pub fn new() -> Self {
        Core {
            session: RwLock::new(None),
            engine: Mutex::new(LayoutEngine::new()),
            undo: Arc::new(delete::UndoStore::new()),
            next_scan: AtomicU32::new(0),
            owner_hwnd: AtomicIsize::new(0),
            focused: AtomicBool::new(true),
            minimized: AtomicBool::new(false),
        }
    }

    /// The watcher's batch window for the window's current state.
    fn pace(&self) -> Duration {
        if self.minimized.load(Relaxed) {
            PACE_MINIMIZED
        } else if self.focused.load(Relaxed) {
            PACE_ACTIVE
        } else {
            PACE_BACKGROUND
        }
    }

    fn current(&self) -> Result<Arc<Session>, String> {
        self.session
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| text::NO_SCAN.get().to_string())
    }

    /// Memory held by the current scan's tree, if there is one.
    pub fn tree_memory(&self) -> Option<tessera_core::tree::TreeMemory> {
        let s = self.current().ok()?;
        Some(s.tree().memory())
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
    /// What is below big folders (counts, bytes per category), for the
    /// details card and the legend; see [`TotalsCache`].
    totals: Mutex<TotalsCache>,
    /// The layout the window last asked for, to tell which live changes show.
    shown: Mutex<Shown>,
    /// Changes came in while the window was minimized; tell it on restore.
    stale: AtomicBool,
}

/// The last layout sent to the window, and how much changed since it (or
/// since the last check that found the picture unchanged).
#[derive(Default)]
struct Shown {
    req: Option<LayoutRequest>,
    buf: Vec<u8>,
    bytes_per_px: f64,
    /// [`Tree::changed_bytes`] and the free space at that point.
    changes: u64,
    free: u64,
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
            totals: Mutex::new(TotalsCache::new()),
            shown: Mutex::new(Shown::default()),
            stale: AtomicBool::new(false),
        }
    }

    fn tree(&self) -> RwLockReadGuard<'_, Tree> {
        self.tree.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn root_size(&self) -> u64 {
        self.tree().node(NodeId::ROOT).size()
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

    /// The free-space and not-scanned-yet tiles of a layout of `req.view`.
    fn extras(&self, t: &Tree, req: &LayoutRequest) -> RootExtras {
        if self.is_volume && req.view == NodeId::ROOT {
            RootExtras {
                free: Some(self.free.load(Relaxed)),
                unscanned: (!self.done.load(Relaxed)).then(|| {
                    self.used
                        .load(Relaxed)
                        .saturating_sub(t.node(NodeId::ROOT).size())
                }),
            }
        } else {
            RootExtras::default()
        }
    }

    /// Remembers the layout just sent to the window.
    fn record_shown(&self, req: LayoutRequest, buf: &[u8], changes: u64) {
        let mut g = lock(&self.shown);
        g.req = Some(req);
        g.buf.clear();
        g.buf.extend_from_slice(buf);
        g.bytes_per_px = layout::header_bytes_per_px(buf).map_or(0.0, f64::from);
        g.changes = changes;
        g.free = self.free.load(Relaxed);
        self.stale.store(false, Relaxed);
    }

    /// True when the tree changed enough since the window's last layout that
    /// its picture may differ: first a cheap bound (the bytes changed inside
    /// the view, as px² of it), then, past that, a layout compared with the
    /// one on screen. Changes Windows makes all the time on a system drive
    /// (logs, caches) mostly stop at the first step and cost no redraw.
    /// Renames change no bytes, so a batch that renamed or moved something
    /// in a folder drawn open goes straight to the comparison.
    fn needs_redraw(&self, core: &Core, rep: &BatchReport) -> bool {
        let t = self.tree();
        let now = t.changed_bytes();
        let (req, since, free, bytes_per_px) = {
            let g = lock(&self.shown);
            match g.req {
                Some(req) => (req, g.changes, g.free, g.bytes_per_px),
                None => return true,
            }
        };
        let view = req.view;
        if view != NodeId::ROOT {
            if view.index() >= t.len() || t.is_deleted(view) {
                return true; // the window falls back to a live folder
            }
            let inside = |d: NodeId| d == view || t.ancestors(d).any(|a| a == view);
            if !rep.dirs.iter().any(|&d| inside(d)) {
                return false;
            }
        }
        let mut bytes = now.wrapping_sub(since) as f64;
        if self.is_volume && view == NodeId::ROOT {
            bytes += self.free.load(Relaxed).abs_diff(free) as f64;
        }
        if bytes_per_px > 0.0
            && bytes_per_px.is_finite()
            && bytes < bytes_per_px * REDRAW_PX2
            && !(rep.moved > 0 && self.renamed_in_sight(view, &rep.dirs))
        {
            return false;
        }
        let extras = self.extras(&t, &req);
        let buf = lock(&core.engine).layout(&t, &req, extras, self.rev.load(Relaxed));
        drop(t);
        let mut g = lock(&self.shown);
        if g.req == Some(req) && layout::same_picture(&g.buf, &buf, REDRAW_TOLERANCE_PX) {
            // count again from here; later checks still compare with the
            // picture on screen, so small changes cannot creep past unseen
            g.changes = now;
            g.free = self.free.load(Relaxed);
            return false;
        }
        true
    }

    /// True when one of `dirs` (folders whose entries changed) is the view or
    /// a folder whose entries are drawn, so a rename there may show.
    fn renamed_in_sight(&self, view: NodeId, dirs: &[NodeId]) -> bool {
        if dirs.contains(&view) {
            return true;
        }
        let Ok(d) = layout::decode(&lock(&self.shown).buf) else {
            return true;
        };
        // open folders, and the innermost folder of a folded chain, whose
        // entries are drawn under the chain's rect
        let open: HashSet<u32> = d
            .records
            .iter()
            .filter(|r| r.kind == layout::kind::DIR)
            .flat_map(|r| [r.id, r.target])
            .collect();
        dirs.iter().any(|dir| open.contains(&dir.0))
    }

    /// Tells the window the tree changed; it asks for a new layout.
    fn send_changed(&self, app: &AppHandle) {
        let root = self.root_size();
        let before = self.last_root.swap(root, Relaxed);
        let _ = app.emit(
            "tree-changed",
            TreeChangedEvent {
                rev: self.rev.load(Relaxed),
                bytes_delta: root as i64 - before as i64,
            },
        );
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
        return Err(text::NOT_FOUND.get().replace("{}", &root));
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
    let opts = WatchOptions {
        throttle: core.pace(),
        ..WatchOptions::default()
    };
    match Watcher::start(&s.root, s.tree.clone(), opts, usn, move |rep| {
        on_batch(&app2, &weak, rep)
    }) {
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
/// The window only hears of it when the change can show (see
/// [`Session::needs_redraw`]); nothing is sent while it is minimized.
fn on_batch(app: &AppHandle, s: &Weak<Session>, rep: &BatchReport) {
    let Some(s) = s.upgrade() else { return };
    let core = app.state::<Core>();
    // a batch can still be in flight while a new scan replaces this session;
    // its delta belongs to the old drive
    if !core.is_current(s.id) {
        return;
    }
    s.rev.fetch_add(1, Relaxed);
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
    if core.minimized.load(Relaxed) {
        s.stale.store(true, Relaxed);
        // restored meanwhile: window_state may have looked before we got here
        if !core.minimized.load(Relaxed) && s.stale.swap(false, Relaxed) {
            s.send_changed(app);
        }
        return;
    }
    if s.needs_redraw(&core, rep) {
        s.send_changed(app);
    }
}

/// The main window gained or lost the focus, or was minimized or restored:
/// sets the watcher's pace, and on restore sends what changed meanwhile.
pub fn window_state(app: &AppHandle, focused: Option<bool>, minimized: Option<bool>) {
    let core = app.state::<Core>();
    if let Some(f) = focused {
        core.focused.store(f, Relaxed);
    }
    let restored = minimized.is_some_and(|m| core.minimized.swap(m, Relaxed) && !m);
    let Ok(s) = core.current() else { return };
    if let Some(w) = lock(&s.watcher).as_ref() {
        w.set_throttle(core.pace());
    }
    if restored && s.stale.swap(false, Relaxed) {
        s.send_changed(app);
    }
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
    let extras = s.extras(&t, &req);
    let rev = s.rev.load(Relaxed);
    let buf = lock(&core.engine).layout(&t, &req, extras, rev);
    let changes = t.changed_bytes();
    drop(t);
    s.record_shown(req, &buf, changes);
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
            text::FREE.get(),
            s.root_text.clone(),
            s.free.load(Relaxed),
            text::FREE_HINT.get(),
        ));
    }
    if id == layout::ids::UNSCANNED {
        let u = s.used.load(Relaxed).saturating_sub(s.root_size());
        return Some(blank(
            id,
            "unscanned",
            text::UNSCANNED.get(),
            s.root_text.clone(),
            u,
            text::UNSCANNED_HINT.get(),
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
            text::GROUP.get(),
            t.path_lossy(folder),
            0,
            text::GROUP_HINT.get(),
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
        Some(text::RECYCLE_BIN.get().to_string())
    } else {
        delete::lock_reason(&path).map(reason_text)
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
    let cat = category::of_node(&t, nid);
    // the tree keeps no times: ask the file system, without holding the tree
    drop(t);
    Some(Details {
        id,
        kind: if n.is_dir() { "dir" } else { "file" },
        hint: hint_for(&name, &n),
        name,
        modified: modified_ms(&path),
        path: path.to_string_lossy().into_owned(),
        size,
        disk: n.size(),
        logical: n.logical(),
        share,
        files,
        dirs,
        cat,
        flags,
        lock_reason,
    })
}

/// Last write time of `path` (not following a link), in Unix milliseconds.
fn modified_ms(path: &std::path::Path) -> Option<f64> {
    let t = std::fs::symlink_metadata(path).ok()?.modified().ok()?;
    Some(match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as f64,
        Err(e) => -(e.duration().as_millis() as f64),
    })
}

/// Recursive file and folder counts. While the root is still being scanned
/// the scanner's own counters are used.
fn counts(s: &Session, t: &Tree, id: NodeId) -> (u64, u64) {
    if id == NodeId::ROOT && !s.done.load(Relaxed) {
        let p = s.progress.snapshot();
        return (p.files, p.dirs);
    }
    let tot = totals(s, t, id);
    (tot.files, tot.dirs)
}

/// What is below `id` now. Only folders on the paths that changed since the
/// last call are counted again, so asking after every live update is cheap.
fn totals(s: &Session, t: &Tree, id: NodeId) -> Totals {
    lock(&s.totals).get(t, id)
}

fn hint_for(name: &str, n: &tessera_core::Node) -> Option<String> {
    let h = match name.to_ascii_lowercase().as_str() {
        "hiberfil.sys" => text::HIBERFIL,
        "pagefile.sys" | "swapfile.sys" => text::PAGEFILE,
        "windows.old" => text::WINDOWS_OLD,
        _ if n.flags.contains(NodeFlags::CLOUD) => text::CLOUD,
        _ if n.flags.contains(NodeFlags::INACCESSIBLE) => text::NO_ACCESS,
        _ => return None,
    };
    Some(h.get().into())
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
    Ok(cat_stats_of(&s, NodeId(view), size_mode.into()))
}

/// Bytes per file category below `view`, largest first.
fn cat_stats_of(s: &Session, view: NodeId, mode: SizeMode) -> Vec<CatStat> {
    let t = s.tree();
    if view.index() >= t.len() || t.is_deleted(view) || !t.node(view).is_dir() {
        return Vec::new();
    }
    let bytes = *totals(s, &t, view).bytes(mode);
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
                reason: reason_text(&f.reason),
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
            return Err(text::NO_ITEM.get().into());
        }
        t.path(NodeId(id))
    };
    open_in_explorer(&path)
}

/// Windows' own context menu for `ids` (all in one folder) at `x`, `y` (CSS
/// px in the window), with Explorer's Shift-only verbs if `extended`. Resolves
/// when the menu closes; "delete" is left to the WebView (see [`shell_menu`]).
#[tauri::command]
pub async fn shell_menu(
    app: AppHandle,
    core: State<'_, Core>,
    ids: Vec<u32>,
    x: f64,
    y: f64,
    extended: bool,
) -> Result<shell_menu::Action, String> {
    let s = core.current()?;
    let paths = menu_paths(&s, &ids)?;
    if paths.is_empty() {
        return Ok(shell_menu::Action::None);
    }
    let window = app
        .get_webview_window("main")
        .ok_or("main window missing")?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let req = shell_menu::Request {
        paths,
        x: (x * scale).round() as i32,
        y: (y * scale).round() as i32,
        extended,
        owner: core.owner_hwnd.load(Relaxed),
    };
    // the menu runs on the window's thread and blocks it until it closes
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = tx.send(shell_menu::show(req));
    })
    .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?
}

/// The paths of live items that share one folder, as the shell menu needs them.
fn menu_paths(s: &Session, ids: &[u32]) -> Result<Vec<PathBuf>, String> {
    let t = s.tree();
    let mut folder = None;
    let mut paths = Vec::with_capacity(ids.len());
    for &id in ids {
        let n = NodeId(id);
        if id as usize >= t.len() || t.is_deleted(n) {
            return Err(text::NO_ITEM.get().into());
        }
        let parent = t.node(n).parent;
        if *folder.get_or_insert(parent) != parent {
            return Err(text::MIXED_FOLDERS.get().into());
        }
        paths.push(t.path(n));
    }
    Ok(paths)
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
    Err(text::WINDOWS_ONLY.get().into())
}

// ---------------------------------------------------------------------------
// Text

/// The core's user-facing reasons are fixed English strings; say them in the
/// UI's language. Anything else (e.g. a message from the shell, which Windows
/// already words in the user's language) is passed through.
fn reason_text(reason: &str) -> String {
    reason_in(i18n::lang(), reason)
}

fn reason_in(lang: i18n::Lang, reason: &str) -> String {
    use delete::reasons::*;
    let t = match reason {
        DRIVE_ROOT => text::DRIVE_ROOT,
        SCAN_ROOT => text::SCAN_ROOT,
        WINDOWS => text::WINDOWS,
        PROGRAMS => text::PROGRAMS,
        APP_DATA => text::APP_DATA,
        PROFILES => text::PROFILES,
        PROFILE => text::PROFILE,
        PERSONAL => text::PERSONAL,
        TEMP => text::TEMP,
        CONTAINS => text::CONTAINS,
        STILL_THERE => text::STILL_THERE,
        NOT_RESTORED => text::NOT_RESTORED,
        NAME_TAKEN => text::NAME_TAKEN,
        CANCELLED => text::CANCELLED,
        SKIPPED => text::SKIPPED,
        other => return other.to_string(),
    };
    t.of(lang).to_string()
}

fn error_text(e: &tessera_core::Error) -> String {
    match e {
        tessera_core::Error::Cancelled => text::CANCELLED.get().into(),
        tessera_core::Error::Unsupported("unknown undo token") => text::UNDO_GONE.get().into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_are_translated() {
        use i18n::Lang::{En, ZhCn};
        assert_eq!(
            reason_in(ZhCn, delete::reasons::WINDOWS),
            "Windows 需要这个文件夹。"
        );
        assert_eq!(
            reason_in(En, delete::reasons::WINDOWS),
            "Windows needs this folder."
        );
        // the app words these two differently from the core: no "delete what is inside"
        assert_eq!(
            reason_in(En, delete::reasons::PERSONAL),
            "This is one of your personal folders. Windows and your apps use it."
        );
        assert_eq!(reason_in(ZhCn, "Access is denied."), "Access is denied.");
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
        let users = t.add_child(NodeId::ROOT, b"Users", 0, 0, NodeFlags::DIR);
        let me = t.add_child(users, b"me", 0, 0, NodeFlags::DIR);
        t.add_child(me, b"a.mp4", 4096 * 100, 400_000, NodeFlags::NONE);
        t.add_child(me, b"b.ts", 4096, 900, NodeFlags::NONE);
        let docs = t.add_child(me, b"Docs", 0, 0, NodeFlags::DIR);
        t.add_child(docs, b"c.pdf", 8192, 8000, NodeFlags::NONE);
        t.add_child(
            NodeId::ROOT,
            b"pagefile.sys",
            4096 * 1000,
            4096 * 1000,
            NodeFlags::NONE,
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
        let stats = cat_stats_of(&s, NodeId::ROOT, SizeMode::Disk);
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
        assert_eq!(cat_stats_of(&s, me, SizeMode::Disk).len(), 3);
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

    #[test]
    fn shell_menu_items_share_a_folder() {
        let s = sample();
        let a = id_of(&s, &[b"Users", b"me", b"a.mp4"]);
        let b = id_of(&s, &[b"Users", b"me", b"b.ts"]);
        let c = id_of(&s, &[b"Users", b"me", b"Docs", b"c.pdf"]);
        let paths = menu_paths(&s, &[a.0, b.0]).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths[1].ends_with("b.ts"), "{:?}", paths[1]);
        assert_eq!(menu_paths(&s, &[0]).unwrap(), [PathBuf::from("C:\\")]);
        assert_eq!(
            menu_paths(&s, &[a.0, c.0]).unwrap_err(),
            text::MIXED_FOLDERS.get()
        );
        assert_eq!(menu_paths(&s, &[9999]).unwrap_err(), text::NO_ITEM.get());
        s.tree.write().unwrap().mark_deleted(b);
        assert_eq!(
            menu_paths(&s, &[a.0, b.0]).unwrap_err(),
            text::NO_ITEM.get()
        );
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
