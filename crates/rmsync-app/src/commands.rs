use std::path::PathBuf;

use rmsync_core::{DocumentSummary, Library, VersionEntry};
use serde::Serialize;
use tauri::State;

use crate::state::{default_library_dir, AppState};

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[tauri::command]
pub fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
pub fn default_library_path() -> Option<PathBuf> {
    default_library_dir()
}

#[derive(Serialize)]
pub struct LibrarySummary {
    pub path: PathBuf,
    pub document_count: usize,
    pub version_count: i64,
    pub blob_count: usize,
    pub size_bytes: u64,
}

#[tauri::command]
pub fn open_library(path: PathBuf, state: State<'_, AppState>) -> Result<(), String> {
    let lib = Library::open(&path).map_err(err)?;
    *state.library.lock().unwrap() = Some(lib);
    *state.library_path.lock().unwrap() = Some(path);
    Ok(())
}

#[tauri::command]
pub fn library_summary(state: State<'_, AppState>) -> Result<LibrarySummary, String> {
    let guard = state.library.lock().unwrap();
    let path_guard = state.library_path.lock().unwrap();
    let lib = guard.as_ref().ok_or("no library is open")?;
    let path = path_guard.clone().ok_or("no library is open")?;
    let docs = lib.list_documents().map_err(err)?;
    let (blob_count, size_bytes) = blob_stats(&path);
    let version_count = lib.version_count().map_err(err)?;
    Ok(LibrarySummary {
        path,
        document_count: docs.len(),
        version_count,
        blob_count,
        size_bytes,
    })
}

#[tauri::command]
pub fn list_documents(state: State<'_, AppState>) -> Result<Vec<DocumentSummary>, String> {
    let guard = state.library.lock().unwrap();
    let lib = guard.as_ref().ok_or("no library is open")?;
    lib.list_documents().map_err(err)
}

#[tauri::command]
pub fn get_history(
    document_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<VersionEntry>, String> {
    let guard = state.library.lock().unwrap();
    let lib = guard.as_ref().ok_or("no library is open")?;
    lib.get_history(&document_id).map_err(err)
}

fn blob_stats(library_root: &std::path::Path) -> (usize, u64) {
    let blobs = library_root.join("blobs");
    let mut count = 0usize;
    let mut size = 0u64;
    walk(&blobs, &mut |path| {
        if let Ok(meta) = std::fs::metadata(path) {
            count += 1;
            size += meta.len();
        }
    });
    (count, size)
}

fn walk(p: &std::path::Path, f: &mut impl FnMut(&std::path::Path)) {
    let Ok(rd) = std::fs::read_dir(p) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, f);
        } else {
            f(&path);
        }
    }
}
