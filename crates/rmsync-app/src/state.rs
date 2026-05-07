use std::path::PathBuf;
use std::sync::Mutex;

use rmsync_core::Library;

pub struct AppState {
    pub library: Mutex<Option<Library>>,
    pub library_path: Mutex<Option<PathBuf>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            library: Mutex::new(None),
            library_path: Mutex::new(None),
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
