# reHydrate — Architecture

This document captures the design rationale behind the current
codebase. It was originally written as a forward-looking
implementation plan; the bulk has been preserved because the design
held up, with section 12 ("Deferred to v2") updated to reflect what
has since shipped.

## 1. Overview

A macOS desktop application that lets a user manage the documents on
their reMarkable 2 tablet from their computer, without using the
official cloud service. The app communicates with the device only
over its built-in USB connection. It mirrors documents to a local
library on the user's computer, keeps a full version history of every
change using content-addressed storage, and lets the user push, pull,
and restore documents at will.

The conceptual model is "iTunes for reMarkable" — a single trusted
desktop app where the user's library lives, with the tablet as a
synced satellite.

## 2. Goals and Non-Goals

### Goals
- Two-way sync between device and computer, over USB only.
- Full version history of every document, addressable by content hash.
- Ability to restore any previous version of a document, either back to the device or to a regular file on disk.
- One codebase that runs on Mac, Windows, and Linux.
- Privacy-respecting: no telemetry, no third-party servers, no cloud component.
- Honest handling of the device's actual data model — documents are trees of files, not single files, and the app must treat them that way.

### Non-Goals
- Editing notes on the desktop.
- Any cloud sync or remote access between user machines.
- Anything that would void the device's warranty (no firmware modification, no launcher mods).
- Mobile clients.

OCR and notebook rendering were originally deferred but were added in
v1 — see §12.

## 3. User Stories

The user should be able to:

1. Plug the device in, open the app, and see a "Connected" status without manual configuration after first-time setup.
2. See a unified view of documents on the device and documents in the local library, with clear indicators for new, changed, deleted, and unchanged items on each side.
3. Trigger a pull (device → computer), a push (computer → device), or a full two-way sync, with a preview of planned changes before anything destructive happens.
4. Browse the version history of any document and see when each version was captured.
5. Restore an old version of a document either to the device or as a regular file on disk.
6. Add PDFs or EPUBs from disk into the library and have them sent to the device on next sync.
7. Delete documents from the device while keeping them in the local archive, or fully delete from both.
8. Trust that closing the app, unplugging the device mid-sync, or losing power will not corrupt either the device or the local library.

## 4. Core Concepts

These are the primitives the rest of the design rests on. Names are placeholders.

**Document.** A single user-visible item on the tablet — a notebook, a PDF, an EPUB. Internally a document is a tree of files, not a single file. The implementation must respect this throughout.

**Folder.** A container for documents. Folders nest. Folder structure is part of metadata and must be preserved across sync.

**Manifest.** A snapshot of one document at one point in time. Records the names and content hashes of every file in the document's tree, plus the document's metadata as it stood. The hash of a manifest is what identifies a "version."

**Blob Store.** A content-addressed local archive. Every distinct file the app has ever seen is stored once, named by its content hash. Blobs are immutable. Two documents that share a file share storage automatically.

**Version Log.** For each document, an ordered list of manifest hashes capturing its history, with timestamps, source (observed on device, imported from disk, restored from history), and any user-added note.

**Library.** The full local state: blob store, version logs, folder and document metadata. The single source of truth on the desktop.

**Sync State.** The app's cached understanding of what it last saw on the device, used to detect change without re-hashing everything every time. Always recoverable from a full rescan.

## 5. High-Level Architecture

The app should be organized into modules that can be reasoned about and tested independently.

**Device Communication Module.** Owns the connection to the tablet. Detects presence, lists contents, downloads files, uploads documents, deletes documents. Exposes a clean abstract interface so the underlying transport can be swapped later. Every other module talks to the device only through this.

**Library Module.** Owns the blob store, version logs, and local metadata. Stores blobs, looks them up by hash, records new versions, lists history, and reconstructs the file tree for any past version on demand. Knows nothing about the device.

**Sync Engine.** The orchestrator. Compares device state to library state, builds a plan, executes it as a sequence of operations on the device and library modules. Supports dry-run, cancellation, partial failure, and resumption.

**Conflict Resolver.** Lives inside the sync engine but is its own concern. Decides what happens when a document has changed on both sides. The hard rule: never lose data. The losing side of a conflict is always preserved as a branch in the version log and can be restored.

**Importer.** Brings new files from disk into the library. Validates, records a first version, queues for upload on next sync.

**Exporter.** Writes a chosen version of a document to a regular file or folder on disk in a form the user can open in other applications.

**UI Layer.** The cross-platform desktop interface. Talks only to the modules above through their public interfaces. Contains no business logic.

**Background Service.** Watches for the device appearing on USB and notifies the UI. Optionally runs scheduled syncs. Must be cancellable and crash-safe, and must be optional — the app should be fully usable without it.

## 6. Key Flows

**First-time setup.** App explains what it does. User is guided through enabling the device's USB interface (one-time device-side setting) and through any one-time credential capture. A new empty library is created at a user-chosen location. No sync happens yet.

**Pull (device → library).** Confirm the device is reachable. Enumerate documents and folders. For each document, fetch its file tree, hash each file as it arrives, write new blobs into the blob store, and skip blobs already present. Build a manifest. If the manifest hash differs from the latest version recorded for that document, append a new version to the version log. Update local folder structure and metadata. Report new, changed, and unchanged counts to the user.

**Push (library → device).** Determine which documents in the library are missing from the device, or whose latest library version is newer than what's on the device. For each, reconstruct the file tree from the blob store at the chosen version, send it to the device, verify it landed correctly. Apply any queued folder moves or deletions. Update sync state.

**Two-way sync.** Pull first. Detect conflicts. Resolve them according to policy, always preserving the loser as a branch in history. Push.

**Restore an old version.** User picks a version from the history view. App reconstructs the file tree from the blob store at that manifest. Either writes it to the device as the document's new current state (which becomes a fresh version that happens to equal the old one), or exports it to disk.

**Browse history.** For any document, show every version with timestamp, size, source, and any user note. Change information is limited to "which files in the tree changed" — not visual diffs of pages.

**Import from disk.** User drops a PDF or EPUB in. App copies it into the blob store, creates a new document with metadata, records the first version. By default the imported document is **not** automatically uploaded to the device — it sits in the library and is sent only when the user explicitly selects it for sync. A "sync entire library" action is available for users who want everything pushed in one go. This default is a setting and can be flipped to auto-upload-on-import if the user prefers.

## 7. Conflict and Edge Case Handling

- **Both sides changed.** Never destroy either side. Both states become entries in the version log. One wins as "current"; the other is reachable via history. The default behavior is **always prompt the user** — the app surfaces the conflict and waits for a decision. The user can change this in settings to one of two automatic policies: device-side wins, or library-side wins. Whichever policy is in effect, the loser is always preserved as a branch in the version log so nothing is lost.
- **Deleted on one side, edited on the other.** Do not auto-resolve. Surface as a conflict. Default action: keep the data.
- **Sync interrupted mid-operation.** Blobs are immutable and content-addressed, so a half-finished pull leaves the blob store consistent — it just has some orphan blobs, which a periodic janitor pass collects. Manifests are written atomically; either a new version exists or it doesn't. There is no half-state.
- **File appears corrupt or truncated on the device.** Skip the affected document, surface a warning, do not advance sync state for it. The user's existing local copy remains intact.
- **Two computers syncing the same device.** Explicitly out of scope for v1. Document the limitation clearly so users don't try.
- **Library partially missing on disk.** The app should detect missing blobs gracefully. Either re-fetch from the device on next sync, or mark affected versions as "incomplete" — never crash, never hide the problem.
- **Clock drift on the device.** Timestamps are advisory. Authoritative comparison is always by content hash.
- **Large libraries.** The app must not require rehashing the entire library to detect changes. Use cached sync state, but be able to rebuild it from scratch on demand if the user requests an integrity check.

## 8. UI/UX Overview

The visual design language is **classic iTunes structure with modern styling**: a three-pane application window (top toolbar, sidebar plus content area, bottom status bar), dense and information-rich, but rendered in a clean modern visual idiom — system fonts, subtle hairline borders, restrained color, no skeuomorphic chrome, light/dark mode aware throughout. The design deliberately rewards users who want to see a lot of their library at once, in keeping with the "library manager" concept.

Four primary views.

**Library view — the main screen.** Three-pane layout:

- *Top toolbar.* On the left, sync controls (pull, push, two-way sync). In the center, a status pill showing device connection state, last sync time, and any pending changes — the spiritual replacement of iTunes' "now playing" display, repurposed for sync state. On the right, a search box.
- *Left sidebar.* A sectioned source list, iTunes-style, with small-caps section headers and indented items. Default sections: **Library** (All Documents, Notebooks, PDFs, EPUBs, Recently Synced), **Device** (the connected reMarkable, shown as a "mounted" item with an eject affordance), **Collections** (user folders and smart collections), **History** (All Versions, Conflicts).
- *Main content area.* A column-based document list with sortable headers, subtle zebra striping for readability, clear selection highlighting, and per-row status badges (synced, new, changed, conflict, only-on-device, only-on-library, queued-for-delete). Default columns: Title, Type, Pages, Modified, Status — user can show, hide, and reorder columns. Selecting a document reveals its detail and version history, either inline or in a side drawer.
- *Bottom status bar.* Summary line: total documents, total library size on disk, total versions stored. Includes a one-click action to garbage-collect unreferenced blobs.

**Sync view.** Shown when a sync starts. Presents the plan before executing it — the user can deselect specific items. Per-document progress, not just a single overall bar. Every operation cancellable cleanly at any point.

**History view.** Per-document, opened from the library view. A timeline of versions with size, timestamp, source, and notes. Per-version actions: restore to device, export to disk, annotate.

**Settings view.** A single screen exposing all user-configurable behavior. At minimum:

- *Library location.* Where the portable library directory lives. Changing this points the app at a different library; it does not move data.
- *Conflict resolution policy.* Always prompt (default), device-side wins, or library-side wins.
- *Version retention.* Unbounded (default), or a chosen retention policy that thins out old versions over time. Retention only ever affects the version log; the current version of every document is always kept.
- *Import behavior.* Manual upload (default — imported files stay in the library until explicitly selected for sync) or auto-upload on import.
- *Background watching.* On or off. The app must be fully usable with this off.

Cross-cutting: a read-only log of recent operations, accessible from any view, for trust and debugging.

A guiding principle: the UI should never lie about state. If the app does not know whether something synced, it says so. If the device is unreachable, the library remains fully usable in read-only mode.

## 9. Platform

- **USB networking.** macOS brings the device's network interface up
  automatically once the cable is connected. The reachability watcher
  (`rehydrate-app::commands::spawn_reachability_watcher`) probes
  `10.11.99.1:22` and surfaces the result to the UI.
- **Portable library.** The library is a single self-contained
  directory: blob store, version logs, metadata, sync state —
  everything the app needs lives inside it. The directory's location
  defaults to a sensible path on first run but is fully
  user-configurable. Names are hashes internally; user-visible names
  live in manifests.
- **Permissions.** The library directory contains personal documents
  and defaults to user-only access. Secrets live in the macOS
  keychain via the `keyring` crate, never on disk.
- **Cross-platform stance.** v1 ships ARM-only macOS. The codebase
  has no platform-specific business logic — Linux/Windows builds were
  cut for distribution reasons, not because the code couldn't run
  there. Sync, library, and version control behavior are platform-
  agnostic by construction.
- **Installers and updates.** Each platform has its norms. Sign and notarize where applicable. Auto-update should be opt-in and the app must function fully offline; it should never need to phone home for routine use.
- **No platform-specific business logic.** All platform differences should be confined to a thin adapter layer. Sync, library, and version control behavior must be identical everywhere.

## 10. Delivery history

Each phase ended with the app in a state the user could actually rely
on. As of v1.0:

- **Phase 1 — Read-only mirror.** Device detection, content listing,
  pull-only, blob store + version log. _Done._
- **Phase 2 — History and export.** Version-log UI, export-any-past-
  version. _Done._
- **Phase 3 — Push and two-way sync.** Upload, change detection,
  conflict handling that preserves losers in the version log.
  _Done._
- **Phase 4 — Library management.** PDF/EPUB import, folders,
  archive-with-restore, garbage collection. _Done._
- **Phase 5 — Polish.** Background watching, operation log,
  `Verify` integrity check, OCR (originally v2-deferred), publishing
  to Ghost/WordPress. _Done._

Signed-installer / notarization is the headline remaining v1.x
polish item — see `PACKAGING.md`.

## 11. Testing Strategy

- Treat the device communication module as a swappable interface. Provide a fake backed by a local directory tree that mimics the device. Most of the app's logic can be tested against the fake without ever touching hardware.
- Property-test the blob store: any sequence of writes and reads must preserve content; any stored blob is retrievable by its hash; manifests reconstruct exactly.
- Scenario-test the sync engine end-to-end against the fake device: clean pull, clean push, both-side edit, deletion races, interrupted sync resumed, corrupted blob detected.
- Manual testing matrix on real hardware on each supported OS for each release. The fake covers logic; real hardware covers transport and platform quirks.
- Maintain a privacy invariant: an automated check that no build artifact attempts outbound network activity to anything other than the device. The expected count of external network destinations is, and stays, zero.

## 12. Originally deferred to v2 — current status

- **Encryption at rest** for the library. _Still deferred._ v1
  relies on OS-level full-disk encryption (FileVault). The data
  model is shaped so wrapping blob/manifest reads/writes in an
  encryption layer later remains a layered change.
- **Multi-computer sync against the same device.** _Still deferred._
  v1 assumes one library, one device. The sync state model does not
  prevent this from being added later.
- **Note rendering and OCR.** _Shipped in v1._ Notebook rendering
  lives in `crates/rm-parser` (parses reMarkable v6) and
  `crates/rehydrate-render` (renders to PDF). OCR is a
  separate crate, `crates/rehydrate-ocr`, talking to a user-supplied
  Ollama daemon. Full-text search remains deferred.
- **Publishing to a CMS.** _Shipped in v1._ `crates/rehydrate-publish`
  posts OCR transcripts to Ghost or WordPress as drafts, via a
  host-pinned HTTP client.
- **Command-line interface.** _Still deferred._ All domain crates
  (`rehydrate-core`, `rehydrate-device`, `rehydrate-sync`, etc.)
  remain Tauri-independent, so adding a CLI later means a new
  binary, not a refactor.
- **Mobile clients.** _Still deferred._
