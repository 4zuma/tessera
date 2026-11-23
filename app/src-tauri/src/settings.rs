//! What Tessera remembers between runs: the window's place and size, and a few view
//! options. Tessera stays one portable exe that writes no files of its own, so these
//! live in the registry, under one key: `HKEY_CURRENT_USER\Software\Tessera`.
//!
//! Only what differs from the default is written. A setting put back to its default
//! loses its value, and the key itself goes once it holds no value, so someone who never
//! changes anything never gets a key. "Clear saved settings" (the ⋯ menu) deletes it.
//!
//! The values:
//!
//! * `Window` (REG_SZ) `"x,y,width,height,dpi"`, plus `",max"` when maximized: the
//!   window's restored rectangle as Windows reports it (GetWindowPlacement, physical
//!   pixels) and the DPI it was saved at. Written when the window closes.
//! * whatever the WebView saves through [`settings_save`]: names of letters and digits,
//!   numbers as REG_DWORD and text as REG_SZ (see `app/src/lib/settings.ts`).

use std::sync::Mutex;

use serde_json::{Map, Value};

/// The key under HKEY_CURRENT_USER.
pub const KEY: &str = r"Software\Tessera";
/// The value the window's placement is kept in; the WebView may not write it.
const WINDOW: &str = "Window";

/// A window's restored rectangle (physical pixels, workspace coordinates as
/// GetWindowPlacement gives them), the DPI it had, and whether it was maximized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub dpi: u32,
    pub max: bool,
}

impl Placement {
    /// The same spot on screen: rectangle and maximized state (the DPI follows from them).
    pub fn same_spot(&self, o: &Placement) -> bool {
        (self.x, self.y, self.w, self.h, self.max) == (o.x, o.y, o.w, o.h, o.max)
    }

    pub fn format(&self) -> String {
        let max = if self.max { ",max" } else { "" };
        format!(
            "{},{},{},{},{}{max}",
            self.x, self.y, self.w, self.h, self.dpi
        )
    }

    pub fn parse(s: &str) -> Option<Placement> {
        let mut it = s.split(',').map(str::trim);
        let mut num = || it.next()?.parse::<i64>().ok();
        let (x, y, w, h, dpi) = (num()?, num()?, num()?, num()?, num()?);
        let max = match it.next() {
            None => false,
            Some("max") => true,
            Some(_) => return None,
        };
        if it.next().is_some() {
            return None;
        }
        let i = |v: i64| i32::try_from(v).ok();
        let p = Placement {
            x: i(x)?,
            y: i(y)?,
            w: i(w)?,
            h: i(h)?,
            dpi: u32::try_from(dpi).ok()?,
            max,
        };
        // a window smaller than this is not one Tessera made (its minimum is 720 x 480 px)
        (p.w >= 200
            && p.h >= 150
            && p.w <= 100_000
            && p.h <= 100_000
            && (48..=1920).contains(&p.dpi))
        .then_some(p)
    }

    /// Where the window goes on a work area `work` (left, top, right, bottom) of a monitor
    /// whose DPI is `dpi`: the size follows the DPI change, the window fits the work area,
    /// and it is moved inside when it sticks out.
    pub fn fit(&self, work: (i32, i32, i32, i32), dpi: u32) -> Placement {
        let scale = |v: i32| {
            if dpi == self.dpi || self.dpi == 0 {
                v
            } else {
                (v as i64 * dpi as i64 / self.dpi as i64) as i32
            }
        };
        let (l, t, r, b) = work;
        let w = scale(self.w).min(r - l).max(1);
        let h = scale(self.h).min(b - t).max(1);
        let x = self.x.min(r - w).max(l);
        let y = self.y.min(b - h).max(t);
        Placement {
            x,
            y,
            w,
            h,
            dpi,
            max: self.max,
        }
    }
}

/// Whether a name may be stored for the WebView: 1 to 32 ASCII letters and digits, and
/// not the window's own value.
pub fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric())
        && !name.eq_ignore_ascii_case(WINDOW)
}

/// A value as the registry holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stored {
    Dword(u32),
    Text(String),
}

impl Stored {
    fn from_json(v: &Value) -> Result<Stored, String> {
        match v {
            Value::Bool(b) => Ok(Stored::Dword(*b as u32)),
            Value::Number(n) => n
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .map(Stored::Dword)
                .ok_or_else(|| format!("not a whole number from 0 to 2^32: {n}")),
            Value::String(s) if s.chars().count() <= 1024 && !s.contains('\0') => {
                Ok(Stored::Text(s.clone()))
            }
            _ => Err("only numbers, true/false and short text can be saved".into()),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Stored::Dword(n) => Value::from(*n),
            Stored::Text(s) => Value::from(s.as_str()),
        }
    }
}

/// Managed by Tauri: what the window's placement is compared with when it closes.
#[derive(Default)]
pub struct WindowMemory {
    /// Where the window opens when nothing is saved (centred, the default size).
    default: Mutex<Option<Placement>>,
    /// Where the window was when the user cleared the saved settings: closing it there
    /// writes nothing, so clearing is not undone by quitting.
    cleared: Mutex<Option<Placement>>,
}

/// Puts the (still hidden) main window where it was last closed, and notes where it
/// would have opened otherwise.
pub fn restore_window(window: &tauri::WebviewWindow, memory: &WindowMemory) {
    let Some(hwnd) = imp::hwnd(window) else {
        return;
    };
    let default = imp::placement(hwnd);
    *memory.default.lock().unwrap_or_else(|e| e.into_inner()) = default;
    let saved = match imp::Reg::open(KEY).and_then(|k| k.get(WINDOW)) {
        Some(Stored::Text(s)) => Placement::parse(&s),
        _ => None,
    };
    let Some(saved) = saved else { return };
    if let Some(p) = imp::place(hwnd, &saved)
        && p.max
    {
        // tao keeps this until the window is shown, then shows it maximized
        let _ = window.maximize();
    }
}

/// Saves where the main window is, unless that is where it opens by default (then the
/// saved value goes) or where it was when the settings were cleared.
pub fn save_window(window: &tauri::WebviewWindow, memory: &WindowMemory) {
    let Some(hwnd) = imp::hwnd(window) else {
        return;
    };
    let Some(now) = imp::placement(hwnd) else {
        return;
    };
    let default = *memory.default.lock().unwrap_or_else(|e| e.into_inner());
    let cleared = *memory.cleared.lock().unwrap_or_else(|e| e.into_inner());
    if default.is_some_and(|d| d.same_spot(&now)) {
        let _ = remove(KEY, WINDOW);
    } else if !cleared.is_some_and(|c| c.same_spot(&now)) {
        let _ = imp::Reg::create(KEY).and_then(|k| k.set(WINDOW, &Stored::Text(now.format())));
    }
}

/// Deletes `name` from `key`, and `key` itself once it holds nothing.
fn remove(key: &str, name: &str) -> Result<(), String> {
    let Some(k) = imp::Reg::open_rw(key) else {
        return Ok(());
    };
    k.delete(name)?;
    if k.is_empty() {
        drop(k);
        imp::delete_key(key)?;
    }
    Ok(())
}

/// Everything saved under `key`, except the window's placement.
fn load(key: &str) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(k) = imp::Reg::open(key) {
        for (name, v) in k.all() {
            if valid_name(&name) {
                out.insert(name, v.to_json());
            }
        }
    }
    out
}

/// Saves `value` as `name` under `key`; `null` deletes it (the setting is back at its default).
fn save(key: &str, name: &str, value: &Value) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("not a setting name: {name}"));
    }
    if value.is_null() {
        return remove(key, name);
    }
    let v = Stored::from_json(value)?;
    imp::Reg::create(key)?.set(name, &v)
}

// ---------------------------------------------------------------------------
// Commands (docs/ipc.md)

/// The settings the WebView saved, as `{ name: number | string }`.
#[tauri::command]
pub fn settings_load() -> Map<String, Value> {
    load(KEY)
}

/// Saves one setting; `null` means it is back at its default and deletes it.
#[tauri::command]
pub fn settings_save(name: String, value: Value) -> Result<(), String> {
    save(KEY, &name, &value)
}

/// Deletes the whole key. The window's current place is not saved again on closing,
/// unless the window is moved or resized first.
#[tauri::command]
pub fn settings_clear(
    window: tauri::WebviewWindow,
    memory: tauri::State<'_, WindowMemory>,
) -> Result<(), String> {
    imp::delete_key(KEY)?;
    let now = imp::hwnd(&window).and_then(imp::placement);
    *memory.cleared.lock().unwrap_or_else(|e| e.into_inner()) = now;
    Ok(())
}

#[cfg(windows)]
mod imp {
    use super::{Placement, Stored};
    use windows::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, HWND, POINT,
        RECT, WIN32_ERROR,
    };
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
        MonitorFromPoint, MonitorFromRect,
    };
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE,
        REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteKeyW,
        RegDeleteTreeW, RegDeleteValueW, RegEnumValueW, RegOpenKeyExW, RegQueryInfoKeyW,
        RegQueryValueExW, RegSetValueExW,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowPlacement, SW_HIDE, SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED, SetWindowPlacement,
        WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED,
    };
    use windows::core::{HSTRING, PCWSTR, PWSTR};

    fn err(what: &str, e: WIN32_ERROR) -> String {
        format!(
            "{what}: {}",
            windows::core::Error::from(e.to_hresult()).message()
        )
    }

    /// An open registry key, closed when dropped.
    pub struct Reg(HKEY);

    impl Drop for Reg {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    impl Reg {
        fn open_with(key: &str, access: REG_SAM_FLAGS) -> Option<Reg> {
            let mut h = HKEY::default();
            let r = unsafe {
                RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(key), None, access, &mut h)
            };
            (r == ERROR_SUCCESS).then_some(Reg(h))
        }

        /// The key for reading, if it exists.
        pub fn open(key: &str) -> Option<Reg> {
            Self::open_with(key, KEY_READ)
        }

        /// The key for reading and writing, if it exists.
        pub fn open_rw(key: &str) -> Option<Reg> {
            Self::open_with(key, KEY_READ | KEY_WRITE)
        }

        /// The key for writing, made if it does not exist yet.
        pub fn create(key: &str) -> Result<Reg, String> {
            let mut h = HKEY::default();
            let r = unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    &HSTRING::from(key),
                    None,
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_READ | KEY_WRITE,
                    None,
                    &mut h,
                    None,
                )
            };
            if r == ERROR_SUCCESS {
                Ok(Reg(h))
            } else {
                Err(err("RegCreateKeyEx", r))
            }
        }

        pub fn get(&self, name: &str) -> Option<Stored> {
            let mut ty = REG_VALUE_TYPE::default();
            let mut buf = vec![0u8; 4096];
            let mut len = buf.len() as u32;
            let r = unsafe {
                RegQueryValueExW(
                    self.0,
                    &HSTRING::from(name),
                    None,
                    Some(&mut ty as *mut _),
                    Some(buf.as_mut_ptr()),
                    Some(&mut len as *mut _),
                )
            };
            if r == ERROR_SUCCESS {
                decode(ty, &buf[..len as usize])
            } else {
                None
            }
        }

        pub fn set(&self, name: &str, v: &Stored) -> Result<(), String> {
            let (ty, bytes) = match v {
                Stored::Dword(n) => (REG_DWORD, n.to_le_bytes().to_vec()),
                Stored::Text(s) => {
                    let w: Vec<u16> = s.encode_utf16().chain([0]).collect();
                    (REG_SZ, w.iter().flat_map(|u| u.to_le_bytes()).collect())
                }
            };
            let r = unsafe { RegSetValueExW(self.0, &HSTRING::from(name), None, ty, Some(&bytes)) };
            if r == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(err("RegSetValueEx", r))
            }
        }

        pub fn delete(&self, name: &str) -> Result<(), String> {
            let r = unsafe { RegDeleteValueW(self.0, &HSTRING::from(name)) };
            if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(err("RegDeleteValue", r))
            }
        }

        /// No values and no subkeys.
        pub fn is_empty(&self) -> bool {
            let (mut keys, mut values) = (0u32, 0u32);
            let r = unsafe {
                RegQueryInfoKeyW(
                    self.0,
                    None,
                    None,
                    None,
                    Some(&mut keys as *mut _),
                    None,
                    None,
                    Some(&mut values as *mut _),
                    None,
                    None,
                    None,
                    None,
                )
            };
            r == ERROR_SUCCESS && keys == 0 && values == 0
        }

        /// Every value of a type Tessera writes, by name.
        pub fn all(&self) -> Vec<(String, Stored)> {
            let mut out = Vec::new();
            for i in 0.. {
                let mut name = [0u16; 256];
                let mut name_len = name.len() as u32;
                let mut ty = 0u32;
                let mut buf = vec![0u8; 4096];
                let mut len = buf.len() as u32;
                let r = unsafe {
                    RegEnumValueW(
                        self.0,
                        i,
                        Some(PWSTR(name.as_mut_ptr())),
                        &mut name_len,
                        None,
                        Some(&mut ty as *mut _),
                        Some(buf.as_mut_ptr()),
                        Some(&mut len as *mut _),
                    )
                };
                if r == ERROR_NO_MORE_ITEMS || (r != ERROR_SUCCESS && r != ERROR_MORE_DATA) {
                    break;
                }
                if r == ERROR_MORE_DATA {
                    continue; // nothing Tessera writes is that long
                }
                let name = String::from_utf16_lossy(&name[..name_len as usize]);
                if let Some(v) = decode(REG_VALUE_TYPE(ty), &buf[..len as usize]) {
                    out.push((name, v));
                }
            }
            out
        }
    }

    fn decode(ty: REG_VALUE_TYPE, b: &[u8]) -> Option<Stored> {
        if ty == REG_DWORD && b.len() == 4 {
            Some(Stored::Dword(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
        } else if ty == REG_SZ {
            let w: Vec<u16> = b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            let end = w.iter().position(|&u| u == 0).unwrap_or(w.len());
            Some(Stored::Text(String::from_utf16_lossy(&w[..end])))
        } else {
            None
        }
    }

    /// Deletes `key` (under HKEY_CURRENT_USER) and everything in it; fine if it is not there.
    pub fn delete_key(key: &str) -> Result<(), String> {
        let k = HSTRING::from(key);
        let r = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &k) };
        if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
            return Err(err("RegDeleteTree", r));
        }
        let r = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, &k) };
        if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(err("RegDeleteKey", r))
        }
    }

    pub fn hwnd(window: &tauri::WebviewWindow) -> Option<HWND> {
        window.hwnd().ok().map(|h| HWND(h.0 as *mut _))
    }

    pub fn placement(hwnd: HWND) -> Option<Placement> {
        let mut wp = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        unsafe { GetWindowPlacement(hwnd, &mut wp) }.ok()?;
        let r = wp.rcNormalPosition;
        let show = wp.showCmd as i32;
        let max = show == SW_SHOWMAXIMIZED.0
            || (show == SW_SHOWMINIMIZED.0 && wp.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0);
        Some(Placement {
            x: r.left,
            y: r.top,
            w: r.right - r.left,
            h: r.bottom - r.top,
            dpi: unsafe { GetDpiForWindow(hwnd) },
            max,
        })
    }

    /// GetWindowPlacement's rectangles are in workspace coordinates: screen coordinates
    /// moved by the room the taskbar takes at the top or left of the primary monitor.
    fn workspace_offset() -> (i32, i32) {
        let primary = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        match monitor_info(primary) {
            Some(mi) => (
                mi.rcWork.left - mi.rcMonitor.left,
                mi.rcWork.top - mi.rcMonitor.top,
            ),
            None => (0, 0),
        }
    }

    fn monitor_info(m: HMONITOR) -> Option<MONITORINFO> {
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetMonitorInfoW(m, &mut mi) }
            .as_bool()
            .then_some(mi)
    }

    /// Moves the hidden window to `saved`, fitted to the monitor it is on now. Nothing
    /// happens when that monitor is gone (the window then opens where it would anyway).
    pub fn place(hwnd: HWND, saved: &Placement) -> Option<Placement> {
        let (ox, oy) = workspace_offset();
        // the strip where the title bar is must be on a monitor
        let strip = RECT {
            left: saved.x + ox + 40,
            top: saved.y + oy,
            right: saved.x + ox + saved.w - 40,
            bottom: saved.y + oy + 32,
        };
        let mon = unsafe { MonitorFromRect(&strip, MONITOR_DEFAULTTONULL) };
        if mon.is_invalid() {
            return None;
        }
        let mi = monitor_info(mon)?;
        let (mut dx, mut dy) = (0u32, 0u32);
        let dpi = if unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) }.is_ok() {
            dx
        } else {
            saved.dpi
        };
        let w = mi.rcWork;
        let p = saved.fit((w.left - ox, w.top - oy, w.right - ox, w.bottom - oy), dpi);
        let mut wp = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        unsafe { GetWindowPlacement(hwnd, &mut wp) }.ok()?;
        wp.showCmd = SW_HIDE.0 as u32;
        wp.flags = Default::default();
        wp.rcNormalPosition = RECT {
            left: p.x,
            top: p.y,
            right: p.x + p.w,
            bottom: p.y + p.h,
        };
        // Moving onto a monitor with another DPI makes the window scale itself on the way;
        // a second time puts it at the size asked for, now that it is on that monitor.
        for _ in 0..2 {
            unsafe { SetWindowPlacement(hwnd, &wp) }.ok()?;
            if placement(hwnd)
                .is_some_and(|now| (now.x, now.y, now.w, now.h) == (p.x, p.y, p.w, p.h))
            {
                break;
            }
        }
        Some(p)
    }
}

#[cfg(not(windows))]
mod imp {
    //! Elsewhere (dev builds only) nothing is remembered.
    use super::{Placement, Stored};

    pub struct Reg;
    impl Reg {
        pub fn open(_key: &str) -> Option<Reg> {
            None
        }
        pub fn open_rw(_key: &str) -> Option<Reg> {
            None
        }
        pub fn create(_key: &str) -> Result<Reg, String> {
            Err("settings are only kept on Windows".into())
        }
        pub fn get(&self, _name: &str) -> Option<Stored> {
            None
        }
        pub fn set(&self, _name: &str, _v: &Stored) -> Result<(), String> {
            Ok(())
        }
        pub fn delete(&self, _name: &str) -> Result<(), String> {
            Ok(())
        }
        pub fn is_empty(&self) -> bool {
            true
        }
        pub fn all(&self) -> Vec<(String, Stored)> {
            Vec::new()
        }
    }
    pub fn delete_key(_key: &str) -> Result<(), String> {
        Ok(())
    }
    #[derive(Clone, Copy)]
    pub struct Hwnd;
    pub fn hwnd(_window: &tauri::WebviewWindow) -> Option<Hwnd> {
        None
    }
    pub fn placement(_hwnd: Hwnd) -> Option<Placement> {
        None
    }
    pub fn place(_hwnd: Hwnd, _saved: &Placement) -> Option<Placement> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_round_trips() {
        let p = Placement {
            x: -1910,
            y: 40,
            w: 1600,
            h: 1000,
            dpi: 144,
            max: true,
        };
        assert_eq!(p.format(), "-1910,40,1600,1000,144,max");
        assert_eq!(Placement::parse(&p.format()), Some(p));
        let q = Placement { max: false, ..p };
        assert_eq!(Placement::parse(&q.format()), Some(q));
    }

    #[test]
    fn odd_placements_are_ignored() {
        for s in [
            "",
            "1,2,3",
            "0,0,1280,820",
            "0,0,1280,820,96,min",
            "0,0,1280,820,96,max,1",
            "0,0,10,10,96",
            "0,0,1280,820,0",
            "a,0,1280,820,96",
        ] {
            assert_eq!(Placement::parse(s), None, "{s}");
        }
    }

    #[test]
    fn fit_scales_with_dpi_and_stays_on_the_work_area() {
        let p = Placement {
            x: 100,
            y: 50,
            w: 1200,
            h: 800,
            dpi: 96,
            max: false,
        };
        // same DPI, fits: unchanged
        assert_eq!(p.fit((0, 0, 1920, 1040), 96), p);
        // 150 %: one and a half times the size, same corner
        let q = p.fit((0, 0, 3840, 2100), 144);
        assert_eq!((q.x, q.y, q.w, q.h, q.dpi), (100, 50, 1800, 1200, 144));
        // a smaller screen: shrunk to it and moved inside
        let r = p.fit((0, 0, 1024, 728), 96);
        assert_eq!((r.x, r.y, r.w, r.h), (0, 0, 1024, 728));
        // sticking out on the right of a second monitor: moved back in
        let s = Placement { x: 3000, ..p }.fit((1920, 0, 3840, 1040), 96);
        assert_eq!((s.x, s.y), (3840 - 1200, 50));
    }

    #[test]
    fn names() {
        assert!(valid_name("SizeMode"));
        assert!(valid_name("DetailsPane2"));
        for n in [
            "",
            "Window",
            "window",
            "a b",
            "Size-Mode",
            "ü",
            &"x".repeat(33),
        ] {
            assert!(!valid_name(n), "{n}");
        }
    }

    #[test]
    fn values_from_json() {
        assert_eq!(Stored::from_json(&Value::from(true)), Ok(Stored::Dword(1)));
        assert_eq!(Stored::from_json(&Value::from(0)), Ok(Stored::Dword(0)));
        assert_eq!(
            Stored::from_json(&Value::from("logical")),
            Ok(Stored::Text("logical".into()))
        );
        assert!(Stored::from_json(&Value::from(-1)).is_err());
        assert!(Stored::from_json(&Value::from(1.5)).is_err());
        assert!(Stored::from_json(&serde_json::json!([1])).is_err());
    }

    /// The real registry, under a key of the test's own (Windows only).
    #[cfg(windows)]
    #[test]
    fn registry_keeps_only_what_is_saved() {
        let key = format!(r"Software\Tessera-test-{}", std::process::id());
        let _ = imp::delete_key(&key);
        assert!(load(&key).is_empty());
        save(&key, "SizeMode", &Value::from("logical")).unwrap();
        save(&key, "DetailsPane", &Value::from(0)).unwrap();
        let m = load(&key);
        assert_eq!(m.get("SizeMode"), Some(&Value::from("logical")));
        assert_eq!(m.get("DetailsPane"), Some(&Value::from(0)));
        assert!(save(&key, "Window", &Value::from("x")).is_err());
        // back to the defaults: the values go, and with the last one the key
        save(&key, "SizeMode", &Value::Null).unwrap();
        assert!(imp::Reg::open(&key).is_some());
        save(&key, "DetailsPane", &Value::Null).unwrap();
        assert!(imp::Reg::open(&key).is_none());
        // clearing a key that is not there is fine
        save(&key, "SizeMode", &Value::from("logical")).unwrap();
        imp::delete_key(&key).unwrap();
        imp::delete_key(&key).unwrap();
        assert!(load(&key).is_empty());
    }
}
