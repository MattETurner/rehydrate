//! Tauri binary entry point. Holds AppState and exposes commands. No
//! business logic — every command delegates to the rmsync-* crates.

mod commands;
mod state;

use std::sync::Arc;

pub use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,rmsync=debug")),
        )
        .init();

    let state = Arc::new(AppState::new());
    let state_for_tauri = state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .manage(state_for_tauri)
        .setup(move |app| {
            commands::spawn_reachability_watcher(app.handle().clone(), state.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::default_library_path,
            commands::open_library,
            commands::library_summary,
            commands::list_documents,
            commands::get_history,
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
