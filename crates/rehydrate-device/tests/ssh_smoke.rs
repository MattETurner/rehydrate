//! Integration smoke test that runs **only** when:
//!   - the `ssh` feature is enabled, AND
//!   - the env var `MARGINALIA_RM_PASSWORD` is set, AND
//!   - the test is invoked with `--ignored`.
//!
//! Run with:
//! ```sh
//! MARGINALIA_RM_PASSWORD='your-device-password' \
//!     cargo test -p rehydrate-device --features ssh \
//!     --test ssh_smoke -- --ignored --nocapture
//! ```
//!
//! Optional env vars:
//!   - `MARGINALIA_RM_HOST` (default `10.11.99.1`)
//!   - `MARGINALIA_RM_USER` (default `root`)
//!   - `MARGINALIA_RM_PORT` (default `22`)
//!
//! The test:
//!   1. Connects via SSH/SFTP.
//!   2. Calls `ping()`.
//!   3. Lists documents and prints a one-line summary.
//!   4. For the first document, fetches its file tree and prints sizes.
//!
//! It does **not** mutate any device state.

#![cfg(feature = "ssh")]

use std::time::Instant;

use rehydrate_device::ssh::{is_reachable, SshConfig, SshDevice};
use rehydrate_device::{Device, RemoteEntryKind};
use secrecy::SecretString;

fn cfg_from_env() -> SshConfig {
    SshConfig {
        host: std::env::var("MARGINALIA_RM_HOST").unwrap_or_else(|_| "10.11.99.1".into()),
        port: std::env::var("MARGINALIA_RM_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(22),
        user: std::env::var("MARGINALIA_RM_USER").unwrap_or_else(|_| "root".into()),
        xochitl_dir: std::env::var("MARGINALIA_RM_XOCHITL")
            .unwrap_or_else(|_| "/home/root/.local/share/remarkable/xochitl".into()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a real reMarkable connected over USB and MARGINALIA_RM_PASSWORD"]
async fn ssh_smoke_against_real_device() {
    let password = std::env::var("MARGINALIA_RM_PASSWORD")
        .expect("MARGINALIA_RM_PASSWORD must be set for this test");
    let cfg = cfg_from_env();

    println!("=== probe {}:{} ===", cfg.host, cfg.port);
    assert!(
        is_reachable(&cfg.host, cfg.port).await,
        "tablet TCP probe failed"
    );

    println!("=== connect ===");
    let t0 = Instant::now();
    let dev = SshDevice::connect(cfg.clone(), SecretString::from(password))
        .await
        .expect("ssh connect failed");
    println!("connected in {:?}", t0.elapsed());

    println!("=== ping ===");
    let info = dev.ping().await.expect("ping failed");
    println!("device: {info:?}");

    println!("=== list_documents ===");
    let t0 = Instant::now();
    let entries = dev.list_documents().await.expect("list_documents failed");
    println!(
        "listed {} entries in {:?} ({} docs, {} folders)",
        entries.len(),
        t0.elapsed(),
        entries
            .iter()
            .filter(|e| matches!(e.kind, RemoteEntryKind::Document))
            .count(),
        entries
            .iter()
            .filter(|e| matches!(e.kind, RemoteEntryKind::Folder))
            .count(),
    );
    for e in entries.iter().take(5) {
        println!(
            "  - {} [{}] {} (parent={:?})",
            e.uuid, e.doc_type, e.visible_name, e.parent
        );
    }

    let first_doc = entries
        .iter()
        .find(|e| matches!(e.kind, RemoteEntryKind::Document))
        .expect("device has no documents — connect a populated tablet");

    println!("=== fetch_document_tree({}) ===", first_doc.uuid);
    let t0 = Instant::now();
    let files = dev
        .fetch_document_tree(&first_doc.uuid)
        .await
        .expect("fetch failed");
    let total: u64 = files.iter().map(|f| f.bytes.len() as u64).sum();
    println!(
        "fetched {} files ({} bytes) in {:?}",
        files.len(),
        total,
        t0.elapsed()
    );
    for f in &files {
        println!("  {:>10}  {}", f.bytes.len(), f.path);
    }

    assert!(!files.is_empty(), "document fetch returned no files");
    assert!(
        files
            .iter()
            .any(|f| f.path == format!("{}.metadata", first_doc.uuid)),
        ".metadata file missing from fetch"
    );
}
