//! Tauri binary entry point. Holds AppState and exposes commands. No
//! business logic — every command delegates to the rmsync-* crates.

mod commands;
mod config;
mod state;

pub use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,rmsync=debug")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::new())
        .setup(|app| {
            commands::spawn_reachability_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::default_library_path,
            commands::open_library,
            commands::auto_open_library,
            commands::library_summary,
            commands::list_documents,
            commands::get_history,
            commands::verify_library,
            commands::device_state,
            commands::save_device_password,
            commands::forget_device_password,
            commands::connect_device,
            commands::disconnect_device,
            commands::pull_plan,
            commands::pull_execute,
        ])
        .run(tauri::generate_context!())
        .expect("error while running marginalia");
}
