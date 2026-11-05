//! Tessera's window and the IPC surface the WebView talks to (docs/ipc.md).
//!
//! The scan core (crate `tessera-core`) is wired in separately; until then
//! `platform_info` reports `core: false` and the WebView uses its built-in mock
//! tree, while everything about the window itself (Mica, accent colour,
//! elevation) is already real.

mod platform;

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
        .setup(|app| {
            let window = app
                .get_webview_window("main")
                .ok_or("main window missing")?;
            let backdrop = platform::apply_backdrop(&window);
            app.manage(platform::Info::detect(backdrop, app.package_info().version.to_string()));
            window.show()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![platform_info, relaunch_elevated])
        .run(tauri::generate_context!())
        .expect("error while running Tessera");
}
