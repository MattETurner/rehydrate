# Marginalia

A privacy-respecting desktop app for managing documents on a reMarkable 2 over USB. No cloud, no telemetry; the app talks only to a tablet plugged into your computer.

The library lives in a single self-contained directory you control: every distinct file is stored once by content hash, every change to every document is captured as a new version, and any past version can be restored to the device or exported to disk.

## Status

Pre-alpha, under active development. Phase 1 (read-only mirror) is the current target — see `remarkable-sync-implementation-plan.md` for the design, and `crates/` for the implementation.

## Stack

- **Core:** Rust workspace (`crates/rmsync-core`, `rmsync-device`, `rmsync-sync`, `rmsync-app`)
- **UI:** Tauri 2 + Vite + React + TypeScript (`ui/`)
- **Transport:** SSH/SFTP over USB-ethernet (`10.11.99.1`)

## Building

Prerequisites: Rust stable, Node 20+, npm.

```sh
# Install UI deps
(cd ui && npm install)

# Run the app in dev mode
cargo tauri dev          # if cargo-tauri is installed
# or, equivalently:
(cd ui && npm run dev) & cargo run -p rmsync-app
```

## Layout

```
crates/
  rmsync-core/    blob store, manifests, version log
  rmsync-device/  device transport: trait + fake + ssh
  rmsync-sync/    sync engine
  rmsync-app/     tauri binary
ui/               react frontend
```
