# reHydrate

A privacy-respecting desktop app for managing documents on a reMarkable 2 over USB. No cloud, no telemetry; the app talks only to a tablet plugged into your computer.

The library lives in a single self-contained directory you control: every distinct file is stored once by content hash, every change to every document is captured as a new version, and any past version can be restored to the device or exported to disk.

## Status

`v0.9.1` — first public release (`v0.9.0` macOS bundles were
unlaunchable on Apple Silicon — see `CHANGELOG.md`). Beta-quality:
feature-complete for the
sync + library use case, but bundles ship unsigned (macOS Gatekeeper /
Windows SmartScreen will warn on first launch — `PACKAGING.md` documents
the signing config that's deferred to `v1.0.0`). Working features:

- Two-way sync (pull + push) over USB-SSH, with progress streamed live.
- Content-addressed blob store, full version history, restore-any-version.
- Archive with restore (soft-delete; the tablet only sees the deletion
  on the next sync).
- Import PDFs and EPUBs from disk; they upload to the tablet on the
  next sync.
- Folder organisation, drag-and-drop moves, multi-select with
  Cmd-click + Shift-click + a selection toolbar.
- Library health (`Verify`) and orphan-blob cleanup (`Clean up unused
  files`) with confirmation.
- Quick Look (Space), command palette (⌘K), keyboard navigation,
  search.
- Multi-library support: switch between per-tablet libraries from the
  toolbar without restarting.

Architecture notes are in `remarkable-sync-implementation-plan.md`;
release notes are in `CHANGELOG.md`.

## Stack

- **Core:** Rust workspace (`crates/rehydrate-core`, `rehydrate-device`, `rehydrate-sync`, `rehydrate-app`)
- **UI:** Tauri 2 + Vite + React + TypeScript (`ui/`)
- **Transport:** SSH/SFTP over USB-ethernet (`10.11.99.1`)

## Building

Prerequisites: Rust stable, Node 20+, npm.

```sh
# Install UI deps and build static assets
(cd ui && npm install && npm run build)

# Run the app
cargo run -p rehydrate-app --release
```

For day-to-day dev with UI hot reload, install the Tauri CLI:

```sh
cargo install tauri-cli --version "^2.0.0"
cargo tauri dev
```

## Testing against a real reMarkable

Two integration tests run against a tablet plugged in over USB. Both
are read-only.

```sh
# Capture password: tablet → Settings → Help → Copyrights and licenses
MARGINALIA_RM_PASSWORD='...' \
    cargo test -p rehydrate-device --features ssh \
    --test ssh_smoke -- --ignored --nocapture

MARGINALIA_RM_PASSWORD='...' \
    cargo test -p rehydrate-sync \
    --test full_pull_smoke -- --ignored --nocapture
```

Packaging notes (signed installers, icons, notarization) are in
`PACKAGING.md`.

## Layout

```
crates/
  rehydrate-core/    blob store, manifests, version log
  rehydrate-device/  device transport: trait + fake + ssh
  rehydrate-sync/    sync engine
  rehydrate-app/     tauri binary
ui/               react frontend
```
