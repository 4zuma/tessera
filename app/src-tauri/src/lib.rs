//! Tessera's window and the IPC surface the WebView talks to (docs/ipc.md).
//!
//! [`backend`] serves the commands and events from `tessera-core`: the whole
//! file tree stays here, the WebView only receives the rects it draws.
//! [`platform`] covers the window itself (Mica, accent colour, elevation),
//! [`shell_menu`] Windows' own context menu for items, [`i18n`] the UI language
//! and the text Rust produces, [`settings`] what is remembered between runs.

mod backend;
mod i18n;
mod memlog;
mod platform;
mod settings;
mod shell_menu;
mod volumes;

use tauri::Manager;

#[tauri::command]
fn platform_info(state: tauri::State<'_, platform::Info>) -> platform::Info {
    state.inner().clone()
}

#[tauri::command]
fn relaunch_elevated(app: tauri::AppHandle) -> Result<(), String> {
    // the new window opens where this one is (app.exit below sends no CloseRequested)
    if let Some(w) = app.get_webview_window("main") {
        settings::save_window(&w, &app.state::<settings::WindowMemory>());
    }
    platform::relaunch_elevated()?;
    app.exit(0);
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .manage(backend::Core::new())
        .manage(settings::WindowMemory::default())
        .setup(|app| {
            let (window, backdrop) = platform::build_main_window(app)?;
            // still hidden: it shows up where it was last closed
            settings::restore_window(&window, &app.state::<settings::WindowMemory>());
            // the language, before anything produces text (here COM is already set up
            // on this thread, as the WebView needs it)
            i18n::choice();
            platform::trim_when_minimized(&window);
            memlog::start(app.handle());
            app.manage(platform::Info::detect(
                backdrop,
                app.package_info().version.to_string(),
            ));
            #[cfg(windows)]
            if let Ok(hwnd) = window.hwnd() {
                app.state::<backend::Core>()
                    .owner_hwnd
                    .store(hwnd.0 as isize, std::sync::atomic::Ordering::Relaxed);
            }
            window.show()?;
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { .. } => {
                if let Some(w) = window.app_handle().get_webview_window(window.label()) {
                    settings::save_window(
                        &w,
                        &window.app_handle().state::<settings::WindowMemory>(),
                    );
                }
            }
            tauri::WindowEvent::Focused(f) => {
                backend::window_state(window.app_handle(), Some(*f), None)
            }
            tauri::WindowEvent::Resized(_) => {
                let minimized = window.is_minimized().unwrap_or(false);
                backend::window_state(window.app_handle(), None, Some(minimized))
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            platform_info,
            relaunch_elevated,
            backend::list_volumes,
            backend::scan_start,
            backend::scan_cancel,
            backend::layout,
            backend::details,
            backend::nav,
            backend::cat_stats,
            backend::recycle,
            backend::restore,
            backend::purge,
            backend::empty_recycle_bin,
            backend::reveal,
            backend::shell_menu,
            settings::settings_load,
            settings::settings_save,
            settings::settings_clear,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tessera");
}
