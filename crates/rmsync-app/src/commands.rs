use std::path::PathBuf;
use std::sync::Arc;

use rmsync_core::{
    DocumentSummary, FolderEntry, GarbageCollectReport, ImportKind, Library, VerifyReport,
    VersionEntry,
};
use rmsync_device::ssh::{is_reachable, SshConfig, SshDevice};
use rmsync_device::{Device, DeviceInfo};
use rmsync_sync::{
    execute_pull, execute_push, plan_pull, plan_push, progress, Cancel, ProgressEvent, PullPlan,
    PushPlan,
};
use secrecy::SecretString;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::config;
use crate::logging;
use crate::state::{default_library_dir, AppState, KEYRING_DEVICE_USER, KEYRING_SERVICE};

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
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
    let lib = Library::open(&path).map_err(err)?;
    *state.library.lock().await = Some(lib);
    *state.library_path.lock().await = Some(path.clone());
    // Persist for next launch. Best-effort — if the config dir isn't
    // writable we still succeed at opening the library this session.
    let mut cfg = config::load();
    cfg.library_path = Some(path);
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
        let _ = config::save(&cfg);
        return Ok(None);
    }
    let lib = Library::open(&path).map_err(err)?;
    *state.library.lock().await = Some(lib);
    *state.library_path.lock().await = Some(path.clone());
    Ok(Some(path))
}

/// Import a PDF or EPUB from disk into the library. The kind is inferred
/// from the file extension; `visible_name` defaults to the filename stem.
#[tauri::command]
pub async fn import_file(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, String> {
    let guard = state.library.lock().await;
    let lib = guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "file has no extension".to_string())?;
    let kind = ImportKind::from_extension(ext).ok_or_else(|| {
        format!("unsupported file type: .{ext} — only PDF and EPUB are supported")
    })?;
    let visible_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string();
    lib.import_file(&path, kind, &visible_name).map_err(err)
}

#[tauri::command]
pub async fn garbage_collect(state: State<'_, AppState>) -> Result<GarbageCollectReport, String> {
    let guard = state.library.lock().await;
    let lib = guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;
    lib.garbage_collect().map_err(err)
}

#[tauri::command]
pub async fn verify_library(state: State<'_, AppState>) -> Result<VerifyReport, String> {
    let guard = state.library.lock().await;
    let lib = guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;
    lib.verify().map_err(err)
}

#[tauri::command]
pub async fn library_summary(state: State<'_, AppState>) -> Result<LibrarySummary, String> {
    let guard = state.library.lock().await;
    let path_guard = state.library_path.lock().await;
    let lib = guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;
    let path = path_guard
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
    let guard = state.library.lock().await;
    let lib = guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;
    lib.list_folders().map_err(err)
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
    use rmsync_core::Manifest;

    let guard = state.library.lock().await;
    let lib = guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;
    let docs = lib.list_documents().map_err(err)?;
    let doc = docs
        .iter()
        .find(|d| d.document_id == document_id)
        .ok_or_else(|| format!("document {document_id} not in library"))?;
    let manifest_bytes = lib.read_blob(&doc.current_manifest).map_err(err)?;
    let manifest = Manifest::from_canonical_json(&manifest_bytes).map_err(err)?;

    let cache_root = directories::ProjectDirs::from("app", "marginalia", "Marginalia")
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
        let ext = body.path.rsplit('.').next().unwrap_or("bin");
        let p = cache_root.join(format!(
            "{safe_name}-{}.{ext}",
            &body.sha256.as_str()[..12]
        ));
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
        const PREVIEW_LAYOUT_VERSION: &str = "ink-v8";
        let p = cache_root.join(format!(
            "{safe_name}-{}-{PREVIEW_LAYOUT_VERSION}.pdf",
            &doc.current_manifest.as_str()[..12]
        ));
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
                crate::notebook_pdf::build_pdf_from_rm_files(&doc.visible_name, &bufs)
                    .or_else(|e| {
                        // .rm parse failed (older v3/v5 format we don't
                        // render, or corrupt page) — fall through to the
                        // thumbnail fallback so the user still sees
                        // something.
                        tracing::warn!(
                            "ink rendering failed for {}: {e}; falling back to thumbnails",
                            doc.document_id
                        );
                        thumbnail_fallback_pdf(lib, &manifest, &doc.visible_name)
                    })?
            } else {
                thumbnail_fallback_pdf(lib, &manifest, &doc.visible_name)?
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
    let guard = state.library.lock().await;
    let lib = guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;
    lib.list_documents().map_err(err)
}

#[tauri::command]
pub async fn get_history(
    document_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<VersionEntry>, String> {
    let guard = state.library.lock().await;
    let lib = guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;
    lib.get_history(&document_id).map_err(err)
}

#[tauri::command]
pub async fn set_version_note(
    version_id: i64,
    note: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let guard = state.library.lock().await;
    let lib = guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;
    lib.set_version_note(version_id, note.as_deref())
        .map_err(err)
}

#[derive(Serialize)]
pub struct ExportResult {
    pub path: PathBuf,
    pub file_count: usize,
}

/// Export a version's file tree under `dest_dir`, into a freshly-created
/// subdirectory named like `<visible_name>-v<version_id>-<observed_at>`.
/// Returns the full path that was written and the number of files in it.
#[tauri::command]
pub async fn export_version(
    version_id: i64,
    dest_dir: PathBuf,
    state: State<'_, AppState>,
) -> Result<ExportResult, String> {
    let guard = state.library.lock().await;
    let lib = guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;

    let entry = lib.get_version(version_id).map_err(err)?;
    let manifest_bytes = lib.read_blob(&entry.manifest_hash).map_err(err)?;
    let manifest = rmsync_core::Manifest::from_canonical_json(&manifest_bytes).map_err(err)?;

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

    Ok(ExportResult {
        path: target,
        file_count: manifest.files.len(),
    })
}

fn thumbnail_fallback_pdf(
    lib: &Library,
    manifest: &rmsync_core::Manifest,
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
    let secret = match password {
        Some(p) => {
            // Save first so a failed connect (e.g. wrong password) doesn't
            // forget the user's typing — we re-throw the error and the user
            // can retry without re-entering. If they want to clear it, they
            // call forget_device_password.
            let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_DEVICE_USER).map_err(err)?;
            entry.set_password(&p).map_err(err)?;
            SecretString::from(p)
        }
        None => {
            let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_DEVICE_USER).map_err(err)?;
            let p = entry
                .get_password()
                .map_err(|e| format!("no password stored ({e}); pass one to connect_device"))?;
            SecretString::from(p)
        }
    };

    let dev = SshDevice::connect(cfg, secret).await.map_err(err)?;
    let info = dev.ping().await.map_err(err)?;
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
    let guard = state.library.lock().await;
    let lib = guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;
    let outcome = lib.restore_version(version_id).map_err(err)?;
    Ok(outcome.version_id)
}

#[tauri::command]
pub async fn pull_plan(state: State<'_, AppState>) -> Result<PullPlan, String> {
    let dev = state
        .device
        .lock()
        .await
        .as_ref()
        .map(Arc::clone)
        .ok_or_else(|| "device not connected".to_string())?;
    let lib_guard = state.library.lock().await;
    let lib = lib_guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;
    plan_pull(lib, dev.as_ref()).await.map_err(err)
}

#[tauri::command]
pub async fn pull_execute(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SyncReportOut, String> {
    let dev = state
        .device
        .lock()
        .await
        .as_ref()
        .map(Arc::clone)
        .ok_or_else(|| "device not connected".to_string())?;
    let lib_guard = state.library.lock().await;
    let lib = lib_guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;

    let plan = plan_pull(lib, dev.as_ref()).await.map_err(err)?;

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

    let report = execute_pull(lib, dev.as_ref(), plan, Some(tx), Cancel::default())
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
    let lib_guard = state.library.lock().await;
    let lib = lib_guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;
    plan_push(lib).map_err(err)
}

#[tauri::command]
pub async fn push_execute(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PushReportOut, String> {
    let dev = state
        .device
        .lock()
        .await
        .as_ref()
        .map(Arc::clone)
        .ok_or_else(|| "device not connected".to_string())?;
    let lib_guard = state.library.lock().await;
    let lib = lib_guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;

    let plan = plan_push(lib).map_err(err)?;
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
    let report = execute_push(lib, dev.as_ref(), plan, Some(tx), Cancel::default())
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
    let dev = state
        .device
        .lock()
        .await
        .as_ref()
        .map(Arc::clone)
        .ok_or_else(|| "device not connected".to_string())?;
    let lib_guard = state.library.lock().await;
    let lib = lib_guard
        .as_ref()
        .ok_or_else(|| "no library is open".to_string())?;

    // ----- PULL phase -----
    let _ = app.emit("sync:phase", "pull");
    let pull_plan = plan_pull(lib, dev.as_ref()).await.map_err(err)?;
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
    let pull = execute_pull(lib, dev.as_ref(), pull_plan, Some(tx), Cancel::default())
        .await
        .map_err(err)?;
    let _ = forwarder.await;

    // ----- PUSH phase -----
    let _ = app.emit("sync:phase", "push");
    let push_plan = plan_push(lib).map_err(err)?;
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
    let push = execute_push(lib, dev.as_ref(), push_plan, Some(tx), Cancel::default())
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
        .name("rmsync-reachability".into())
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
