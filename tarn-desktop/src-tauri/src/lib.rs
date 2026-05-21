mod error;
mod ipc;
mod sidecar;
mod state;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            ipc::discover_tests,
            ipc::run_tests,
            ipc::cancel_run,
            ipc::get_run_report,
            ipc::list_environments,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
