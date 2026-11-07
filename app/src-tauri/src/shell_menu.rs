//! Windows' own context menu for items of the tree: the classic menu that
//! Explorer shows under "Show more options" (or with Shift+right-click).
//!
//! Windows 11's newer context menu has no public API; apps can only host the
//! classic one, which is also the complete one. Tessera's own menu plays the
//! part of the short menu and opens this one from its last item, like
//! Explorer does.
//!
//! The menu comes from the items' parent folder (`IShellFolder::GetUIObjectOf`),
//! so all items must share one folder. Two verbs are not left to the shell:
//!
//! * "delete" comes back as [`Action::Recycle`] (or [`Action::Purge`] with
//!   Shift held), and the WebView deletes through Tessera's own path, with its
//!   protected folders, undo and animation;
//! * "rename" is never offered (no `CMF_CANRENAME`): the shell would ask the
//!   host for in-place editing, which only Explorer's views have.
//!
//! Everything else runs as in Explorer, and whatever it changes on disk
//! reaches the tree through the watcher.
//!
//! # Threading
//!
//! [`show`] must run on the thread that owns the main window: the menu's
//! owner has to belong to the calling thread, and submenus such as "Open
//! with" and "Send to" are filled in through messages to that window, which
//! is subclassed while the menu is open to pass them on to the handler
//! (`IContextMenu2`/`IContextMenu3`). Tauri's main thread is that thread,
//! and already a single-threaded COM apartment (WebView2 needs one).
//!
//! Building the menu runs every installed menu extension (7-Zip, OneDrive,
//! Defender, packaged apps…) in our process. The first time, Windows loads
//! their DLLs and fills its caches for the file type, which can take
//! seconds; Explorer hides that because it has been running since sign-in.
//! On the window's thread that would hang the window, so [`prepare`] builds
//! the same menu once on a background thread first (the "shell-menu" thread,
//! which stays for the life of the process so the DLLs stay loaded), and
//! [`show`] then finds everything loaded and opens at once.
//!
//! # Theme
//!
//! The menu follows Windows' light or dark app mode, as the WebView does:
//! tao opts the process and its windows in to dark menus at startup
//! (uxtheme's `SetPreferredAppMode(AllowDark)` and `AllowDarkModeForWindow`).

use std::path::PathBuf;

use serde::Serialize;

/// What became of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// Closed without choosing anything.
    None,
    /// The shell ran the chosen command.
    Invoked,
    /// "Delete" was chosen: Tessera moves the items to the Recycle Bin itself.
    Recycle,
    /// "Delete" was chosen with Shift held: Tessera asks to delete them permanently.
    Purge,
}

/// A menu to show.
pub struct Request {
    /// The items, all in one folder.
    pub paths: Vec<PathBuf>,
    /// Where the menu opens, in physical pixels from the top left corner of
    /// the window's client area.
    pub x: i32,
    pub y: i32,
    /// Include the verbs Explorer shows only with Shift held, such as "Copy
    /// as path".
    pub extended: bool,
    /// The main window (`HWND`), owner of the menu and of whatever the chosen
    /// command shows.
    pub owner: isize,
}

/// Shows the menu at the requested point, blocks until it closes, and runs
/// the chosen command. Call on the main window's thread (see the module docs).
pub fn show(req: Request) -> Result<Action, String> {
    imp::show(req)
}

/// Builds the menu for `paths` (all in one folder) on the background thread,
/// so that the menu extensions are loaded before [`show`] needs them. Blocks
/// until done, so call it off the async runtime. Repeating it for the same
/// items soon after is free.
pub fn prepare(paths: Vec<PathBuf>) -> Result<(), String> {
    imp::prepare(paths)
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::ptr::null_mut;
    use std::sync::mpsc::{Sender, channel, sync_channel};
    use std::sync::{Mutex, OnceLock, PoisonError};
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{
        E_INVALIDARG, HANDLE, HWND, LPARAM, LRESULT, POINT, WAIT_OBJECT_0, WPARAM,
    };
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree,
    };
    use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_CONTROL_DOWN, CMIC_MASK_PTINVOKE,
        CMIC_MASK_SHIFT_DOWN, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, DefSubclassProc,
        GCS_VERBW, IContextMenu, IContextMenu2, IContextMenu3, ILFindLastID, IShellFolder,
        RemoveWindowSubclass, SEE_MASK_ASYNCOK, SEE_MASK_UNICODE, SHBindToParent,
        SHParseDisplayName, SetWindowSubclass,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, DestroyMenu, DispatchMessageW, GetMenuItemCount, GetSubMenu, HMENU, MSG,
        MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
        SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, TranslateMessage,
        WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WM_QUIT,
    };
    use windows::core::{HSTRING, Interface, PCSTR, PCWSTR, PSTR};

    use super::{Action, Request};

    /// Command ids the handler may use: QueryContextMenu numbers its items
    /// from `FIRST`, and 0 means that nothing was chosen.
    const FIRST: u32 = 1;
    const LAST: u32 = 0x7fff;
    /// Identifies our subclass of the main window.
    const SUBCLASS_ID: usize = 0x5e11;
    /// A [`prepare`] for the same items within this long is skipped.
    const STILL_WARM: Duration = Duration::from_secs(60);

    /// The open menu's handler, for the subclass procedure.
    struct Handler {
        cm2: Option<IContextMenu2>,
        cm3: Option<IContextMenu3>,
    }

    thread_local! {
        static OPEN: RefCell<Option<Handler>> = const { RefCell::new(None) };
    }

    /// An absolute item ID list from the shell, freed on drop.
    struct Pidl(*mut ITEMIDLIST);

    impl Drop for Pidl {
        fn drop(&mut self) {
            unsafe { CoTaskMemFree(Some(self.0 as *const _)) }
        }
    }

    struct Menu(HMENU);

    impl Drop for Menu {
        fn drop(&mut self) {
            let _ = unsafe { DestroyMenu(self.0) };
        }
    }

    /// The context menu handler for `paths`, which share one folder, filled
    /// into a new popup menu. The ID lists must outlive the handler.
    unsafe fn build(
        owner: HWND,
        paths: &[PathBuf],
        flags: u32,
    ) -> windows::core::Result<(IContextMenu, Menu, Vec<Pidl>)> {
        let mut pidls = Vec::with_capacity(paths.len());
        for p in paths {
            let mut pidl = null_mut();
            unsafe { SHParseDisplayName(&HSTRING::from(p.as_os_str()), None, &mut pidl, 0, None)? };
            pidls.push(Pidl(pidl));
        }
        let first = pidls
            .first()
            .ok_or(windows::core::Error::from(E_INVALIDARG))?;
        // a drive's parent is "This PC", so a drive gets the menu Explorer shows for it
        let folder: IShellFolder = unsafe { SHBindToParent(first.0, None)? };
        let children: Vec<*const ITEMIDLIST> = pidls
            .iter()
            .map(|p| unsafe { ILFindLastID(p.0) } as *const _)
            .collect();
        let cm: IContextMenu = unsafe { folder.GetUIObjectOf(owner, &children, None)? };
        let menu = Menu(unsafe { CreatePopupMenu()? });
        unsafe { cm.QueryContextMenu(menu.0, 0, FIRST, LAST, flags).ok()? };
        Ok((cm, menu, pidls))
    }

    pub fn show(req: Request) -> Result<Action, String> {
        unsafe { run(&req) }.map_err(|e| e.message())
    }

    unsafe fn run(req: &Request) -> windows::core::Result<Action> {
        let owner = HWND(req.owner as *mut _);
        let flags = CMF_NORMAL | if req.extended { CMF_EXTENDEDVERBS } else { 0 };
        let (cm, menu, _pidls) = unsafe { build(owner, &req.paths, flags)? };

        let mut pt = POINT { x: req.x, y: req.y };
        let _ = unsafe { ClientToScreen(owner, &mut pt) };

        OPEN.with(|o| {
            *o.borrow_mut() = Some(Handler {
                cm2: cm.cast().ok(),
                cm3: cm.cast().ok(),
            })
        });
        let subclassed =
            unsafe { SetWindowSubclass(owner, Some(forward), SUBCLASS_ID, 0) }.as_bool();
        let cmd = unsafe {
            TrackPopupMenuEx(
                menu.0,
                (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
                pt.x,
                pt.y,
                owner,
                None,
            )
        }
        .0 as u32;
        if subclassed {
            let _ = unsafe { RemoveWindowSubclass(owner, Some(forward), SUBCLASS_ID) };
        }
        OPEN.with(|o| o.borrow_mut().take());

        if cmd < FIRST {
            return Ok(Action::None);
        }
        let offset = (cmd - FIRST) as usize;
        // the keys as they were when the item was clicked
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        if unsafe { verb(&cm, offset) }.is_some_and(|v| v.eq_ignore_ascii_case("delete")) {
            return Ok(if shift {
                Action::Purge
            } else {
                Action::Recycle
            });
        }

        let mut mask = SEE_MASK_UNICODE | SEE_MASK_ASYNCOK | CMIC_MASK_PTINVOKE;
        if shift {
            mask |= CMIC_MASK_SHIFT_DOWN;
        }
        if ctrl {
            mask |= CMIC_MASK_CONTROL_DOWN;
        }
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: mask,
            hwnd: owner,
            // MAKEINTRESOURCE: the command's offset instead of a verb string
            lpVerb: PCSTR(offset as *const u8),
            lpVerbW: PCWSTR(offset as *const u16),
            nShow: SW_SHOWNORMAL.0,
            ptInvoke: pt,
            ..Default::default()
        };
        unsafe { cm.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)? };
        Ok(Action::Invoked)
    }

    /// The language-independent verb of a command ("delete", "properties"…),
    /// if its handler names one.
    unsafe fn verb(cm: &IContextMenu, offset: usize) -> Option<String> {
        let mut buf = [0u16; 64];
        unsafe {
            cm.GetCommandString(
                offset,
                GCS_VERBW,
                None,
                PSTR(buf.as_mut_ptr().cast()),
                buf.len() as u32,
            )
        }
        .ok()?;
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]))
    }

    /// Passes the messages that build submenus and owner-drawn items on to
    /// the open menu's handler.
    unsafe extern "system" fn forward(
        hwnd: HWND,
        msg: u32,
        wp: WPARAM,
        lp: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if matches!(
            msg,
            WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR
        ) {
            let handled = OPEN.with(|o| {
                let o = o.borrow();
                let h = o.as_ref()?;
                if let Some(cm3) = &h.cm3 {
                    let mut res = LRESULT(0);
                    unsafe { cm3.HandleMenuMsg2(msg, wp, lp, Some(&mut res)) }.ok()?;
                    Some(res)
                } else if let Some(cm2) = &h.cm2 {
                    // WM_MENUCHAR needs a result, which only IContextMenu3 gives
                    if msg == WM_MENUCHAR {
                        return None;
                    }
                    unsafe { cm2.HandleMenuMsg(msg, wp, lp) }.ok()?;
                    Some(LRESULT(0))
                } else {
                    None
                }
            });
            if let Some(res) = handled {
                return res;
            }
        }
        unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
    }

    // -----------------------------------------------------------------------
    // Loading the extensions ahead of time

    /// The items [`prepare`] last finished, and when.
    static WARM: Mutex<Option<(Vec<PathBuf>, Instant)>> = Mutex::new(None);

    pub fn prepare(paths: Vec<PathBuf>) -> Result<(), String> {
        let (tx, rx) = sync_channel(1);
        let queued = worker::run(Box::new(move || {
            let _ = tx.send(warm(paths));
        }));
        if !queued {
            // no background thread: show() does all the work, as it can
            return Ok(());
        }
        rx.recv().unwrap_or(Ok(()))
    }

    /// Runs on the background thread.
    fn warm(paths: Vec<PathBuf>) -> Result<(), String> {
        let mut last = WARM.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((p, at)) = &*last
            && *p == paths
            && at.elapsed() < STILL_WARM
        {
            return Ok(());
        }
        unsafe { load(&paths) }.map_err(|e| e.message())?;
        *last = Some((paths, Instant::now()));
        Ok(())
    }

    unsafe fn load(paths: &[PathBuf]) -> windows::core::Result<()> {
        let (cm, menu, _pidls) = unsafe { build(HWND::default(), paths, CMF_NORMAL)? };
        // fill the submenus ("Open with", "Send to"…) too, as opening them would,
        // so that their first hover doesn't wait either
        if let Ok(cm3) = cm.cast::<IContextMenu3>() {
            for i in 0..unsafe { GetMenuItemCount(Some(menu.0)) }.max(0) {
                let sub = unsafe { GetSubMenu(menu.0, i) };
                if !sub.is_invalid() {
                    let (wp, lp) = (WPARAM(sub.0 as usize), LPARAM(i as isize));
                    let _ = unsafe { cm3.HandleMenuMsg2(WM_INITMENUPOPUP, wp, lp, None) };
                }
            }
        }
        Ok(())
    }

    /// The "shell-menu" thread: a single-threaded COM apartment that runs
    /// jobs and pumps window messages for whatever the extensions create on
    /// it. It waits without using any CPU and lives as long as the process,
    /// so the extensions' DLLs stay loaded.
    mod worker {
        use super::*;

        pub type Job = Box<dyn FnOnce() + Send>;

        struct Worker {
            jobs: Sender<Job>,
            /// Auto-reset event that wakes the thread for new jobs.
            wake: isize,
        }

        static WORKER: OnceLock<Option<Worker>> = OnceLock::new();

        /// Queues `job`; false when the thread could not be started.
        pub fn run(job: Job) -> bool {
            let Some(w) = WORKER.get_or_init(start) else {
                return false;
            };
            if w.jobs.send(job).is_err() {
                return false;
            }
            unsafe { SetEvent(HANDLE(w.wake as *mut _)) }.is_ok()
        }

        fn start() -> Option<Worker> {
            let wake = unsafe { CreateEventW(None, false, false, None) }.ok()?;
            let wake = wake.0 as isize;
            let (jobs, rx) = channel::<Job>();
            std::thread::Builder::new()
                .name("shell-menu".into())
                .spawn(move || unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
                    let events = [HANDLE(wake as *mut _)];
                    let mut msg = MSG::default();
                    loop {
                        while let Ok(job) = rx.try_recv() {
                            job();
                        }
                        let r = MsgWaitForMultipleObjectsEx(
                            Some(&events),
                            INFINITE,
                            QS_ALLINPUT,
                            MWMO_INPUTAVAILABLE,
                        );
                        if r == WAIT_OBJECT_0 {
                            continue;
                        }
                        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                            if msg.message == WM_QUIT {
                                return;
                            }
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                })
                .ok()?;
            Some(Worker { jobs, wake })
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::PathBuf;

    use super::{Action, Request};

    pub fn show(_req: Request) -> Result<Action, String> {
        Err(crate::i18n::text::WINDOWS_ONLY.get().into())
    }

    pub fn prepare(_paths: Vec<PathBuf>) -> Result<(), String> {
        Ok(())
    }
}
