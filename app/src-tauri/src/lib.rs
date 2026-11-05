//! Tessera's window and the IPC surface the WebView talks to (docs/ipc.md).
//!
//! [`backend`] serves the commands and events from `tessera-core`: the whole
//! file tree stays here, the WebView only receives the rects it draws.
//! [`platform`] covers the window itself (Mica, accent colour, elevation).

mod backend;
mod platform;
mod volumes;

use tauri::Manager;

#[tauri::command]
fn platform_info(state: tauri::State<'_, platform::Info>) -> platform::Info {
    state.inner().clone()
}

#[tauri::command]
fn relaunch_elevated(app: tauri::AppHandle) -> Result<(), String> {
    platform::relaunch_elevated()?;
    app.exit(0);
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .manage(backend::Core::new())
        .setup(|app| {
            let window = app
                .get_webview_window("main")
                .ok_or("main window missing")?;
            let backdrop = platform::apply_backdrop(&window);
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tessera");
}
