//! Windows integration: Mica backdrop, system accent colour, elevation, WebView2 memory
//! while minimized.

use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Accent {
    /// accent for light theme surfaces (AccentDark1, as WinUI uses it)
    pub light: String,
    /// accent for dark theme surfaces (AccentLight2)
    pub dark: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub elevated: bool,
    /// "mica" when the window got the Mica backdrop, "none" otherwise
    pub backdrop: &'static str,
    pub accent: Option<Accent>,
    pub version: String,
    /// the scan core serves the commands; false (non-Windows dev builds) means
    /// the WebView uses its mock tree
    pub core: bool,
}

impl Info {
    pub fn detect(backdrop: &'static str, version: String) -> Self {
        Info {
            elevated: imp::is_elevated(),
            backdrop,
            accent: imp::accent(),
            version,
            core: cfg!(windows),
        }
    }
}

/// Mica needs Windows 11 (build 22000+); elsewhere the WebView paints its own base.
pub fn apply_backdrop(window: &tauri::WebviewWindow) -> &'static str {
    use tauri::window::{Effect, EffectsBuilder};
    if !imp::supports_mica() {
        return "none";
    }
    match window.set_effects(EffectsBuilder::new().effect(Effect::Mica).build()) {
        Ok(()) => "mica",
        Err(_) => "none",
    }
}

pub fn relaunch_elevated() -> Result<(), String> {
    imp::relaunch_elevated()
}

/// While the window is minimized, WebView2 is asked to keep its memory low: its processes
/// collect garbage and drop caches, and Windows may page out what is left. Back to normal
/// as soon as the window shows again.
pub fn trim_when_minimized(window: &tauri::WebviewWindow) {
    use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
    let w = window.clone();
    let minimized = AtomicBool::new(false);
    window.on_window_event(move |event| {
        if !matches!(event, tauri::WindowEvent::Resized(_)) {
            return;
        }
        let now = w.is_minimized().unwrap_or(false);
        if minimized.swap(now, Relaxed) != now {
            imp::low_memory(&w, now);
        }
    });
}

#[cfg(windows)]
mod imp {
    use super::Accent;
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, Interface, PCWSTR, w};

    pub fn supports_mica() -> bool {
        windows_version::OsVersion::current().build >= 22000
    }

    pub fn is_elevated() -> bool {
        unsafe {
            let mut token = HANDLE::default();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
                return false;
            }
            let mut elev = TOKEN_ELEVATION::default();
            let mut len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elev as *mut _ as *mut _),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
            .is_ok();
            let _ = CloseHandle(token);
            ok && elev.TokenIsElevated != 0
        }
    }

    pub fn accent() -> Option<Accent> {
        let ui = UISettings::new().ok()?;
        let hex = |t: UIColorType| -> Option<String> {
            let c = ui.GetColorValue(t).ok()?;
            Some(format!("#{:02x}{:02x}{:02x}", c.R, c.G, c.B))
        };
        Some(Accent {
            light: hex(UIColorType::AccentDark1)?,
            dark: hex(UIColorType::AccentLight2)?,
        })
    }

    pub fn low_memory(window: &tauri::WebviewWindow, low: bool) {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL, ICoreWebView2_19,
        };
        let level = if low {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
        } else {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
        };
        let _ = window.with_webview(move |wv| unsafe {
            // ICoreWebView2_19 needs WebView2 Runtime 1.0.1774 or newer; older ones skip this
            if let Ok(core) = wv.controller().CoreWebView2()
                && let Ok(core) = core.cast::<ICoreWebView2_19>()
            {
                let _ = core.SetMemoryUsageTargetLevel(level);
            }
        });
    }

    pub fn relaunch_elevated() -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = HSTRING::from(exe.as_os_str());
        let r = unsafe {
            ShellExecuteW(
                None,
                w!("runas"),
                &exe,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        // values above 32 mean success; the user may also have declined the UAC prompt
        if r.0 as isize > 32 {
            Ok(())
        } else {
            Err("没有以管理员身份重启（可能在 UAC 提示里点了“否”）".into())
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Accent;
    pub fn supports_mica() -> bool {
        false
    }
    pub fn is_elevated() -> bool {
        false
    }
    pub fn accent() -> Option<Accent> {
        None
    }
    pub fn relaunch_elevated() -> Result<(), String> {
        Err("only supported on Windows".into())
    }
    pub fn low_memory(_window: &tauri::WebviewWindow, _low: bool) {}
}
