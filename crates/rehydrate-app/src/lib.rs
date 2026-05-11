//! Tauri binary entry point. Holds AppState and exposes commands. No
//! business logic — every command delegates to the rehydrate-* crates.

mod commands;
mod config;
mod logging;
mod notebook_pdf;
mod ocr_commands;
mod state;

pub use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init();
    tracing::info!("rehydrate starting; log dir = {:?}", logging::log_dir());

    // Audit fix M6: tauri-plugin-shell was registered but never used
    // from Rust; the renderer's `plugin:shell|open` IPC was a free
    // surface for opening arbitrary http/tel/mailto URLs. The opener
    // plugin handles the legitimate "open the document I just
    // exported" path on its own.
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
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
            commands::switch_library,
            commands::pick_library_directory,
            commands::list_recent_libraries,
            commands::library_summary,
            commands::list_documents,
            commands::list_folders,
            commands::list_archived,
            commands::move_document,
            commands::rename_document,
            commands::rename_folder,
            commands::create_folder,
            commands::reorder_folder,
            commands::archive_document,
            commands::unarchive_document,
            commands::purge_archived_document,
            commands::open_document,
            commands::document_thumbnail,
            commands::get_history,
            commands::set_version_note,
            commands::export_version,
            commands::verify_library,
            commands::import_file,
            commands::import_dropped_file,
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
            // OCR + CMS — Phase 1.0 OCR uses an Ollama daemon the
            // user runs themselves; the Settings modal lets them
            // pick base URL + model.
            ocr_commands::ocr_status,
            ocr_commands::transcribe_document,
            ocr_commands::get_transcript,
            ocr_commands::export_transcript,
            ocr_commands::get_ollama_config,
            ocr_commands::save_ollama_config,
            ocr_commands::ping_ollama,
            ocr_commands::list_curated_ollama_models,
            ocr_commands::default_ollama_model,
            ocr_commands::list_documents_needing_ocr,
            ocr_commands::publish_transcript,
            ocr_commands::publish_credential_status,
            ocr_commands::ping_publish_target,
            ocr_commands::set_ghost_credentials,
            ocr_commands::forget_ghost_credentials,
            ocr_commands::set_wordpress_credentials,
            ocr_commands::forget_wordpress_credentials,
        ])
        .run(tauri::generate_context!())
        .expect("error while running rehydrate");
}
