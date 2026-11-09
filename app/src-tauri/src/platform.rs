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
    /// UI language (a dictionary id: "en" or "zh-CN"), see [`crate::i18n`]
    pub lang: &'static str,
    /// BCP 47 tag for formatting dates and numbers, e.g. "en-GB"
    pub locale: String,
}

impl Info {
    pub fn detect(backdrop: &'static str, version: String) -> Self {
        Info {
            elevated: imp::is_elevated(),
            backdrop,
            accent: imp::accent(),
            version,
            core: cfg!(windows),
            lang: crate::i18n::lang().id(),
            locale: crate::i18n::choice().locale.clone(),
        }
    }
}

/// Builds the main window from its entry in tauri.conf.json (which has `"create": false`)
/// and gives it its backdrop. Returns the window and the backdrop it got ("mica" or "none").
///
/// Mica needs Windows 11 (build 22000+). There the window is transparent, so Mica shows
/// through the WebView. It also has no redirection bitmap: the WebView draws all of the
/// client area itself, so the window's own copy of it (4 bytes a pixel, 30 MB maximized
/// on a 4K screen) would only hold memory. On Windows 10 a transparent window has nothing behind it (the
/// desktop shows through until the page paints, and in the strip a resize uncovers), so
/// the window is opaque and painted with what WinUI itself shows when Mica is unavailable:
/// the theme's solid base colour. The page paints the same colour (`--solid-base`).
pub fn build_main_window(
    app: &tauri::App,
) -> Result<(tauri::WebviewWindow, &'static str), Box<dyn std::error::Error>> {
    use tauri::window::{Effect, EffectsBuilder};
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .ok_or("main window missing")?;
    let mica = imp::supports_mica();
    let window = tauri::WebviewWindowBuilder::from_config(app.handle(), config)?
        .transparent(mica)
        .no_redirection_bitmap(mica)
        .build()?;
    if !mica {
        // still hidden (`"visible": false`), so the colour is in place before the first frame;
        // the window's theme is the one Windows gave its title bar
        let theme = window.theme().unwrap_or(tauri::Theme::Light);
        let _ = window.set_background_color(Some(solid_base(theme)));
        follow_theme(&window);
        return Ok((window, "none"));
    }
    let backdrop = match window.set_effects(EffectsBuilder::new().effect(Effect::Mica).build()) {
        Ok(()) => "mica",
        Err(_) => "none",
    };
    Ok((window, backdrop))
}

/// WinUI's SolidBackgroundFillColorBase, the colour Mica falls back to. Same values as
/// `solid-base` in app/src/lib/theme.ts.
fn solid_base(theme: tauri::Theme) -> tauri::window::Color {
    if theme == tauri::Theme::Dark {
        tauri::window::Color(0x20, 0x20, 0x20, 0xff)
    } else {
        tauri::window::Color(0xf3, 0xf3, 0xf3, 0xff)
    }
}

/// Keeps an opaque window's own colour on the current theme, so a resize never uncovers
/// the other theme's colour before the page repaints.
fn follow_theme(window: &tauri::WebviewWindow) {
    let w = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::ThemeChanged(theme) = event {
            let _ = w.set_background_color(Some(solid_base(*theme)));
        }
    });
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
            Err(crate::i18n::text::NOT_ELEVATED.get().into())
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
