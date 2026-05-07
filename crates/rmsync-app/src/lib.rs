//! Tauri binary entry point. Holds AppState and exposes commands. No
//! business logic — every command delegates to the rmsync-* crates.

mod commands;
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
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::default_library_path,
            commands::open_library,
            commands::library_summary,
            commands::list_documents,
            commands::get_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running marginalia");
}
