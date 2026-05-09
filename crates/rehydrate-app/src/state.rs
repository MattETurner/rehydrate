use std::path::PathBuf;
use std::sync::Arc;

use rehydrate_core::Library;
use rehydrate_device::ssh::SshDevice;
use rehydrate_device::DeviceInfo;
use rehydrate_ocr::{ModelStore, OcrBackend};
use tokio::sync::{Mutex, RwLock};

/// Shared application state. Held in `tauri::State<AppState>` and accessed
/// from async command handlers; we use tokio::Mutex everywhere so locks can
/// be held across .await points.
///
/// The `library` is held as `Arc<Library>` rather than `Library` directly
/// so command handlers can clone the Arc out of the mutex briefly and
/// then run long operations against `&*library` without keeping any other
/// command blocked. Library itself is `Sync`, so `&Library` is `Send` and
/// can cross `.await` points.
pub struct AppState {
    pub library: Mutex<Option<Arc<Library>>>,
    pub library_path: Mutex<Option<PathBuf>>,
    pub device: Mutex<Option<Arc<SshDevice>>>,
    pub device_info: RwLock<Option<DeviceInfo>>,
    pub device_reachable: RwLock<bool>,
    /// Active OCR backend. Defaults to a Mock that surfaces "model
    /// not loaded" — the real `MistralRsBackend` is wired in after
    /// the user downloads the GGUF on first OCR.
    pub ocr_backend: RwLock<Arc<dyn OcrBackend>>,
    /// Disk-backed store of downloaded models. Located under the
    /// platform's data dir so it survives across library switches.
    pub model_store: ModelStore,
}

impl AppState {
    pub fn new() -> Self {
        let model_root = ocr_models_dir();
        Self {
            library: Mutex::new(None),
            library_path: Mutex::new(None),
            device: Mutex::new(None),
            device_info: RwLock::new(None),
            device_reachable: RwLock::new(false),
            ocr_backend: RwLock::new(Arc::new(rehydrate_ocr::Mock {
                canned: Vec::new(),
            })),
            model_store: ModelStore::new(model_root),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Default cross-platform library location: `<user docs>/reHydrate` on
/// platforms that have a documents dir, falling back to `<home>/reHydrate`.
pub fn default_library_dir() -> Option<PathBuf> {
    if let Some(dirs) = directories::UserDirs::new() {
        if let Some(docs) = dirs.document_dir() {
            return Some(docs.join("reHydrate"));
        }
    }
    directories::BaseDirs::new().map(|b| b.home_dir().join("reHydrate"))
}

pub const KEYRING_SERVICE: &str = "reHydrate";
pub const KEYRING_DEVICE_USER: &str = "remarkable-usb-password";

/// Keychain entry for Ghost publish credentials. Stores a JSON blob
/// (`{"base_url":..., "admin_api_key":...}`) so the URL travels with
/// the secret; one entry instead of two reduces UI / forget logic.
pub const KEYRING_GHOST_CREDS: &str = "publish-ghost";
/// Keychain entry for WordPress publish credentials. Stores a JSON
/// blob (`{"base_url":..., "username":..., "application_password":...}`).
pub const KEYRING_WORDPRESS_CREDS: &str = "publish-wordpress";

/// Where downloaded VLM weights live. Cross-platform via
/// `directories::ProjectDirs`, falling back to `~/.rehydrate/models`
/// if the platform doesn't expose a data dir (rare).
pub fn ocr_models_dir() -> PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("app", "rehydrate", "reHydrate") {
        return dirs.data_dir().join("models");
    }
    if let Some(base) = directories::BaseDirs::new() {
        return base.home_dir().join(".rehydrate").join("models");
    }
    PathBuf::from("./rehydrate-models")
}
