//! Desktop shell: a native window showing the web app from `build/web`.
//!
//! Everything (emulation, storage, input, audio) runs inside the webview exactly
//! as in the browser; this binary only provides the window and the installer.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running Pipit");
}
