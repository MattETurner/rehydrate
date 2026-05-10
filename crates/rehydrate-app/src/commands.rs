use std::path::PathBuf;
use std::sync::Arc;

use rehydrate_core::{
    ArchiveReason, ArchivedDocument, DocumentSummary, FolderEntry, GarbageCollectReport,
    ImportKind, Library, VerifyReport, VersionEntry,
};
use rehydrate_device::ssh::{is_reachable, SshConfig, SshDevice};
use rehydrate_device::{Device, DeviceInfo};
use rehydrate_sync::{
    execute_pull, execute_push, plan_pull, plan_push, progress, Cancel, ProgressEvent, PullPlan,
    PushPlan,
};
use secrecy::SecretString;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::config;
use crate::logging;
use crate::state::{default_library_dir, AppState, KEYRING_DEVICE_USER, KEYRING_SERVICE};

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Clone the `Arc<Library>` out of the mutex briefly and drop the guard.
/// The returned Arc keeps the Library alive; long operations against the
/// library can then run without holding the AppState mutex, so other
/// commands aren't blocked while a sync or open_document is in flight.
async fn lib_arc(state: &State<'_, AppState>) -> Result<Arc<Library>, String> {
    state
        .library
        .lock()
        .await
        .as_ref()
        .map(Arc::clone)
        .ok_or_else(|| "no library is open".to_string())
}

#[tauri::command]
pub fn ping() -> &'static str {
    "pong"
}

#[derive(Serialize)]
pub struct LogTail {
    pub lines: Vec<String>,
    pub log_dir: Option<PathBuf>,
}

#[tauri::command]
pub async fn get_recent_logs(max_lines: Option<usize>) -> Result<LogTail, String> {
    let n = max_lines.unwrap_or(500);
    let lines = logging::read_tail(n).map_err(err)?;
    Ok(LogTail {
        lines,
        log_dir: logging::log_dir(),
    })
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

#[derive(Serialize)]
pub struct DeviceState {
    pub reachable: bool,
    pub connected: bool,
    pub info: Option<DeviceInfo>,
    /// True if a password is stored in the OS keychain — the UI uses this
    /// to decide whether to show a password prompt or just a Connect button.
    pub has_stored_password: bool,
}

#[derive(Serialize)]
pub struct SyncReportOut {
    pub recorded: usize,
    pub unchanged: usize,
    pub skipped: usize,
}

#[derive(Serialize)]
pub struct PushReportOut {
    pub pushed: usize,
    pub unchanged: usize,
    pub skipped: usize,
}

#[derive(Serialize)]
pub struct TwoWayReport {
    pub pull: SyncReportOut,
    pub push: PushReportOut,
}

// ---------- Library ---------------------------------------------------------

#[tauri::command]
pub async fn open_library(path: PathBuf, state: State<'_, AppState>) -> Result<(), String> {
    open_library_at(&path, &state).await
}

/// Common path-validated open used by `open_library`, `switch_library`,
/// and `switch_library_via_dialog`. Drops the previously-held library
/// (and its OS lock) before opening the new one, so the same process
/// can move between per-device libraries without restarting.
async fn open_library_at(
    path: &std::path::Path,
    state: &State<'_, AppState>,
) -> Result<(), String> {
    // Drop the existing Library first so its `.lock` is released. If
    // the user is switching to the SAME library, this avoids
    // `AlreadyOpen` when we re-open it below.
    {
        let mut slot = state.library.lock().await;
        *slot = None;
    }
    let lib = Library::open(path).map_err(err)?;
    *state.library.lock().await = Some(Arc::new(lib));
    *state.library_path.lock().await = Some(path.to_path_buf());

    // Persist for next launch + push to recents. Best-effort.
    let mut cfg = config::load();
    cfg.record_open(path.to_path_buf());
    if let Err(e) = config::save(&cfg) {
        tracing::warn!("failed to persist app config: {e}");
    }
    Ok(())
}

/// On launch the UI calls this to restore the previously-opened library
/// without forcing the user through the welcome screen each time. Returns
/// the path that was opened, or `None` if there was no valid prior library.
#[tauri::command]
pub async fn auto_open_library(state: State<'_, AppState>) -> Result<Option<PathBuf>, String> {
    let mut cfg = config::load();
    let Some(path) = cfg.library_path.clone() else {
        return Ok(None);
    };
    if !path.exists() {
        // Stale config — drop the entry silently so the user sees the
        // welcome screen again rather than a confusing error.
        cfg.library_path = None;
        cfg.recent_libraries.retain(|r| r.path != path);
        let _ = config::save(&cfg);
        return Ok(None);
    }
    open_library_at(&path, &state).await?;
    Ok(Some(path))
}

/// Switch to a previously-opened library by path. The path must
/// already be in the recents list (so the user has consciously opened
/// it before via the picker). Returns the path opened, or an error if
/// the library is no longer there or its stamp is invalid.
#[tauri::command]
pub async fn switch_library(path: PathBuf, state: State<'_, AppState>) -> Result<PathBuf, String> {
    let cfg = config::load();
    if !cfg.recent_libraries.iter().any(|r| r.path == path) {
        return Err(format!(
            "{} is not in the recent libraries list; use 'Open another library…' to add it",
            path.display()
        ));
    }
    open_library_at(&path, &state).await?;
    Ok(path)
}

/// Open the OS folder picker so the user can pick a library
/// directory, then probe the chosen path. Returns:
/// - `None` if the user cancelled the dialog;
/// - `Some({ path, kind: "existing" })` if the path is already a
///   stamped library — the renderer should call `open_library` to
///   open it without further confirmation;
/// - `Some({ path, kind: "empty" })` if the path is an empty (or
///   dotfile-only) directory — the renderer should ask the user
///   "Create a new library here?" before calling `open_library`.
///
/// Foreign-but-non-empty directories surface as `Err`. We never
/// open the library here so a confirm prompt can sit between the
/// pick and the side-effecting open.
#[tauri::command]
pub async fn pick_library_directory(
    app: AppHandle,
) -> Result<Option<PickedLibraryDirectory>, String> {
    let app_for_pick = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app_for_pick
            .dialog()
            .file()
            .set_title("Open a reHydrate library folder")
            .blocking_pick_folder()
    })
    .await
    .map_err(err)?;

    let Some(file_path) = picked else {
        return Ok(None);
    };
    let path = file_path
        .into_path()
        .map_err(|e| format!("could not resolve picked folder: {e}"))?;

    // Probe synchronously — no lock acquired, no library.json
    // written — so the renderer's confirm prompt sits between this
    // and the actual `open_library`.
    let kind = match Library::probe_path(&path) {
        Ok(rehydrate_core::LibraryPathKind::Empty) => PickedLibraryKind::Empty,
        Ok(rehydrate_core::LibraryPathKind::Existing) => PickedLibraryKind::Existing,
        Err(e) => return Err(err(e)),
    };
    Ok(Some(PickedLibraryDirectory { path, kind }))
}

#[derive(Serialize)]
pub struct PickedLibraryDirectory {
    pub path: PathBuf,
    pub kind: PickedLibraryKind,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PickedLibraryKind {
    Empty,
    Existing,
}

#[derive(Serialize)]
pub struct RecentLibraryEntry {
    pub path: PathBuf,
    pub label: String,
    pub last_opened: String,
    /// True if `path` still exists and looks like a library on disk.
    /// The UI uses this to grey out stale entries.
    pub available: bool,
    /// True if this is the currently-open library.
    pub current: bool,
}

#[tauri::command]
pub async fn list_recent_libraries(
    state: State<'_, AppState>,
) -> Result<Vec<RecentLibraryEntry>, String> {
    let cfg = config::load();
    let active = state.library_path.lock().await.clone();
    Ok(cfg
        .recent_libraries
        .into_iter()
        .map(|r| {
            let available = r.path.is_dir() && r.path.join("library.json").is_file();
            let current = active.as_ref() == Some(&r.path);
            RecentLibraryEntry {
                path: r.path,
                label: r.label,
                last_opened: r.last_opened,
                available,
                current,
            }
        })
        .collect())
}

/// Import a PDF or EPUB from disk into the library.
///
/// Audit fix H6: the OS file picker runs server-side here; the
/// renderer can no longer hand us a path of its choosing (e.g. a
/// symlink `evil.pdf → ~/.ssh/id_rsa`). Returns `None` if the user
/// cancelled the dialog. We also sniff the magic bytes after picking
/// so a renamed-but-not-actually-PDF/EPUB is rejected early.
#[tauri::command]
pub async fn import_file(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<DocumentSummary>, String> {
    let lib = lib_arc(&state).await?;

    // The dialog is a blocking OS call; run it on a worker thread so
    // we don't tie up the Tauri main thread.
    let app_for_pick = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app_for_pick
            .dialog()
            .file()
            .add_filter("Documents", &["pdf", "epub"])
            .set_title("Import a PDF or EPUB")
            .blocking_pick_file()
    })
    .await
    .map_err(err)?;

    let Some(file_path) = picked else {
        return Ok(None);
    };
    let path = file_path
        .into_path()
        .map_err(|e| format!("could not resolve picked path: {e}"))?;

    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "file has no extension".to_string())?;
    let kind = ImportKind::from_extension(ext).ok_or_else(|| {
        format!("unsupported file type: .{ext} — only PDF and EPUB are supported")
    })?;

    // Magic-byte sniff: refuse a "*.pdf" symlink that actually points
    // at, say, an SSH private key. PDF starts with "%PDF-", EPUB is a
    // ZIP ("PK\x03\x04").
    let mut head = [0u8; 5];
    {
        use std::io::Read;
        let mut f = std::fs::File::open(&path).map_err(err)?;
        let _ = f.read(&mut head).map_err(err)?;
    }
    let looks_pdf = head.starts_with(b"%PDF-");
    let looks_epub = head.starts_with(b"PK\x03\x04");
    let extension_kind_ok = match kind {
        ImportKind::Pdf => looks_pdf,
        ImportKind::Epub => looks_epub,
    };
    if !extension_kind_ok {
        return Err(format!(
            "{} does not look like a {} file (header check failed)",
            path.display(),
            ext.to_uppercase()
        ));
    }

    let visible_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string();
    lib.import_file(&path, kind, &visible_name)
        .map(Some)
        .map_err(err)
}

#[tauri::command]
pub async fn garbage_collect(state: State<'_, AppState>) -> Result<GarbageCollectReport, String> {
    let lib = lib_arc(&state).await?;
    lib.garbage_collect().map_err(err)
}

#[tauri::command]
pub async fn verify_library(state: State<'_, AppState>) -> Result<VerifyReport, String> {
    let lib = lib_arc(&state).await?;
    lib.verify().map_err(err)
}

#[tauri::command]
pub async fn library_summary(state: State<'_, AppState>) -> Result<LibrarySummary, String> {
    let lib = lib_arc(&state).await?;
    let path = state
        .library_path
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no library is open".to_string())?;
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
pub async fn list_folders(state: State<'_, AppState>) -> Result<Vec<FolderEntry>, String> {
    let lib = lib_arc(&state).await?;
    lib.list_folders().map_err(err)
}

/// Return a base64 data-URL for the document's first-page thumbnail, or
/// `None` if the manifest has no `.thumbnails/*.png` page (rare for
/// pulled docs; possible for fresh imports that haven't been synced
/// yet). The data-URL form lets the webview show the PNG without a
/// custom asset-protocol capability — and thumbnails are typically
/// 5–50 KB so the IPC payload stays small.
#[tauri::command]
pub async fn document_thumbnail(
    document_id: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use rehydrate_core::Manifest;

    let lib = lib_arc(&state).await?;
    let docs = lib.list_documents().map_err(err)?;
    let doc = docs
        .iter()
        .find(|d| d.document_id == document_id)
        .ok_or_else(|| format!("document {document_id} not in library"))?;
    let manifest_bytes = lib.read_blob(&doc.current_manifest).map_err(err)?;
    let manifest = Manifest::from_canonical_json(&manifest_bytes).map_err(err)?;

    let mut thumbs: Vec<_> = manifest
        .files
        .iter()
        .filter(|f| f.path.ends_with(".png") && f.path.contains(".thumbnails"))
        .collect();
    if thumbs.is_empty() {
        return Ok(None);
    }
    thumbs.sort_by(|a, b| a.path.cmp(&b.path));
    let bytes = lib.read_blob(&thumbs[0].sha256).map_err(err)?;
    Ok(Some(format!(
        "data:image/png;base64,{}",
        STANDARD.encode(&bytes)
    )))
}

/// Materialise a document's content into a cache directory and open it
/// with the OS's default viewer. PDFs and EPUBs are written as-is.
/// Notebooks (no PDF/EPUB body file) get assembled into a single
/// multi-page PDF from the device's per-page thumbnail PNGs — a preview,
/// not faithful ink rendering.
#[tauri::command]
pub async fn open_document(
    document_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PathBuf, String> {
    use rehydrate_core::Manifest;

    let lib = lib_arc(&state).await?;
    let docs = lib.list_documents().map_err(err)?;
    let doc = docs
        .iter()
        .find(|d| d.document_id == document_id)
        .ok_or_else(|| format!("document {document_id} not in library"))?;
    let manifest_bytes = lib.read_blob(&doc.current_manifest).map_err(err)?;
    let manifest = Manifest::from_canonical_json(&manifest_bytes).map_err(err)?;

    let cache_root = directories::ProjectDirs::from("app", "rehydrate", "reHydrate")
        .map(|d| d.cache_dir().to_path_buf())
        .ok_or_else(|| "no cache directory on this platform".to_string())?
        .join("open");
    std::fs::create_dir_all(&cache_root).map_err(err)?;
    let safe_name = sanitize(&doc.visible_name);

    // Resolve body: full PDF/EPUB, otherwise stitch thumbnails to a PDF.
    let cache_path = if let Some(body) = manifest
        .files
        .iter()
        .find(|f| f.path.ends_with(".pdf") || f.path.ends_with(".epub"))
    {
        let bytes = lib.read_blob(&body.sha256).map_err(err)?;
        // Audit fix H8: explicitly allow-list the cache extension to
        // {pdf, epub}. The previous `body.path.rsplit('.').next()`
        // accepted any tail — a manifest with `body.path = "x.command"`
        // landed a `.command` file that LaunchServices would then
        // execute. Manifest::validate_paths blocks `..`/absolutes but
        // doesn't restrict extensions.
        let ext = if body.path.ends_with(".pdf") {
            "pdf"
        } else {
            "epub"
        };
        // Sha256Hex deserialization is now strict (audit H4) so the
        // hash is always 64 chars; .get(..12) defends against any
        // future relaxation.
        let prefix = body
            .sha256
            .as_str()
            .get(..12)
            .unwrap_or(body.sha256.as_str());
        let p = cache_root.join(format!("{safe_name}-{prefix}.{ext}"));
        if !p.exists() {
            std::fs::write(&p, &bytes).map_err(err)?;
        }
        p
    } else {
        // Notebook. Two render paths:
        //   1) `.rm` ink files → vector PDF (sharp at any zoom).
        //   2) Fallback: stitch per-page thumbnail PNGs (low-fidelity
        //      preview, used only if a page has no parseable ink data).
        // Cache key includes a layout version suffix so bumping the
        // assembly logic invalidates stale previews automatically.
        const PREVIEW_LAYOUT_VERSION: &str = "ink-v15";
        // Defensive `.get(..12)` so we can never panic on a hex-string
        // that for some reason is shorter than expected (audit H4 made
        // this impossible at the type level, but the slice was a UI
        // panic vector before that fix).
        let manifest_hex = doc.current_manifest.as_str();
        let prefix = manifest_hex.get(..12).unwrap_or(manifest_hex);
        let p = cache_root.join(format!("{safe_name}-{prefix}-{PREVIEW_LAYOUT_VERSION}.pdf"));
        if !p.exists() {
            let mut rm_pages: Vec<_> = manifest
                .files
                .iter()
                .filter(|f| {
                    f.path.ends_with(".rm")
                        && !f.path.contains(".thumbnails")
                        && !f.path.ends_with(".local")
                })
                .collect();
            rm_pages.sort_by(|a, b| a.path.cmp(&b.path));

            let pdf_bytes = if !rm_pages.is_empty() {
                let mut bufs = Vec::with_capacity(rm_pages.len());
                for f in &rm_pages {
                    bufs.push(lib.read_blob(&f.sha256).map_err(err)?);
                }
                crate::notebook_pdf::build_pdf_from_rm_files(&doc.visible_name, &bufs).or_else(
                    |e| {
                        // .rm parse failed (older v3/v5 format we don't
                        // render, or corrupt page) — fall through to the
                        // thumbnail fallback so the user still sees
                        // something.
                        tracing::warn!(
                            "ink rendering failed for {}: {e}; falling back to thumbnails",
                            doc.document_id
                        );
                        thumbnail_fallback_pdf(&lib, &manifest, &doc.visible_name)
                    },
                )?
            } else {
                thumbnail_fallback_pdf(&lib, &manifest, &doc.visible_name)?
            };
            std::fs::write(&p, &pdf_bytes).map_err(err)?;
        }
        p
    };

    app.opener()
        .open_path(cache_path.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("could not open {}: {e}", cache_path.display()))?;
    Ok(cache_path)
}

#[tauri::command]
pub async fn list_documents(state: State<'_, AppState>) -> Result<Vec<DocumentSummary>, String> {
    let lib = lib_arc(&state).await?;
    lib.list_documents().map_err(err)
}

#[tauri::command]
pub async fn list_archived(state: State<'_, AppState>) -> Result<Vec<ArchivedDocument>, String> {
    let lib = lib_arc(&state).await?;
    lib.list_archived().map_err(err)
}

/// Rename a live document. Writes the new title into `.metadata`'s
/// `visibleName` and records a new version, so the tablet picks up
/// the rename on the next push.
#[tauri::command]
pub async fn rename_document(
    document_id: String,
    new_name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let lib = lib_arc(&state).await?;
    lib.rename_document(&document_id, &new_name).map_err(err)?;
    Ok(())
}

/// Rename a folder. Updates the local row and flags it for push so
/// the next sync uploads the new metadata to the tablet.
#[tauri::command]
pub async fn rename_folder(
    folder_id: String,
    new_name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let lib = lib_arc(&state).await?;
    lib.rename_folder(&folder_id, &new_name).map_err(err)?;
    Ok(())
}

/// Move a live document into a different folder (or to root if
/// `parentId` is None). Records a new version so the change propagates to
/// the device on the next push.
#[tauri::command]
pub async fn move_document(
    document_id: String,
    parent_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let lib = lib_arc(&state).await?;
    lib.move_document(&document_id, parent_id.as_deref())
        .map_err(err)?;
    Ok(())
}

/// Soft-delete a live document — moves it to the archive with reason
/// "local". Versions are kept so it can be restored.
#[tauri::command]
pub async fn archive_document(
    document_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let lib = lib_arc(&state).await?;
    lib.archive_document(&document_id, ArchiveReason::Local)
        .map_err(err)
}

/// Restore an archived document to the live listing.
#[tauri::command]
pub async fn unarchive_document(
    document_id: String,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, String> {
    let lib = lib_arc(&state).await?;
    lib.unarchive_document(&document_id).map_err(err)
}

/// Permanently delete an archived document. Drops the version log so blobs
/// become orphans; run garbage_collect to reclaim disk.
#[tauri::command]
pub async fn purge_archived_document(
    document_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let lib = lib_arc(&state).await?;
    lib.purge_archived_document(&document_id).map_err(err)
}

#[tauri::command]
pub async fn get_history(
    document_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<VersionEntry>, String> {
    let lib = lib_arc(&state).await?;
    lib.get_history(&document_id).map_err(err)
}

#[tauri::command]
pub async fn set_version_note(
    version_id: i64,
    note: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let lib = lib_arc(&state).await?;
    lib.set_version_note(version_id, note.as_deref())
        .map_err(err)
}

#[derive(Serialize)]
pub struct ExportResult {
    pub path: PathBuf,
    pub file_count: usize,
}

/// Export a version's file tree under a user-picked directory, into
/// a freshly-created subdirectory named like
/// `<visible_name>-v<version_id>-<observed_at>`. Returns the full path
/// that was written and the number of files in it, or `None` if the
/// user cancelled the directory picker.
///
/// Audit fix H7: the destination is chosen via a server-side folder
/// picker so the renderer can't direct writes into
/// `~/Library/LaunchAgents` or similar.
#[tauri::command]
pub async fn export_version(
    app: AppHandle,
    version_id: i64,
    state: State<'_, AppState>,
) -> Result<Option<ExportResult>, String> {
    let lib = lib_arc(&state).await?;

    let entry = lib.get_version(version_id).map_err(err)?;
    let manifest_bytes = lib.read_blob(&entry.manifest_hash).map_err(err)?;
    let manifest = rehydrate_core::Manifest::from_canonical_json(&manifest_bytes).map_err(err)?;

    let app_for_pick = app.clone();
    let title = format!("Export \"{}\" v{} to…", manifest.visible_name, version_id);
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app_for_pick
            .dialog()
            .file()
            .set_title(&title)
            .blocking_pick_folder()
    })
    .await
    .map_err(err)?;

    let Some(dest_path) = picked else {
        return Ok(None);
    };
    let dest_dir = dest_path
        .into_path()
        .map_err(|e| format!("could not resolve picked folder: {e}"))?;

    let safe_name = sanitize(&manifest.visible_name);
    let safe_ts = entry
        .observed_at
        .replace([':', 'T'], "-")
        .trim_end_matches('Z')
        .to_string();
    let dir_name = format!("{safe_name}-v{version_id}-{safe_ts}");
    let target = dest_dir.join(&dir_name);

    if target.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite",
            target.display()
        ));
    }

    lib.reconstruct(version_id, &target).map_err(err)?;

    Ok(Some(ExportResult {
        path: target,
        file_count: manifest.files.len(),
    }))
}

fn thumbnail_fallback_pdf(
    lib: &Library,
    manifest: &rehydrate_core::Manifest,
    title: &str,
) -> Result<Vec<u8>, String> {
    let mut thumbs: Vec<_> = manifest
        .files
        .iter()
        .filter(|f| f.path.ends_with(".png") && f.path.contains(".thumbnails"))
        .collect();
    if thumbs.is_empty() {
        return Err(
            "notebook has no .rm ink files we can parse and no thumbnails to fall back on \
             — sync the device once (or open and edit the notebook on the tablet first) \
             and try again"
                .to_string(),
        );
    }
    thumbs.sort_by(|a, b| a.path.cmp(&b.path));
    let mut pages = Vec::with_capacity(thumbs.len());
    for f in &thumbs {
        pages.push(lib.read_blob(&f.sha256).map_err(err)?);
    }
    crate::notebook_pdf::build_pdf_from_pngs(title, &pages)
}

fn sanitize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
            out.push(c);
        } else if c == ' ' {
            out.push('-');
        }
    }
    if out.is_empty() {
        "Untitled".into()
    } else {
        out
    }
}

// ---------- Device ----------------------------------------------------------

#[tauri::command]
pub async fn device_state(state: State<'_, AppState>) -> Result<DeviceState, String> {
    Ok(DeviceState {
        reachable: *state.device_reachable.read().await,
        connected: state.device.lock().await.is_some(),
        info: state.device_info.read().await.clone(),
        has_stored_password: keyring::Entry::new(KEYRING_SERVICE, KEYRING_DEVICE_USER)
            .ok()
            .and_then(|e| e.get_password().ok())
            .is_some(),
    })
}

/// Save a device password into the OS keychain. Does not connect.
#[tauri::command]
pub async fn save_device_password(password: String) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_DEVICE_USER).map_err(err)?;
    entry.set_password(&password).map_err(err)
}

#[tauri::command]
pub async fn forget_device_password() -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_DEVICE_USER).map_err(err)?;
    // Ignore "not found" since the operation is logically idempotent.
    let _ = entry.delete_credential();
    Ok(())
}

/// Open an SSH session against the tablet. If `password` is `None`, reads
/// from the keychain. If a password is supplied, also persists it to the
/// keychain on success.
#[tauri::command]
pub async fn connect_device(
    password: Option<String>,
    state: State<'_, AppState>,
) -> Result<DeviceInfo, String> {
    let cfg = SshConfig::default();
    // Resolve the password without persisting yet. Persisting before we
    // know the password is correct means a typo gets cached and the next
    // connect attempt silently uses the bad value.
    //
    // Audit fix M4: a parallel un-zeroized `Option<String>` shadow used
    // to defeat SecretString's zero-on-drop. We now carry only the
    // SecretString and a flag — the keychain write reads the bytes
    // back out via expose_secret().
    let (secret, freshly_typed) = match password {
        Some(p) => (SecretString::from(p), true),
        None => {
            let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_DEVICE_USER).map_err(err)?;
            let stored = entry
                .get_password()
                .map_err(|e| format!("no password stored ({e}); pass one to connect_device"))?;
            (SecretString::from(stored), false)
        }
    };

    let dev = SshDevice::connect(cfg, secret.clone()).await.map_err(err)?;
    let info = dev.ping().await.map_err(err)?;

    // Connection succeeded — only NOW persist a freshly-typed password.
    if freshly_typed {
        use secrecy::ExposeSecret;
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_DEVICE_USER) {
            if let Err(e) = entry.set_password(secret.expose_secret()) {
                tracing::warn!("could not persist device password to keychain: {e}");
            }
        }
    }

    *state.device.lock().await = Some(Arc::new(dev));
    *state.device_info.write().await = Some(info.clone());
    Ok(info)
}

#[tauri::command]
pub async fn disconnect_device(state: State<'_, AppState>) -> Result<(), String> {
    *state.device.lock().await = None;
    *state.device_info.write().await = None;
    Ok(())
}

// ---------- Sync ------------------------------------------------------------

#[tauri::command]
pub async fn restore_version(version_id: i64, state: State<'_, AppState>) -> Result<i64, String> {
    let lib = lib_arc(&state).await?;
    let outcome = lib.restore_version(version_id).map_err(err)?;
    Ok(outcome.version_id)
}

async fn device_arc(state: &State<'_, AppState>) -> Result<Arc<SshDevice>, String> {
    state
        .device
        .lock()
        .await
        .as_ref()
        .map(Arc::clone)
        .ok_or_else(|| "device not connected".to_string())
}

#[tauri::command]
pub async fn pull_plan(state: State<'_, AppState>) -> Result<PullPlan, String> {
    let dev = device_arc(&state).await?;
    let lib = lib_arc(&state).await?;
    plan_pull(&lib, dev.as_ref()).await.map_err(err)
}

#[tauri::command]
pub async fn pull_execute(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SyncReportOut, String> {
    let dev = device_arc(&state).await?;
    let lib = lib_arc(&state).await?;

    let plan = plan_pull(&lib, dev.as_ref()).await.map_err(err)?;

    let (tx, mut rx) = progress::channel(64);
    let app_for_task = app.clone();
    let forwarder = tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = app_for_task.emit("sync:progress", &ev);
            if matches!(ev, ProgressEvent::Done { .. } | ProgressEvent::Cancelled) {
                break;
            }
        }
    });

    let report = execute_pull(&lib, dev.as_ref(), plan, Some(tx), Cancel::default())
        .await
        .map_err(err)?;
    let _ = forwarder.await;

    Ok(SyncReportOut {
        recorded: report.recorded,
        unchanged: report.unchanged,
        skipped: report.skipped,
    })
}

#[tauri::command]
pub async fn push_plan(state: State<'_, AppState>) -> Result<PushPlan, String> {
    let lib = lib_arc(&state).await?;
    plan_push(&lib).map_err(err)
}

#[tauri::command]
pub async fn push_execute(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PushReportOut, String> {
    let dev = device_arc(&state).await?;
    let lib = lib_arc(&state).await?;

    let plan = plan_push(&lib).map_err(err)?;
    let (tx, mut rx) = progress::channel(64);
    let app_for_task = app.clone();
    let forwarder = tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = app_for_task.emit("sync:progress", &ev);
            if matches!(ev, ProgressEvent::Done { .. } | ProgressEvent::Cancelled) {
                break;
            }
        }
    });
    let report = execute_push(&lib, dev.as_ref(), plan, Some(tx), Cancel::default())
        .await
        .map_err(err)?;
    let _ = forwarder.await;

    Ok(PushReportOut {
        pushed: report.pushed,
        unchanged: report.unchanged,
        skipped: report.skipped,
    })
}

/// Pull-then-push. The pull-first ordering means device-side changes are
/// captured before any library-side change overwrites them; the loser of any
/// conflict is preserved in the version log via parent_version_id and
/// remains restorable from the history view.
#[tauri::command]
pub async fn sync_two_way(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TwoWayReport, String> {
    let dev = device_arc(&state).await?;
    let lib = lib_arc(&state).await?;

    // ----- PULL phase -----
    let _ = app.emit("sync:phase", "pull");
    let pull_plan = plan_pull(&lib, dev.as_ref()).await.map_err(err)?;
    let (tx, mut rx) = progress::channel(64);
    let app_for_task = app.clone();
    let forwarder = tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = app_for_task.emit("sync:progress", &ev);
            if matches!(ev, ProgressEvent::Done { .. } | ProgressEvent::Cancelled) {
                break;
            }
        }
    });
    let pull = execute_pull(&lib, dev.as_ref(), pull_plan, Some(tx), Cancel::default())
        .await
        .map_err(err)?;
    let _ = forwarder.await;

    // ----- PUSH phase -----
    let _ = app.emit("sync:phase", "push");
    let push_plan = plan_push(&lib).map_err(err)?;
    let (tx, mut rx) = progress::channel(64);
    let app_for_task = app.clone();
    let forwarder = tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = app_for_task.emit("sync:progress", &ev);
            if matches!(ev, ProgressEvent::Done { .. } | ProgressEvent::Cancelled) {
                break;
            }
        }
    });
    let push = execute_push(&lib, dev.as_ref(), push_plan, Some(tx), Cancel::default())
        .await
        .map_err(err)?;
    let _ = forwarder.await;

    Ok(TwoWayReport {
        pull: SyncReportOut {
            recorded: pull.recorded,
            unchanged: pull.unchanged,
            skipped: pull.skipped,
        },
        push: PushReportOut {
            pushed: push.pushed,
            unchanged: push.unchanged,
            skipped: push.skipped,
        },
    })
}

// ---------- Internals -------------------------------------------------------

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

/// Background task started at app launch. Polls the USB-ethernet endpoint
/// every 2s and emits `device:reachable` events on changes. Cheap and
/// platform-agnostic — no USB driver hooks needed.
pub fn spawn_reachability_watcher(app: AppHandle) {
    // Run on a dedicated OS thread with its own tokio current-thread
    // runtime. Doing this from the Tauri `setup` callback fails because
    // neither tokio nor `tauri::async_runtime` has its reactor live yet
    // — they spin up later in Builder::run(). A standalone thread sidesteps
    // the ordering question entirely. The work is tiny (one TCP connect
    // every 2s), so a private runtime is fine.
    std::thread::Builder::new()
        .name("rehydrate-reachability".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("failed to start reachability watcher runtime: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                let mut last: Option<bool> = None;
                loop {
                    let now = is_reachable("10.11.99.1", 22).await;
                    if last != Some(now) {
                        let state = app.state::<AppState>();
                        *state.device_reachable.write().await = now;
                        let _ = app.emit("device:reachable", now);
                        last = Some(now);
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            });
        })
        .expect("spawn reachability watcher thread");
}
