use std::path::PathBuf;
use std::sync::Arc;

use rmsync_core::{DocumentSummary, Library, VersionEntry};
use rmsync_device::ssh::{is_reachable, SshConfig, SshDevice};
use rmsync_device::{Device, DeviceInfo};
use rmsync_sync::{execute_pull, plan_pull, progress, Cancel, ProgressEvent, PullPlan};
use secrecy::SecretString;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{default_library_dir, AppState, KEYRING_DEVICE_USER, KEYRING_SERVICE};

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

// ---------- Library ---------------------------------------------------------

#[tauri::command]
pub async fn open_library(path: PathBuf, state: State<'_, AppState>) -> Result<(), String> {
    let lib = Library::open(&path).map_err(err)?;
    *state.library.lock().await = Some(lib);
    *state.library_path.lock().await = Some(path);
    Ok(())
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
