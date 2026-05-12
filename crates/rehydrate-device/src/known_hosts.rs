//! Trust-on-first-use host-key pinning for the device SSH session.
//!
//! When `SshDevice::connect` runs against a host for the first time
//! the server's public-key fingerprint is recorded under
//! `<config_dir>/known_hosts.json`. Every subsequent connect refuses
//! to proceed if the presented key's fingerprint doesn't match —
//! returning [`DeviceError::HostKeyChanged`] so the UI can surface
//! "the device's identity has changed; was it factory-reset?".
//!
//! The shape of the file is a single JSON map `{ "10.11.99.1:22":
//! "SHA256:abc…" }`. JSON because it's trivially editable by the
//! user when they intentionally need to forget a pinned host (firmware
//! reset is the typical case) and because we already depend on
//! `serde_json` everywhere else.
//!
//! Threat model: the reMarkable USB-ethernet endpoint is normally
//! adversary-free, but the previous "accept any key" stub left open
//! the case where another local process binds `10.11.99.1:22` (USB
//! ethernet bridge / VM with custom routing) and impersonates the
//! tablet. The user pastes the device password — an attacker walks
//! away with cleartext SSH credentials. TOFU closes that hole at the
//! one-line cost of pinning the first key.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
struct KnownHostsFile {
    /// Map of `host:port` → `SHA256:<base64-url>` fingerprint.
    #[serde(flatten)]
    entries: BTreeMap<String, String>,
}

/// Storage for known-hosts lookups, persisted as a JSON map under
/// `path`. Designed so the lock is held for the minimum duration —
/// `lookup` and `record` each open the file, mutate, and close.
#[derive(Debug, Clone)]
pub struct KnownHosts {
    path: PathBuf,
}

impl KnownHosts {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Return the previously-pinned fingerprint for `endpoint`, if any.
    pub fn lookup(&self, endpoint: &str) -> io::Result<Option<String>> {
        let file = match fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let parsed: KnownHostsFile = serde_json::from_slice(&file).unwrap_or_default();
        Ok(parsed.entries.get(endpoint).cloned())
    }

    /// Pin `fingerprint` for `endpoint`. Overwrites any existing entry —
    /// callers should only call this *after* verifying that no pinned
    /// fingerprint already exists (otherwise the override silently
    /// papers over an unexpected key change).
    pub fn record(&self, endpoint: &str, fingerprint: &str) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut parsed: KnownHostsFile = match fs::read(&self.path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => KnownHostsFile::default(),
            Err(e) => return Err(e),
        };
        parsed
            .entries
            .insert(endpoint.to_string(), fingerprint.to_string());
        let serialised =
            serde_json::to_vec_pretty(&parsed).map_err(|e| io::Error::other(e.to_string()))?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, &serialised)?;
        fs::rename(&tmp, &self.path)?;
        set_owner_only(&self.path);
        Ok(())
    }
}

/// Remove the OS-default umask bits so the file is owner-readable only.
/// The fingerprint isn't sensitive on its own, but the file's presence
/// reveals which device the user pairs with; clamping permissions is
/// cheap insurance and matches what `~/.ssh/known_hosts` does.
#[cfg(unix)]
fn set_owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_record_then_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let kh = KnownHosts::new(dir.path().join("known_hosts.json"));
        assert!(kh.lookup("10.11.99.1:22").unwrap().is_none());
        kh.record("10.11.99.1:22", "SHA256:abcdef").unwrap();
        assert_eq!(
            kh.lookup("10.11.99.1:22").unwrap().as_deref(),
            Some("SHA256:abcdef")
        );
    }

    #[test]
    fn record_overwrites_existing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let kh = KnownHosts::new(dir.path().join("known_hosts.json"));
        kh.record("a:22", "SHA256:1").unwrap();
        kh.record("a:22", "SHA256:2").unwrap();
        assert_eq!(kh.lookup("a:22").unwrap().as_deref(), Some("SHA256:2"));
    }
}
