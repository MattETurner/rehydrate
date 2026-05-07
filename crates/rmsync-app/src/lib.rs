//! Tauri binary entry point. Holds AppState and exposes commands. No
//! business logic — every command delegates to the rmsync-* crates.

mod commands;
mod config;
mod logging;
mod notebook_pdf;
mod state;

pub use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init();
    tracing::info!("marginalia starting; log dir = {:?}", logging::log_dir());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::new())
        .setup(|app| {
            commands::spawn_reachability_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::get_recent_logs,
            commands::default_library_path,
            commands::open_library,
            commands::auto_open_library,
            commands::library_summary,
            commands::list_documents,
            commands::list_folders,
            commands::open_document,
            commands::get_history,
            commands::set_version_note,
            commands::export_version,
            commands::verify_library,
            commands::import_file,
            commands::garbage_collect,
            commands::device_state,
            commands::save_device_password,
            commands::forget_device_password,
            commands::connect_device,
            commands::disconnect_device,
            commands::pull_plan,
            commands::pull_execute,
            commands::push_plan,
            commands::push_execute,
            commands::sync_two_way,
            commands::restore_version,
        ])
        .run(tauri::generate_context!())
        .expect("error while running marginalia");
}
