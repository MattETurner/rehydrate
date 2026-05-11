//! SSH/SFTP transport against the reMarkable USB-ethernet endpoint at
//! `10.11.99.1:22`.
//!
//! Authentication uses the device password the user captures from the tablet
//! UI on first run (Settings → Help → Copyrights and licenses → "GPLv3
//! Compliance" reveals the SSH password). The password is held in memory in
//! a `secrecy::SecretString` and persisted to the OS keychain by the app
//! layer; this module never touches the keychain itself.
//!
//! Phase 1 needs only enumeration and download. The SSH session is held
//! across calls (long-lived TCP connection) and operations are serialised
//! through a `Mutex`. That's plenty fast over USB-ethernet — the bottleneck
//! is the device, not concurrency.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use russh::client::{self, Handle, Handler};
use russh::keys::PublicKey;
use russh::ChannelMsg;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::error::{DeviceError, DeviceResult};
use crate::model::{DeviceInfo, RemoteEntry, RemoteEntryKind, RemoteFile};
use crate::trait_def::Device;

pub const DEFAULT_HOST: &str = "10.11.99.1";
pub const DEFAULT_PORT: u16 = 22;
pub const DEFAULT_USER: &str = "root";
pub const XOCHITL_DIR: &str = "/home/root/.local/share/remarkable/xochitl";

/// Hard cap on a single SFTP read. Real reMarkable documents top out at a
/// few hundred megabytes for very large PDFs; anything past this is either
/// an exotic edge case the user needs to resolve manually, or a malicious /
/// corrupted device serving an oversized file. Without this cap the client
/// would happily allocate gigabytes from a single bad read.
const MAX_REMOTE_FILE_BYTES: u64 = 512 * 1024 * 1024;

/// Bound on directory recursion depth inside `fetch_subtree[_named]`.
/// reMarkable's xochitl tree is essentially flat (one level under the
/// document UUID); 16 leaves enormous headroom while killing any pathological
/// loop that slipped past the symlink filter.
const MAX_SUBTREE_DEPTH: usize = 16;

#[derive(Debug, Clone)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub xochitl_dir: String,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            user: DEFAULT_USER.to_string(),
            xochitl_dir: XOCHITL_DIR.to_string(),
        }
    }
}

/// Minimal russh client handler. Phase 1 accepts any host key on first
/// connect (TOFU is a Phase 5 concern); over USB-ethernet to 10.11.99.1
/// the threat model is "did the user plug the right device in", which is
/// covered by the user physically plugging the device in.
#[derive(Default)]
struct ClientHandler;

impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

pub struct SshDevice {
    cfg: SshConfig,
    inner: Mutex<Inner>,
}

struct Inner {
    handle: Handle<ClientHandler>,
    sftp: SftpSession,
}

impl SshDevice {
    /// Open an SSH session and SFTP subsystem against the configured host.
    pub async fn connect(cfg: SshConfig, password: SecretString) -> DeviceResult<Self> {
        let russh_cfg = Arc::new(client::Config {
            inactivity_timeout: Some(std::time::Duration::from_secs(60)),
            keepalive_interval: Some(std::time::Duration::from_secs(15)),
            ..Default::default()
        });

        let mut handle = client::connect(russh_cfg, (cfg.host.as_str(), cfg.port), ClientHandler)
            .await
            .map_err(|e| DeviceError::Unreachable(format!("{}:{} ({e})", cfg.host, cfg.port)))?;

        let auth_ok = handle
            .authenticate_password(&cfg.user, password.expose_secret())
            .await
            .map_err(|e| DeviceError::Other(format!("authenticate_password: {e}")))?;
        if !auth_ok.success() {
            return Err(DeviceError::AuthFailed);
        }

        let sftp = open_sftp(&handle).await?;

        Ok(Self {
            cfg,
            inner: Mutex::new(Inner { handle, sftp }),
        })
    }

    pub fn config(&self) -> &SshConfig {
        &self.cfg
    }

    /// Run `cmd` and capture stdout. Used for the device-info probe; SFTP is
    /// preferred for everything else.
    async fn exec(&self, cmd: &str) -> DeviceResult<String> {
        let inner = self.inner.lock().await;
        let mut channel = inner
            .handle
            .channel_open_session()
            .await
            .map_err(|e| DeviceError::Other(format!("channel_open_session: {e}")))?;
        channel
            .exec(true, cmd)
            .await
            .map_err(|e| DeviceError::Other(format!("exec: {e}")))?;

        let mut out = Vec::new();
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { ref data } => out.extend_from_slice(data),
                ChannelMsg::ExtendedData { .. } => {}
                ChannelMsg::ExitStatus { .. } => {}
                ChannelMsg::Eof => break,
                _ => {}
            }
        }
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    async fn read_file(&self, sftp: &SftpSession, path: &str) -> DeviceResult<Vec<u8>> {
        let f = sftp
            .open(path)
            .await
            .map_err(|e| sftp_err("open", path, e))?;
        read_capped(f, path).await
    }

    /// Stage a file's bytes to `<path>.rehydrate-tmp`. Does NOT touch the
    /// live `<path>` — that's done in a second phase by `commit_staged`,
    /// after every file in the document has been staged. This split means
    /// a network interruption or write error mid-upload leaves the live
    /// document untouched on the device, instead of half-overwritten.
    async fn stage_file(&self, sftp: &SftpSession, path: &str, bytes: &[u8]) -> DeviceResult<()> {
        if let Some(parent) = path.rsplit_once('/').map(|(p, _)| p) {
            if !parent.is_empty() {
                let _ = sftp.create_dir(parent).await;
            }
        }
        let tmp_path = staged_path(path);
        let _ = sftp.remove_file(&tmp_path).await;

        let flags = OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE;
        let mut file = sftp
            .open_with_flags(&tmp_path, flags)
            .await
            .map_err(|e| sftp_err("open_with_flags", &tmp_path, e))?;
        file.write_all(bytes)
            .await
            .map_err(|e| DeviceError::Io(std::io::Error::other(e.to_string())))?;
        file.shutdown()
            .await
            .map_err(|e| DeviceError::Io(std::io::Error::other(e.to_string())))?;
        drop(file);
        Ok(())
    }

    /// Promote a previously-staged file into place. Uses a backup-rename
    /// pattern because `russh-sftp` 2.1's plain rename refuses to overwrite
    /// (per the SFTP spec) and the SFTP server on the reMarkable ships
    /// without the posix-rename extension we'd otherwise prefer.
    ///
    /// 1. If `<path>` exists, rename it to `<path>.rehydrate-bak`.
    /// 2. Rename `<path>.rehydrate-tmp` → `<path>`.
    /// 3. Remove `<path>.rehydrate-bak`.
    ///
    /// If step 2 fails, restore by renaming the backup back into place so
    /// the device never sees a hole where the file used to be.
    ///
    /// Audit fix H2: pre-existing backups are conditionally cleaned.
    /// If `<path>` is missing but `<path>.rehydrate-bak` exists, a
    /// previous push must have hit a double-fault (step 2 failed AND
    /// the rollback rename also failed) — the only surviving copy of
    /// the user's file is the backup. Recover it instead of wiping it.
    async fn commit_staged(&self, sftp: &SftpSession, path: &str) -> DeviceResult<()> {
        let tmp_path = staged_path(path);
        let bak_path = backup_path(path);

        // Conditional pre-cleanup of any leftover backup.
        let live_existed = sftp.metadata(path).await.is_ok();
        let bak_existed = sftp.metadata(&bak_path).await.is_ok();
        match (live_existed, bak_existed) {
            (true, true) => {
                // Live exists; bak is leftover from a prior cycle that
                // forgot to clean up. Safe to drop.
                let _ = sftp.remove_file(&bak_path).await;
            }
            (false, true) => {
                // Live missing, bak present → recover bak as live
                // before staging. Surfaces a previous double-fault as
                // a recovery action rather than silent data loss.
                tracing::warn!(
                    path,
                    "previous push left {path}.rehydrate-bak with no live copy; recovering"
                );
                sftp.rename(&bak_path, path)
                    .await
                    .map_err(|e| sftp_err("recover(bak→live)", path, e))?;
            }
            _ => {}
        }

        // Re-check live presence after a possible recovery rename.
        let live_existed = sftp.metadata(path).await.is_ok();
        if live_existed {
            sftp.rename(path, &bak_path)
                .await
                .map_err(|e| sftp_err("rename(live→bak)", path, e))?;
        }

        match sftp.rename(&tmp_path, path).await {
            Ok(()) => {
                if live_existed {
                    let _ = sftp.remove_file(&bak_path).await;
                }
                Ok(())
            }
            Err(e) => {
                // Promotion failed. Try to restore the backup. If even
                // the rollback fails, surface a distinct error pointing
                // at the surviving bak so the user — and the next
                // push's recovery branch — can find it.
                if live_existed {
                    if let Err(roll_err) = sftp.rename(&bak_path, path).await {
                        return Err(DeviceError::Other(format!(
                            "promotion failed for {path} ({e}); rollback also failed ({roll_err}); \
                             previous file preserved at {bak_path}"
                        )));
                    }
                }
                Err(sftp_err("rename(tmp→live)", path, e))
            }
        }
    }

    /// Best-effort cleanup of staged tmps for a document — used when a
    /// document upload fails partway through, so we don't leave the
    /// device with `.rehydrate-tmp` litter.
    async fn discard_staged(&self, sftp: &SftpSession, paths: &[String]) {
        for path in paths {
            let _ = sftp.remove_file(&staged_path(path)).await;
        }
    }
}

fn staged_path(path: &str) -> String {
    format!("{path}.rehydrate-tmp")
}

fn backup_path(path: &str) -> String {
    format!("{path}.rehydrate-bak")
}

async fn open_sftp(handle: &Handle<ClientHandler>) -> DeviceResult<SftpSession> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| DeviceError::Other(format!("channel_open_session: {e}")))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| DeviceError::Other(format!("request_subsystem(sftp): {e}")))?;
    let stream = channel.into_stream();
    SftpSession::new(stream)
        .await
        .map_err(|e| DeviceError::Other(format!("SftpSession::new: {e}")))
}

fn sftp_err(op: &str, path: &str, e: russh_sftp::client::error::Error) -> DeviceError {
    let msg = format!("{op} {path}: {e}");
    if msg.contains("NoSuchFile") || msg.contains("ENOENT") {
        DeviceError::NotFound(path.to_string())
    } else {
        DeviceError::Other(msg)
    }
}

#[async_trait]
impl Device for SshDevice {
    async fn ping(&self) -> DeviceResult<DeviceInfo> {
        let model = self
            .exec("cat /sys/devices/soc0/machine 2>/dev/null || echo reMarkable")
            .await?;
        // /proc/device-tree/serial-number is NUL-terminated; trim NULs and ws.
        let serial = self
            .exec("tr -d '\\0' < /proc/device-tree/serial-number 2>/dev/null || true")
            .await
            .ok()
            .map(|s| s.trim().to_string());
        // reMarkable firmware exposes the release in /usr/share/remarkable/update.conf
        // (REMARKABLE_RELEASE_VERSION=...). Fall back to /etc/version if missing.
        let software_version = self
            .exec(
                "(awk -F= '/^REMARKABLE_RELEASE_VERSION/ {print $2}' \
                  /usr/share/remarkable/update.conf 2>/dev/null; \
                  cat /etc/version 2>/dev/null) | head -n1",
            )
            .await
            .ok()
            .map(|s| s.trim().to_string());
        Ok(DeviceInfo {
            model: if model.is_empty() {
                "reMarkable".into()
            } else {
                model
            },
            serial: serial.filter(|s| !s.is_empty()),
            software_version: software_version.filter(|s| !s.is_empty()),
        })
    }

    async fn list_documents(&self) -> DeviceResult<Vec<RemoteEntry>> {
        let inner = self.inner.lock().await;
        let dir = self.cfg.xochitl_dir.clone();
        let entries = inner
            .sftp
            .read_dir(&dir)
            .await
            .map_err(|e| sftp_err("read_dir", &dir, e))?;

        let mut out = Vec::new();
        for entry in entries {
            let name = entry.file_name();
            let Some(uuid) = name.strip_suffix(".metadata") else {
                continue;
            };

            let metadata_path = format!("{dir}/{name}");
            let bytes = self.read_file(&inner.sftp, &metadata_path).await?;
            let metadata: Value = serde_json::from_slice(&bytes)
                .map_err(|e| DeviceError::Protocol(format!("bad metadata json for {uuid}: {e}")))?;
            let visible_name = metadata
                .get("visibleName")
                .and_then(|v| v.as_str())
                .unwrap_or("Untitled")
                .to_string();
            let parent = metadata
                .get("parent")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let device_mtime_hint = metadata
                .get("lastModified")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let type_field = metadata.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let kind = if type_field == "CollectionType" {
                RemoteEntryKind::Folder
            } else {
                RemoteEntryKind::Document
            };

            // Best-effort doc_type from <uuid>.content's fileType.
            let doc_type = if matches!(kind, RemoteEntryKind::Document) {
                let content_path = format!("{dir}/{uuid}.content");
                match self.read_file(&inner.sftp, &content_path).await {
                    Ok(cb) => {
                        let cv: Value = serde_json::from_slice(&cb).unwrap_or(Value::Null);
                        cv.get("fileType")
                            .and_then(|v| v.as_str())
                            .map(|s| match s {
                                "pdf" => "DocumentType.Pdf".to_string(),
                                "epub" => "DocumentType.Epub".to_string(),
                                _ => "Notebook".to_string(),
                            })
                            .unwrap_or_else(|| "Notebook".to_string())
                    }
                    Err(_) => "Notebook".to_string(),
                }
            } else {
                "Folder".to_string()
            };

            out.push(RemoteEntry {
                uuid: uuid.to_string(),
                visible_name,
                doc_type,
                parent,
                kind,
                device_mtime_hint,
                metadata,
            });
        }
        out.sort_by(|a, b| a.uuid.cmp(&b.uuid));
        Ok(out)
    }

    async fn put_document_tree(&self, _uuid: &str, files: &[RemoteFile]) -> DeviceResult<()> {
        let inner = self.inner.lock().await;
        let dir = self.cfg.xochitl_dir.clone();

        // Phase 1: stage every file as `<path>.rehydrate-tmp`. The live
        // document on the device is not touched yet, so an upload failure
        // in this phase leaves it intact.
        let mut targets: Vec<String> = Vec::with_capacity(files.len());
        for f in files {
            let target = format!("{dir}/{}", f.path);
            if let Err(e) = self.stage_file(&inner.sftp, &target, &f.bytes).await {
                self.discard_staged(&inner.sftp, &targets).await;
                return Err(e);
            }
            targets.push(target);
        }

        // Phase 2: promote each staged file into place. Per-file commit
        // uses a backup pattern so a single failed promote can be rolled
        // back. A failure here can leave the document partially-updated;
        // we surface the error so the caller doesn't advance sync_state
        // and the next push will retry the whole tree.
        for target in &targets {
            if let Err(e) = self.commit_staged(&inner.sftp, target).await {
                // Best-effort: try to discard remaining staged files so
                // the device isn't littered with stale .rehydrate-tmp.
                self.discard_staged(&inner.sftp, &targets).await;
                return Err(e);
            }
        }

        drop(inner);

        // Per-document restart was removed: a multi-document push
        // would otherwise restart xochitl N times, blanking the
        // tablet UI for ~3s each. The push engine batches a single
        // `refresh_document_index` call after all docs land.
        Ok(())
    }

    async fn refresh_document_index(&self) -> DeviceResult<()> {
        // The tablet caches the document index in memory; this
        // restart is what makes freshly-pushed files visible on the
        // tablet's UI. ~3s interruption. If it fails we return Err so
        // the push engine can emit a warning event — the files
        // themselves are already safely on the device.
        self.exec("systemctl restart xochitl").await.map(|_| ())
    }

    async fn fetch_document_tree(&self, uuid: &str) -> DeviceResult<Vec<RemoteFile>> {
        let inner = self.inner.lock().await;
        let dir = self.cfg.xochitl_dir.clone();

        let metadata_path = format!("{dir}/{uuid}.metadata");
        // Probe; an open() error on .metadata means "not found".
        let _ = self.read_file(&inner.sftp, &metadata_path).await?;

        let mut out = Vec::new();
        // 1. All sibling files prefixed with `<uuid>.`
        let entries = inner
            .sftp
            .read_dir(&dir)
            .await
            .map_err(|e| sftp_err("read_dir", &dir, e))?;
        for entry in entries {
            let name = entry.file_name();
            if !name.starts_with(&format!("{uuid}.")) {
                continue;
            }
            // Skip subdirs under xochitl/ — handled below.
            if entry.file_type().is_dir() {
                continue;
            }
            // Audit fix H1: a previously-failed push leaves staging
            // tombstones (`*.rehydrate-tmp`, `*.rehydrate-bak`) on
            // the device. Without this filter we'd hash them as if
            // they were real document files, store them in the
            // manifest, and round-trip them back on the next push.
            if name.ends_with(".rehydrate-tmp") || name.ends_with(".rehydrate-bak") {
                continue;
            }
            let path = format!("{dir}/{name}");
            let bytes = self.read_file(&inner.sftp, &path).await?;
            out.push(RemoteFile {
                path: name.clone(),
                bytes,
                mode: 0o644,
            });
        }

        // 2. Optional per-document directory: <xochitl>/<uuid>/...
        let subdir = format!("{dir}/{uuid}");
        match inner.sftp.read_dir(&subdir).await {
            Ok(_) => {
                fetch_subtree(&inner.sftp, &subdir, uuid, &mut out).await?;
            }
            Err(e) => {
                let msg = e.to_string();
                if !msg.contains("NoSuchFile") && !msg.contains("ENOENT") {
                    return Err(sftp_err("read_dir", &subdir, e));
                }
            }
        }

        // 3. Optional thumbnails directory: <xochitl>/<uuid>.thumbnails
        let thumb = format!("{dir}/{uuid}.thumbnails");
        if let Ok(_dir_entries) = inner.sftp.read_dir(&thumb).await {
            fetch_subtree_named(&inner.sftp, &thumb, &format!("{uuid}.thumbnails"), &mut out)
                .await?;
        }

        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}

/// Recursively read everything under `device_dir` into `out`, with on-disk
/// paths rewritten so the output uses `<uuid>/<rel...>` as the path.
async fn fetch_subtree(
    sftp: &SftpSession,
    device_dir: &str,
    uuid: &str,
    out: &mut Vec<RemoteFile>,
) -> DeviceResult<()> {
    fetch_subtree_named(sftp, device_dir, uuid, out).await
}

async fn fetch_subtree_named(
    sftp: &SftpSession,
    device_dir: &str,
    rel_root: &str,
    out: &mut Vec<RemoteFile>,
) -> DeviceResult<()> {
    // (device path, relative path, depth-from-root). Depth is bounded so a
    // pathological tree — or a symlink that the SFTP server does not
    // advertise as such — cannot trap us indefinitely.
    let mut stack: Vec<(String, PathBuf, usize)> =
        vec![(device_dir.to_string(), PathBuf::from(rel_root), 0)];
    while let Some((dev, rel, depth)) = stack.pop() {
        let entries = sftp
            .read_dir(&dev)
            .await
            .map_err(|e| sftp_err("read_dir", &dev, e))?;
        for entry in entries {
            let name = entry.file_name();
            // Skip symlinks: the device reports `xochitl` straight off the
            // stock filesystem and should not contain symlinks under a
            // document tree, so anything that does is either malicious
            // (loop back to `..`) or accidental (some debug helper).
            // Either way we can't safely follow it.
            if entry.file_type().is_symlink() {
                continue;
            }
            let dev_child = format!("{dev}/{name}");
            let rel_child = rel.join(&name);
            if entry.file_type().is_dir() {
                if depth + 1 > MAX_SUBTREE_DEPTH {
                    return Err(DeviceError::Other(format!(
                        "subtree depth exceeds {MAX_SUBTREE_DEPTH} at {dev_child}"
                    )));
                }
                stack.push((dev_child, rel_child, depth + 1));
            } else {
                let bytes = read_path(sftp, &dev_child).await?;
                out.push(RemoteFile {
                    path: rel_child.to_string_lossy().replace('\\', "/"),
                    bytes,
                    mode: 0o644,
                });
            }
        }
    }
    Ok(())
}

async fn read_path(sftp: &SftpSession, path: &str) -> DeviceResult<Vec<u8>> {
    let f = sftp
        .open(path)
        .await
        .map_err(|e| sftp_err("open", path, e))?;
    read_capped(f, path).await
}

/// Read an SFTP file with a hard size limit. `take(MAX_REMOTE_FILE_BYTES + 1)`
/// lets us distinguish "file fits within budget" from "file exceeds budget"
/// without ever allocating beyond the cap.
async fn read_capped<R>(reader: R, path: &str) -> DeviceResult<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut limited = reader.take(MAX_REMOTE_FILE_BYTES + 1);
    limited
        .read_to_end(&mut buf)
        .await
        .map_err(|e| DeviceError::Io(std::io::Error::other(e.to_string())))?;
    if buf.len() as u64 > MAX_REMOTE_FILE_BYTES {
        return Err(DeviceError::Other(format!(
            "remote file {path} exceeds {} byte cap",
            MAX_REMOTE_FILE_BYTES
        )));
    }
    Ok(buf)
}

/// Quick TCP probe used by the connection watcher. Cheap; doesn't open SSH.
pub async fn is_reachable(host: &str, port: u16) -> bool {
    use std::time::Duration;
    let addr = format!("{host}:{port}");
    matches!(
        tokio::time::timeout(
            Duration::from_millis(800),
            tokio::net::TcpStream::connect(addr)
        )
        .await,
        Ok(Ok(_))
    )
}
