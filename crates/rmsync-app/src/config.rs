//! Persistent app config. A tiny JSON file in the platform's config dir
//! that remembers the last-opened library so the user doesn't have to
//! click "Open library" on every launch.
//!
//! macOS:   ~/Library/Application Support/Marginalia/config.json
//! Linux:   ~/.config/marginalia/config.json
//! Windows: %APPDATA%\Marginalia\config.json

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub library_path: Option<PathBuf>,
}

fn config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("app", "marginalia", "Marginalia")
        .map(|d| d.config_dir().to_path_buf())
}

fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.json"))
}

pub fn load() -> AppConfig {
    let Some(path) = config_path() else {
        return AppConfig::default();
    };
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

pub fn save(cfg: &AppConfig) -> Result<(), std::io::Error> {
    let Some(dir) = config_dir() else {
        return Err(std::io::Error::other("no config dir on this platform"));
    };
    fs::create_dir_all(&dir)?;
    let path = dir.join("config.json");
    let bytes = serde_json::to_vec_pretty(cfg).map_err(|e| std::io::Error::other(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}
