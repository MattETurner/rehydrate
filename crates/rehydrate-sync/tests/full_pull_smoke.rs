//! End-to-end smoke test: connect to a real reMarkable, pull every document
//! into a temp library, verify dedup on a second pull, list everything.
//!
//! Run:
//! ```sh
//! MARGINALIA_RM_PASSWORD='your-device-password' \
//!     cargo test -p rehydrate-sync --test full_pull_smoke -- --ignored --nocapture
//! ```
//!
//! Read-only against the device. Mutates only a temp library directory that
//! is cleaned up at the end of the test.

use std::time::Instant;

use rehydrate_core::Library;
use rehydrate_device::known_hosts::KnownHosts;
use rehydrate_device::ssh::{is_reachable, SshConfig, SshDevice};
use rehydrate_device::Device;
use rehydrate_sync::{execute_pull, plan_pull, progress, ProgressEvent};
use secrecy::SecretString;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a real reMarkable connected over USB and MARGINALIA_RM_PASSWORD"]
async fn full_pull_against_real_device() {
    let password =
        std::env::var("MARGINALIA_RM_PASSWORD").expect("MARGINALIA_RM_PASSWORD must be set");
    let cfg = SshConfig::default();

    println!("=== probe {}:{} ===", cfg.host, cfg.port);
    assert!(
        is_reachable(&cfg.host, cfg.port).await,
        "tablet TCP probe failed"
    );

    let lib_dir = tempfile::tempdir().expect("tempdir");
    println!("library: {}", lib_dir.path().display());

    let kh_dir = tempfile::tempdir().expect("tempdir for known_hosts");
    let kh = KnownHosts::new(kh_dir.path().join("known_hosts.json"));
    let dev = SshDevice::connect(cfg, SecretString::from(password), kh)
        .await
        .expect("ssh connect failed");
    let info = dev.ping().await.expect("ping");
    println!("device: {info:?}");

    let lib = Library::open(lib_dir.path()).expect("library open");

    // ===== first pull =====
    println!("=== plan_pull (first) ===");
    let t0 = Instant::now();
    let plan = plan_pull(&lib, &dev).await.expect("plan_pull");
    println!(
        "planned {} entries in {:?} (new={}, changed={}, unchanged={}, skipped={})",
        plan.items.len(),
        t0.elapsed(),
        plan.items
            .iter()
            .filter(|p| matches!(p.status, rehydrate_sync::PlanItemStatus::New))
            .count(),
        plan.items
            .iter()
            .filter(|p| matches!(p.status, rehydrate_sync::PlanItemStatus::Changed))
            .count(),
        plan.items
            .iter()
            .filter(|p| matches!(p.status, rehydrate_sync::PlanItemStatus::Unchanged))
            .count(),
        plan.items
            .iter()
            .filter(|p| matches!(p.status, rehydrate_sync::PlanItemStatus::Skipped))
            .count(),
    );

    let (tx, mut rx) = progress::channel(64);
    let progress_task = tokio::spawn(async move {
        let mut bytes_total = 0u64;
        let mut bytes_deduped = 0u64;
        let mut files_total = 0usize;
        let mut files_deduped = 0usize;
        while let Some(ev) = rx.recv().await {
            match ev {
                ProgressEvent::DocumentStarted { visible_name, .. } => {
                    println!("  → {visible_name}");
                }
                ProgressEvent::FileFetched { bytes, deduped, .. } => {
                    bytes_total += bytes;
                    files_total += 1;
                    if deduped {
                        bytes_deduped += bytes;
                        files_deduped += 1;
                    }
                }
                ProgressEvent::DocumentSkipped {
                    document_id,
                    reason,
                } => {
                    println!("  ⚠ skipped {document_id}: {reason}");
                }
                ProgressEvent::Done {
                    recorded,
                    unchanged,
                    skipped,
                } => {
                    println!(
                        "  done: recorded={recorded} unchanged={unchanged} skipped={skipped}, \
                         {files_total} files ({bytes_total} bytes), \
                         {files_deduped} deduped ({bytes_deduped} bytes)"
                    );
                }
                _ => {}
            }
        }
        (files_total, bytes_total, files_deduped, bytes_deduped)
    });

    let t0 = Instant::now();
    let report = execute_pull(&lib, &dev, plan, Some(tx), Default::default())
        .await
        .expect("execute_pull");
    let elapsed = t0.elapsed();
    let (files_total, bytes_total, files_deduped, bytes_deduped) =
        progress_task.await.expect("progress task");
    println!(
        "execute_pull(first) {report:?} in {elapsed:?}, {files_total} files / {bytes_total} bytes total \
         ({files_deduped} files / {bytes_deduped} bytes deduped within batch)"
    );
    // Cross-document dedup *within* a first pull is expected: multiple
    // notebooks tend to share identical `.local`, empty `.pagedata`, etc.
    // The real invariant is that the second pull writes zero new blobs —
    // verified below.
    assert!(
        report.recorded > 0,
        "expected at least one document recorded"
    );

    let docs = lib.list_documents().expect("list_documents");
    println!("library now has {} documents", docs.len());
    assert!(!docs.is_empty(), "library should have documents after pull");

    // ===== second pull (should be a complete no-op) =====
    println!("=== plan_pull (second) ===");
    let plan2 = plan_pull(&lib, &dev).await.expect("plan_pull #2");
    let unchanged = plan2
        .items
        .iter()
        .filter(|p| matches!(p.status, rehydrate_sync::PlanItemStatus::Unchanged))
        .count();
    println!(
        "second plan: {} entries, {unchanged} unchanged",
        plan2.items.len()
    );
    assert_eq!(
        unchanged,
        plan2.items.len(),
        "every entry (docs + folders) must be 'Unchanged' on a second back-to-back pull"
    );

    let (tx2, mut rx2) = progress::channel(64);
    let bg = tokio::spawn(async move {
        let mut new_files = 0usize;
        while let Some(ev) = rx2.recv().await {
            if matches!(ev, ProgressEvent::FileFetched { deduped: false, .. }) {
                new_files += 1;
            }
        }
        new_files
    });
    let report2 = execute_pull(&lib, &dev, plan2, Some(tx2), Default::default())
        .await
        .expect("execute_pull #2");
    let new_files = bg.await.unwrap();
    println!("execute_pull(second) {report2:?}, new files written: {new_files}");
    assert_eq!(report2.recorded, 0, "second pull must record nothing new");
    assert_eq!(new_files, 0, "second pull must not write any new blob");
}
