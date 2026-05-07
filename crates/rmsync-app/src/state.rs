use std::path::PathBuf;
use std::sync::Arc;

use rmsync_core::Library;
use rmsync_device::ssh::SshDevice;
use rmsync_device::DeviceInfo;
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
}

impl AppState {
    pub fn new() -> Self {
        Self {
            library: Mutex::new(None),
            library_path: Mutex::new(None),
            device: Mutex::new(None),
            device_info: RwLock::new(None),
            device_reachable: RwLock::new(false),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Default cross-platform library location: `<user docs>/Marginalia` on
/// platforms that have a documents dir, falling back to `<home>/Marginalia`.
pub fn default_library_dir() -> Option<PathBuf> {
    if let Some(dirs) = directories::UserDirs::new() {
        if let Some(docs) = dirs.document_dir() {
            return Some(docs.join("Marginalia"));
        }
    }
    directories::BaseDirs::new().map(|b| b.home_dir().join("Marginalia"))
}

pub const KEYRING_SERVICE: &str = "Marginalia";
pub const KEYRING_DEVICE_USER: &str = "remarkable-usb-password";
