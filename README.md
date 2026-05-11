# reHydrate

A privacy-respecting desktop app for managing documents on a reMarkable 2 over USB. No cloud, no telemetry; the app talks only to a tablet plugged into your computer.

The library lives in a single self-contained directory you control: every distinct file is stored once by content hash, every change to every document is captured as a new version, and any past version can be restored to the device or exported to disk.

## Status

`v1.0.0` — first stable release. Bundles still ship **unsigned**; see
[Installing](#installing) below for the Gatekeeper / SmartScreen
right-click dance on first launch. Working features:

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
release notes are in `CHANGELOG.md`; threat model and security
posture in `SECURITY.md`.

## Installing

reHydrate ships pre-built bundles on the [GitHub Releases][releases]
page for macOS (Intel + Apple Silicon), Windows, and Linux. v1.0
bundles are **unsigned** — buying code-signing certificates is on the
roadmap for v1.1. Until then, the OS will warn on first launch:

### macOS

1. Download the matching `.dmg` for your Mac (Intel or Apple Silicon).
2. **Right-click the `.dmg`** (or Control-click) and choose **Open**.
   *Don't* double-click — Gatekeeper will reject the unsigned bundle.
3. Drag reHydrate to `/Applications`.
4. The first time you launch the app, **right-click reHydrate in
   `/Applications`** and choose **Open**. Confirm the "unidentified
   developer" warning. After this one-time bypass, normal
   double-click works.

### Windows

1. Download the `.msi` installer.
2. Windows SmartScreen will warn "Windows protected your PC". Click
   **More info → Run anyway** to proceed.
3. Install and launch normally.

### Linux

The `.AppImage` works on most distributions out of the box — make it
executable (`chmod +x`) and run. The `.deb` targets Debian/Ubuntu;
install via `sudo dpkg -i reHydrate_*.deb`.

On minimal Linux installs without `secret-service` (e.g.
gnome-keyring), the app can't save your tablet's SSH password between
sessions — you'll see a toast warning and need to type it on each
connect. Install `gnome-keyring` or `KeePassXC`-with-secret-service
to enable persistence.

## Updates

reHydrate does **not** auto-update. Subscribe to the GitHub repo's
release feed, or check the [Releases][releases] page periodically.
Security fixes will be called out in the release notes.

[releases]: https://github.com/dm807cam/rehydrate/releases

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
