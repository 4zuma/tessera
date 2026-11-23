//! Windows 11: the title bar is the page's own (app/src/components/TitleBar.svelte), as in
//! WinUI apps, so that it can hold Tessera's own controls. The window is built without
//! decorations (Tauri then keeps the frame's shadow and resize borders) and the page marks
//! its title bar with `app-region: drag`, which WebView2 turns into a real caption:
//! dragging, double-click to maximize, Aero Snap and the system menu all come from Windows.
//!
//! The one thing the page cannot give is Windows 11's snap layouts flyout, which opens when
//! the pointer rests on whatever the window reports as its maximize button (`HTMAXBUTTON`
//! from WM_NCHITTEST). The page covers the whole client area, so Windows never asks the
//! window. A small child window with nothing to draw sits over the page's maximize button
//! and answers "transparent" to that question, which sends it on to the main window, and
//! the main window answers `HTMAXBUTTON` there. The pointer over that spot then talks to
//! the main window, which tells the page when to draw the button hovered or pressed (the
//! `caption` event) and maximizes or restores on a click.
//!
//! Windows 10 keeps its own title bar (Mica is missing there, and with it the look this is
//! for, see `platform::build_main_window`). `TESSERA_TITLEBAR=native` keeps it on
//! Windows 11 too.

use serde::Serialize;

/// The page's title bar replaces Windows' one (only asked when Mica applies).
pub fn wanted() -> bool {
    !std::env::var("TESSERA_TITLEBAR").is_ok_and(|v| v.trim().eq_ignore_ascii_case("native"))
}

/// What the page's caption buttons show.
#[derive(Clone, Copy, Default, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CaptionState {
    pub maximized: bool,
    /// the pointer is on the maximize button (Windows tells the main window, not the page)
    pub hover: bool,
    pub pressed: bool,
}

/// Starts answering for the page's maximize button (see the module docs).
pub fn attach(window: &tauri::WebviewWindow) {
    imp::attach(window);
}

/// The page's caption buttons: `"min"`, `"max"` (maximize or restore) or `"close"`.
#[tauri::command]
pub fn caption(window: tauri::WebviewWindow, action: String) -> Result<(), String> {
    let r = match action.as_str() {
        "min" => window.minimize(),
        "max" if window.is_maximized().unwrap_or(false) => window.unmaximize(),
        "max" => window.maximize(),
        "close" => window.close(),
        _ => return Err(format!("unknown caption action {action}")),
    };
    r.map_err(|e| e.to_string())
}

/// Where the page's maximize button is, in CSS px: its distance from the window's right
/// edge, its top, width and height.
#[tauri::command]
pub fn caption_button(window: tauri::WebviewWindow, right: f64, top: f64, width: f64, height: f64) {
    imp::set_button([right, top, width, height]);
    // the child window belongs to the window's thread
    let _ = window.run_on_main_thread(imp::place);
}

/// The caption state now; afterwards the `caption` event brings every change.
#[tauri::command]
pub fn caption_state() -> CaptionState {
    imp::state()
}

#[cfg(windows)]
mod imp {
    use std::sync::{Mutex, OnceLock};

    use tauri::{AppHandle, Emitter, Manager};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::PtInRect;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
    };
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, GetClientRect, GetWindowRect, HTMAXBUTTON, HTTRANSPARENT,
        HWND_TOP, IsZoomed, MA_NOACTIVATE, PostMessageW, RegisterClassExW, SC_MAXIMIZE, SC_RESTORE,
        SIZE_MAXIMIZED, SIZE_MINIMIZED, SM_CYFRAME, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        SetWindowPos, WINDOW_EX_STYLE, WM_DPICHANGED, WM_MOUSEACTIVATE, WM_NCDESTROY, WM_NCHITTEST,
        WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
        WM_SIZE, WM_SYSCOMMAND, WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS,
    };
    use windows::core::w;

    use super::CaptionState;

    const SUBCLASS_ID: usize = 0x7465_7373; // "tess"

    struct Caption {
        app: AppHandle,
        /// the main window and the child over the maximize button (HWNDs as numbers: Send)
        main: isize,
        button: isize,
        /// the page's maximize button in CSS px: right, top, width, height
        css: [f64; 4],
        state: CaptionState,
    }

    static CAPTION: OnceLock<Mutex<Caption>> = OnceLock::new();

    /// Runs `f` on the shared state. Nothing that can send a window message or run script
    /// may happen inside: the window procedure takes the same lock.
    fn with<R>(f: impl FnOnce(&mut Caption) -> R) -> Option<R> {
        let mut g = CAPTION.get()?.lock().unwrap_or_else(|e| e.into_inner());
        Some(f(&mut g))
    }

    /// Changes the caption state and tells the page when it changed.
    fn change(f: impl FnOnce(&mut CaptionState)) {
        let changed = with(|c| {
            let before = c.state;
            f(&mut c.state);
            (c.state != before).then(|| (c.app.clone(), c.state))
        });
        if let Some(Some((app, state))) = changed {
            let _ = app.emit("caption", state);
        }
    }

    fn hwnd(n: isize) -> HWND {
        HWND(n as *mut _)
    }

    pub fn attach(window: &tauri::WebviewWindow) {
        let Ok(main) = window.hwnd() else { return };
        let main = HWND(main.0 as *mut _);
        unsafe {
            let instance = GetModuleHandleW(None).ok().map(|h| h.into());
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(button_proc),
                hInstance: instance.unwrap_or_default(),
                lpszClassName: w!("TesseraCaptionButton"),
                ..Default::default()
            };
            RegisterClassExW(&class);
            // no size until the page says where its button is
            let Ok(button) = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("TesseraCaptionButton"),
                w!(""),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                0,
                0,
                Some(main),
                None,
                instance,
                None,
            ) else {
                return;
            };
            let state = CaptionState {
                maximized: IsZoomed(main).as_bool(),
                ..Default::default()
            };
            let _ = CAPTION.set(Mutex::new(Caption {
                app: window.app_handle().clone(),
                main: main.0 as isize,
                button: button.0 as isize,
                css: [0.0; 4],
                state,
            }));
            let _ = SetWindowSubclass(main, Some(main_proc), SUBCLASS_ID, 0);
        }
    }

    pub fn set_button(css: [f64; 4]) {
        with(|c| c.css = css);
    }

    pub fn state() -> CaptionState {
        with(|c| c.state).unwrap_or_default()
    }

    /// Puts the child window over the page's button: it keeps out of the strip at the top
    /// where the window is resized (none while maximized). On the window's thread only.
    pub fn place() {
        let Some((main, button, [right, top, w, h])) =
            with(|c| (hwnd(c.main), hwnd(c.button), c.css))
        else {
            return;
        };
        unsafe {
            let mut client = RECT::default();
            if w <= 0.0 || h <= 0.0 || GetClientRect(main, &mut client).is_err() {
                let _ = SetWindowPos(
                    button,
                    Some(HWND_TOP),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_HIDEWINDOW,
                );
                return;
            }
            let dpi = GetDpiForWindow(main).max(96);
            let k = dpi as f64 / 96.0;
            let x = client.right - ((right + w) * k).round() as i32;
            let y0 = (top * k).round() as i32;
            let y1 = ((top + h) * k).round() as i32;
            let edge = if IsZoomed(main).as_bool() {
                0
            } else {
                GetSystemMetricsForDpi(SM_CYFRAME, dpi)
            };
            let y = y0.max(edge);
            let _ = SetWindowPos(
                button,
                Some(HWND_TOP),
                x,
                y,
                (w * k).round() as i32,
                (y1 - y).max(0),
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }

    /// The child over the page's button: draws nothing (the window it sits in has no
    /// surface to draw on) and passes every hit test on to the main window.
    unsafe extern "system" fn button_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
        }
    }

    fn on_button(main: HWND, lp: LPARAM) -> bool {
        let pt = POINT {
            x: (lp.0 & 0xffff) as i16 as i32,
            y: ((lp.0 >> 16) & 0xffff) as i16 as i32,
        };
        let Some(button) = with(|c| (c.main == main.0 as isize).then(|| hwnd(c.button))).flatten()
        else {
            return false;
        };
        let mut r = RECT::default();
        unsafe { GetWindowRect(button, &mut r) }.is_ok()
            && r.right > r.left
            && unsafe { PtInRect(&r, pt) }.as_bool()
    }

    unsafe extern "system" fn main_proc(
        h: HWND,
        msg: u32,
        wp: WPARAM,
        lp: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        let max = wp.0 == HTMAXBUTTON as usize;
        match msg {
            WM_NCHITTEST if on_button(h, lp) => return LRESULT(HTMAXBUTTON as isize),
            WM_NCMOUSEMOVE => {
                change(|s| {
                    s.hover = max;
                    s.pressed &= max;
                });
                if max {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: h,
                        dwHoverTime: 0,
                    };
                    let _ = unsafe { TrackMouseEvent(&mut t) };
                }
            }
            WM_NCMOUSELEAVE => {
                change(|s| (s.hover, s.pressed) = (false, false));
            }
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if max => {
                change(|s| s.pressed = true);
                return LRESULT(0);
            }
            WM_NCLBUTTONUP if max => {
                let was = with(|c| c.state.pressed).unwrap_or(false);
                change(|s| s.pressed = false);
                if was {
                    let cmd = if unsafe { IsZoomed(h) }.as_bool() {
                        SC_RESTORE
                    } else {
                        SC_MAXIMIZE
                    };
                    let _ = unsafe {
                        PostMessageW(Some(h), WM_SYSCOMMAND, WPARAM(cmd as usize), LPARAM(0))
                    };
                }
                return LRESULT(0);
            }
            WM_SIZE if wp.0 != SIZE_MINIMIZED as usize => {
                let r = unsafe { DefSubclassProc(h, msg, wp, lp) };
                change(|s| s.maximized = wp.0 == SIZE_MAXIMIZED as usize);
                place();
                return r;
            }
            WM_DPICHANGED => {
                let r = unsafe { DefSubclassProc(h, msg, wp, lp) };
                place();
                return r;
            }
            WM_NCDESTROY => {
                let _ = unsafe { RemoveWindowSubclass(h, Some(main_proc), SUBCLASS_ID) };
            }
            _ => {}
        }
        unsafe { DefSubclassProc(h, msg, wp, lp) }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::CaptionState;
    pub fn attach(_window: &tauri::WebviewWindow) {}
    pub fn set_button(_css: [f64; 4]) {}
    pub fn place() {}
    pub fn state() -> CaptionState {
        CaptionState::default()
    }
}
