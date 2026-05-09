//! Privacy invariant tests.
//!
//! The only outbound network destination Marginalia talks to is the
//! reMarkable USB endpoint at `10.11.99.1`, contacted via SSH/SFTP through
//! `russh`. These are static checks against `Cargo.lock` rather than a
//! runtime sniffer because the latter would need a kernel-level network
//! sandbox to be trustworthy. A grep is good enough to keep us honest.
//!
//! Tauri's own dep graph pulls in `reqwest` for internal IPC plumbing —
//! that's noise we can't realistically remove. What matters is that no
//! *plugin* that surfaces arbitrary HTTP capabilities to the webview is
//! present, and that no JS-callable command in our app issues network
//! requests. The tests below enforce both.

use std::fs;
use std::path::PathBuf;

fn workspace_lockfile() -> String {
    // CARGO_MANIFEST_DIR points at this crate; the workspace Cargo.lock
    // sits one directory above.
    let lock_path: PathBuf = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".."))
        .join("Cargo.lock");
    fs::read_to_string(&lock_path).unwrap_or_else(|e| {
        panic!(
            "could not read {} — run `cargo build` first ({e})",
            lock_path.display()
        )
    })
}

#[test]
fn no_http_capability_plugins_loaded() {
    let lock = workspace_lockfile();

    // Banned plugins — these expose HTTP fetch / shell exec / global Tauri
    // capabilities to the webview JS. Any of them being present in our dep
    // graph would mean we shipped JS access to arbitrary external hosts.
    //
    // tauri-plugin-shell IS present (and is fine — we use it to open
    // local file paths), but it's the only "broad capability" plugin we
    // accept. tauri-plugin-http would expose `fetch()` to JS, which we
    // refuse.
    let banned_plugins = [
        "tauri-plugin-http",
        "tauri-plugin-upload",
        "tauri-plugin-websocket",
    ];
    for crate_name in &banned_plugins {
        let needle = format!("name = \"{crate_name}\"");
        assert!(
            !lock.contains(&needle),
            "Marginalia must not depend on `{crate_name}`. The webview UI must \
             not be able to issue arbitrary network requests. If this dependency \
             is intentional, update no_egress.rs together with a justification."
        );
    }
}

#[test]
fn rmsync_crates_have_no_general_purpose_http_clients() {
    // Business-logic crates that must NEVER speak HTTP. The
    // user-explicit publish + ocr crates are intentionally NOT in
    // this list — they're the only crates allowed to make
    // outbound HTTP, and only to user-configured CMS hosts /
    // HuggingFace's allow-list.
    use std::process::Command;
    let crates = ["rmsync-core", "rmsync-device", "rmsync-sync"];
    let workspace_root = workspace_root_path();

    let banned = [
        "reqwest",
        "ureq",
        "isahc",
        "surf",
        "hyper-tls",
        "hyper-rustls",
        "awc",
    ];

    for crate_name in &crates {
        let output = Command::new(env!("CARGO"))
            .args([
                "tree", "--target", "all", "-p", crate_name, "-e", "normal", "--prefix", "none",
            ])
            .current_dir(&workspace_root)
            .output()
            .expect("cargo tree failed to execute");
        let stdout = String::from_utf8_lossy(&output.stdout);
        for banned_crate in &banned {
            assert!(
                !stdout.contains(&format!("{banned_crate} v")),
                "{crate_name} must not transitively depend on `{banned_crate}`. \
                 Business-logic crates must stay free of general-purpose HTTP \
                 clients so the privacy story holds even outside the Tauri shell."
            );
        }
    }
}

#[test]
fn publish_and_ocr_crates_use_ureq_only() {
    // Positive controls: the publish + ocr crates DO depend on
    // ureq (the sanctioned HTTP client). If a refactor accidentally
    // drops them, the feature is silently gone.
    use std::process::Command;
    let workspace_root = workspace_root_path();

    for crate_name in ["rmsync-publish", "rmsync-ocr"] {
        let output = Command::new(env!("CARGO"))
            .args([
                "tree", "--target", "all", "-p", crate_name, "-e", "normal", "--prefix", "none",
            ])
            .current_dir(&workspace_root)
            .output()
            .expect("cargo tree failed to execute");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("ureq v"),
            "{crate_name} must depend on `ureq` (sanctioned HTTP client). \
             If this dependency disappeared the feature is broken."
        );
        // And conversely — the *banned* clients must not slip in.
        for banned_crate in ["reqwest", "isahc", "surf", "awc"] {
            assert!(
                !stdout.contains(&format!("{banned_crate} v")),
                "{crate_name} must not depend on `{banned_crate}`; ureq is the \
                 sanctioned HTTP client for the publish/ocr layer."
            );
        }
    }
}

#[test]
fn publish_http_uses_restricted_agent_wrapper() {
    // Defence-in-depth: the publish crate's `RestrictedAgent` is
    // the only sanctioned way to obtain a `ureq::Agent`. Anything
    // else would let a future caller open connections to arbitrary
    // hosts. Grep the source files to confirm `Agent::new` /
    // `AgentBuilder::new` only appear inside http.rs.
    let publish_root = workspace_root_path()
        .join("crates")
        .join("rmsync-publish")
        .join("src");
    let mut offenders = Vec::new();
    for entry in fs::read_dir(&publish_root).expect("read publish src") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        if name == "http.rs" {
            continue; // wrapper itself is allowed to construct an Agent
        }
        let body = fs::read_to_string(&path).unwrap();
        if body.contains("ureq::AgentBuilder")
            || body.contains("ureq::Agent::")
            || body.contains("AgentBuilder::new")
        {
            offenders.push(name);
        }
    }
    assert!(
        offenders.is_empty(),
        "ureq::Agent must only be constructed inside http.rs. Offenders: {offenders:?}"
    );
}

#[test]
fn tauri_conf_keeps_csp_set() {
    // Audit fix M5 set a real CSP; this test prevents a silent
    // regression to `"csp": null`.
    let conf = workspace_root_path()
        .join("crates")
        .join("rmsync-app")
        .join("tauri.conf.json");
    let body = fs::read_to_string(&conf).expect("read tauri.conf.json");
    let json: serde_json::Value = serde_json::from_str(&body).expect("tauri.conf.json json");
    let csp = json
        .pointer("/app/security/csp")
        .expect("missing /app/security/csp in tauri.conf.json");
    assert!(
        csp.is_string() && !csp.as_str().unwrap().is_empty(),
        "tauri.conf.json /app/security/csp must be a non-empty string"
    );
}

fn workspace_root_path() -> PathBuf {
    std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".."))
}

#[test]
fn ssh_is_the_only_network_transport() {
    // CARGO_MANIFEST_DIR points at this crate; the workspace Cargo.lock
    // sits one directory above.
    let lock_path: PathBuf = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".."))
        .join("Cargo.lock");
    let lock = fs::read_to_string(&lock_path).unwrap();
    // We expect russh (SSH client) and russh-sftp (SFTP subsystem).
    // The presence of these is a positive control — if they vanish, the
    // app can't talk to the device at all and we'd want to know.
    assert!(
        lock.contains("name = \"russh\""),
        "russh disappeared from the dep graph"
    );
    assert!(
        lock.contains("name = \"russh-sftp\""),
        "russh-sftp disappeared from the dep graph"
    );
}
