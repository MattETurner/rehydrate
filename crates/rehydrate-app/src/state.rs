use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use rehydrate_core::Library;
use rehydrate_device::ssh::SshDevice;
use rehydrate_device::DeviceInfo;
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
    /// Cached result of the most recent Ollama reachability probe.
    /// `transcribe_document` consults this before making a real
    /// request; cache TTL is `OLLAMA_PING_TTL` (30 s) so we don't
    /// re-probe on every OCR action while the user is mid-batch.
    pub last_ollama_ping: RwLock<Option<OllamaPing>>,
}

#[derive(Debug, Clone)]
pub struct OllamaPing {
    pub at: Instant,
    pub base_url: String,
    pub ok: bool,
}

/// How long a successful `ping_ollama` result is trusted before a
/// new probe is needed. Short enough that the user starting Ollama
/// after a failed transcribe isn't stuck waiting for the cache to
/// expire; long enough that batched OCR over many docs reuses one
/// probe.
pub const OLLAMA_PING_TTL: std::time::Duration = std::time::Duration::from_secs(30);

impl AppState {
    pub fn new() -> Self {
        Self {
            library: Mutex::new(None),
            library_path: Mutex::new(None),
            device: Mutex::new(None),
            device_info: RwLock::new(None),
            device_reachable: RwLock::new(false),
            last_ollama_ping: RwLock::new(None),
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
/// Keychain slot for Ghost admin API credentials. JSON-encoded
/// `rehydrate_publish::GhostCredentials`.
pub const KEYRING_GHOST_CREDS: &str = "ghost-admin-credentials";
/// Keychain slot for WordPress credentials. JSON-encoded
/// `rehydrate_publish::WordpressCredentials`.
pub const KEYRING_WORDPRESS_CREDS: &str = "wordpress-application-password";
