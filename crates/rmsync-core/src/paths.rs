use std::path::{Path, PathBuf};

use crate::hash::Sha256Hex;

/// All on-disk locations inside a library directory.
#[derive(Debug, Clone)]
pub struct LibraryPaths {
    pub root: PathBuf,
    pub blobs: PathBuf,
    pub tmp: PathBuf,
    pub db: PathBuf,
    pub logs: PathBuf,
    pub library_json: PathBuf,
}

impl LibraryPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            blobs: root.join("blobs"),
            tmp: root.join("tmp"),
            db: root.join("db.sqlite"),
            logs: root.join("logs"),
            library_json: root.join("library.json"),
            root,
        }
    }

    pub fn blob_path(&self, hash: &Sha256Hex) -> PathBuf {
        let (a, b) = hash.fanout();
        self.blobs.join(a).join(b).join(hash.as_str())
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for dir in [&self.blobs, &self.tmp, &self.logs] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
