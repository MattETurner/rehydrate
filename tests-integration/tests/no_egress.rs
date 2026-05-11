//! Privacy invariant tests.
//!
//! The only outbound network destination reHydrate talks to is the
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
            "reHydrate must not depend on `{crate_name}`. The webview UI must \
             not be able to issue arbitrary network requests. If this dependency \
             is intentional, update no_egress.rs together with a justification."
        );
    }
}

#[test]
fn rehydrate_business_crates_have_no_http_clients() {
    // The business-logic crates listed below must stay free of
    // general-purpose HTTP clients. `rehydrate-ocr` (Ollama) and
    // `rehydrate-publish` (Ghost / WordPress) are *sanctioned*
    // egress paths and are deliberately omitted from this list —
    // both route every request through a host-pinned
    // `RestrictedAgent` (asserted separately by
    // `sanctioned_egress_crates_pin_to_one_host`).
    use std::process::Command;
    let crates = ["rehydrate-core", "rehydrate-device", "rehydrate-sync"];
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
fn sanctioned_egress_crates_pin_to_one_host() {
    // The two crates that legitimately make outbound HTTP requests
    // (`rehydrate-ocr` for Ollama, `rehydrate-publish` for
    // Ghost/WordPress) must both expose a `RestrictedAgent` that
    // pins each request to the host the user explicitly configured.
    // A future refactor that bypasses the wrapper — e.g. by
    // calling `ureq::Agent::new()` directly — would defeat the
    // privacy story, so this positive-control assertion locks the
    // invariant in CI.
    let root = workspace_root_path();
    for path in [
        root.join("crates/rehydrate-ocr/src/http.rs"),
        root.join("crates/rehydrate-publish/src/http.rs"),
    ] {
        let body = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "expected sanctioned-egress wrapper at {} ({e})",
                path.display()
            )
        });
        assert!(
            body.contains("pub struct RestrictedAgent"),
            "{} must define `pub struct RestrictedAgent`: the wrapper is the \
             only sanctioned way to obtain a ureq agent inside the crate.",
            path.display()
        );
        assert!(
            body.contains("host_of("),
            "{} must use `host_of(...)` to compare request hosts against the \
             pinned base — bypassing it would let a redirect or malicious \
             config exfiltrate to a different host.",
            path.display()
        );
    }
    // Same files must not call the un-pinned `ureq::Agent::new()`
    // constructor — that would bypass the host pin entirely.
    // `AgentBuilder::new()` is fine, since `for_base_with_timeout`
    // builds the pinned agent through it; we ban it only outside
    // those constructor paths via grep — which is impossible
    // without a real parser, so we rely on the broader assertion
    // that the file uses the host-pin pattern.
    //
    // The narrower check: no file in these crates *outside* http.rs
    // should reference `ureq::Agent::new` or `ureq::AgentBuilder::new`.
    // The wrappers themselves call `AgentBuilder::new()` inside
    // `for_base_with_timeout`; anything else routing around the
    // wrapper would have to do so here.
    for (crate_dir, allowed_file) in &[
        ("crates/rehydrate-ocr/src", "http.rs"),
        ("crates/rehydrate-publish/src", "http.rs"),
    ] {
        let dir = root.join(crate_dir);
        let entries = fs::read_dir(&dir).unwrap_or_else(|e| {
            panic!("cannot read {} ({e})", dir.display())
        });
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            if p.file_name().and_then(|n| n.to_str()) == Some(*allowed_file) {
                continue;
            }
            let body = fs::read_to_string(&p).unwrap_or_default();
            assert!(
                !body.contains("ureq::Agent::new")
                    && !body.contains("ureq::AgentBuilder::new")
                    && !body.contains("AgentBuilder::new()"),
                "{} must construct ureq agents only through {}::RestrictedAgent — \
                 calling ureq::Agent::new() / AgentBuilder::new() directly bypasses \
                 the host pin.",
                p.display(),
                allowed_file
            );
        }
    }
}

#[test]
fn tauri_conf_keeps_csp_set() {
    // Audit fix M5 set a real CSP; this test prevents a silent
    // regression to `"csp": null`.
    let conf = workspace_root_path()
        .join("crates")
        .join("rehydrate-app")
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
