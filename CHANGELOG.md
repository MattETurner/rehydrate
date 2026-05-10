# Changelog

All notable changes to reHydrate are recorded here. The format is loosely
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
the project follows [Semantic Versioning](https://semver.org/) once it
hits `1.0.0`. Pre-1.0 releases may break compatibility freely; the
library on-disk format is forward-stable from `0.9.0`.

## [0.9.0] — 2026-05-10

First public release. Feature-complete for the sync + library use case
the project set out to solve; bundles ship unsigned, full code-signing
is deferred to `1.0.0`.

### Sync (USB)

- Pull from a reMarkable 2 over USB-SSH, into a content-addressed blob
  store with one stored copy per distinct file content. The same notebook
  template across 200 documents costs ~80 KB on disk.
- Push library-side edits back to the tablet: rename, move-between-folders,
  archive (soft-delete). Each push diffs against the device manifest so
  unchanged files transfer zero bytes.
- Two-way sync that runs pull then push under one transaction, with a
  pre-flight plan that shows the user exactly what will move before
  they hit `Start`.
- Live per-document progress in the sync drawer; the toolbar `StatusPill`
  shows a `Syncing…` chip while a sync is in flight, so closing the
  drawer doesn't lose visibility.

### Library

- Content-addressed blob store: every distinct file is stored once by
  SHA-256, so duplicates across documents and versions are free.
- Full version history: every change to every document is captured as
  a new version with a parent pointer, optional human note, byte-level
  diff against the parent, and one-click restore.
- Archive with restore: archived documents disappear from the library
  view and from the tablet on next sync, but every prior version is
  retained; restore brings them back.
- Permanent purge from archive: drops every saved version of a document
  (gated by an explicit confirm).
- Folder tree mirroring the tablet's hierarchy. Drag-and-drop moves
  documents between folders or into the archive; spring-loaded folders
  expand under a hovering drag.
- Multi-library support: switch between per-tablet libraries from a
  toolbar dropdown without restarting the app. The recents list stays
  sticky across launches.

### Import + export

- Import any PDF or EPUB via a server-side native picker (renderer
  cannot supply arbitrary paths; audit fix H6). The imported file
  uploads to the tablet on the next sync.
- Export any version of any document back to disk via the history
  drawer, again through a server-side picker.

### Health

- Library health check (`Verify`) reports manifest integrity, missing
  blobs, and orphan blobs at a glance.
- Garbage-collect orphan blobs (`Clean up unused files`) with a
  confirmation dialog explaining the operation.

### UX

- Onboarding (plug in tablet → open library) on first launch; auto-
  advances when the tablet is detected.
- Quick Look popover (Space) with thumbnail and metadata. Follows
  ↑/↓ navigation while open, mirroring macOS Finder.
- Command palette (⌘K) with fuzzy match across actions, views, folders,
  and documents.
- Search-this-view (⌘F) with live filter.
- Drag-and-drop with multi-selection batch support; custom drag-image
  showing the count.
- Keyboard navigation: arrow keys for the focus cursor, Enter to open,
  F2 to rename, ⌘⌫ to bulk-archive, Esc to close any dialog/drawer.
- View persistence: the last view (`All Documents` / `Pending Sync` /
  `Notebooks` / etc.) and viewMode (list vs. grid) survive across
  launches.
- Per-document menu in both list and grid views (Open in viewer,
  Rename…, Show history, Move to Archive).
- Toast system with optional `Undo` action on archive + move.
- Cheatsheet (`?`) listing every keyboard shortcut.

### Privacy

- No telemetry, no analytics, no auto-update check on launch — the app
  makes zero network calls until the user takes an explicit action.
- The only network destination the app contacts is the tablet at
  `10.11.99.1` over USB-SSH (russh + russh-sftp).
- Tablet password persists in the OS keychain (Keychain on macOS,
  Secret Service on Linux, Credential Manager on Windows).
- Locked Tauri capability surface: no `tauri-plugin-http`, no
  `tauri-plugin-shell` for arbitrary commands, content-security-policy
  set to `default-src 'self'; img-src 'self' data: blob:; …`.
- An automated CI check (`tests-integration/tests/no_egress.rs`)
  asserts that no Rust crate in the workspace transitively depends on
  any general-purpose HTTP client (`reqwest`, `ureq`, `isahc`, `surf`,
  `hyper-tls`, `hyper-rustls`, `awc`). The privacy invariant is
  enforced in code, not just documented.

### Engineering

- 38 workspace tests (29 in `rehydrate-core`, 3 in `rehydrate-sync`,
  2 in `rehydrate-device`, 4 integration including the egress
  invariants).
- CI runs `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, the workspace test suite, and a
  cross-platform UI build on every push.
- Release workflow fans out to four runners (macOS Intel, macOS Apple
  Silicon, Linux, Windows) and attaches `.dmg` / `.AppImage` / `.deb`
  / `.msi` bundles to a draft GitHub Release.

### Known gaps

- **Bundles are unsigned.** macOS users see a Gatekeeper warning on
  first launch (right-click → Open clears it); Windows users see
  SmartScreen ("Run anyway"). Code-signing config in `tauri.conf.json`
  is documented in `PACKAGING.md` and wired up to take effect once
  signing secrets are added to the release workflow. Targeted for
  `v1.0.0`.
- **No auto-update.** The design says auto-update should be opt-in;
  not implemented for this release.
- **`rehydrate-app` IPC layer has no test suite.** Other crates are
  well-covered; the IPC commands are exercised by manual smoke runs.
  Adding an IPC test harness is on the `1.0.0` punch list.

[0.9.0]: https://github.com/dm807cam/rehydrate/releases/tag/v0.9.0
