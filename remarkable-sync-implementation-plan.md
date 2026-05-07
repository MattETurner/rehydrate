# reMarkable Sync — Implementation Plan

## 1. Overview

A cross-platform desktop application for Mac, Windows, and Linux that lets a user manage the documents on their reMarkable 2 tablet from their computer, without using the official cloud service. The app communicates with the device only over its built-in USB connection. It mirrors documents to a local library on the user's computer, keeps a full version history of every change using content-addressed storage, and lets the user push, pull, and restore documents at will.

The conceptual model is "iTunes for reMarkable" — a single trusted desktop app where the user's library lives, with the tablet as a synced satellite.

## 2. Goals and Non-Goals

### Goals
- Two-way sync between device and computer, over USB only.
- Full version history of every document, addressable by content hash.
- Ability to restore any previous version of a document, either back to the device or to a regular file on disk.
- One codebase that runs on Mac, Windows, and Linux.
- Privacy-respecting: no telemetry, no third-party servers, no cloud component.
- Honest handling of the device's actual data model — documents are trees of files, not single files, and the app must treat them that way.

### Non-Goals for the first version
- Rendering or previewing handwritten notes.
- OCR or full-text search inside notes.
- Editing notes on the desktop.
- Any cloud sync or remote access.
- Anything that would void the device's warranty (no firmware modification, no launcher mods).
- Mobile clients.

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

## 9. Cross-Platform Considerations

- **USB networking varies by OS.** On some platforms the device's network interface comes up automatically; on others it requires user acknowledgement or a driver step the first time. The first-run flow should detect this gracefully and provide platform-specific guidance, not a generic error.
- **Portable library.** The library is a single self-contained directory: blob store, version logs, metadata, sync state — everything the app needs lives inside it. The user can move that directory between machines (different OS included) and point the app at it from the new machine, and it just works. The app stores no library data anywhere outside this directory. The directory's location defaults to a sensible per-platform path on first run but is fully user-configurable. Never hardcode path separators. Do not assume case-sensitive or case-insensitive filesystem behavior — name internal files by hash, treat user-visible names as metadata.
- **Permissions.** The library directory contains personal documents. On platforms that support per-user permissions, default to user-only access.
- **Background behavior.** OS conventions for "running in the background watching for a device" differ — menu bar item on Mac, system tray on Windows, varies by environment on Linux. Respect each platform's norms. The app must also work as a strictly foreground app for users who don't want background services.
- **Installers and updates.** Each platform has its norms. Sign and notarize where applicable. Auto-update should be opt-in and the app must function fully offline; it should never need to phone home for routine use.
- **No platform-specific business logic.** All platform differences should be confined to a thin adapter layer. Sync, library, and version control behavior must be identical everywhere.

## 10. Phased Delivery

Each phase ends with the app in a state the user could actually rely on, even if features are still missing.

**Phase 1 — Read-only mirror.** Detect the device. List its contents. Pull everything to a local library, hashing as it goes, building the blob store and version log. No push, no restore. Already useful as a backup tool.

**Phase 2 — History and export.** Expose the version log in the UI. Let the user export any past version of any document as a regular file on disk. The app is now also a time machine.

**Phase 3 — Push and two-way sync.** Add upload. Add change detection on the library side. Add conflict handling, preserving losers in history. The app is now a full sync tool.

**Phase 4 — Library management.** Drag-and-drop import of PDFs and EPUBs. Folder management. Deletion with undo via version history. Garbage collection of unreferenced blobs.

**Phase 5 — Polish.** Signed installers for each platform. Background watching. Scheduled syncs. Operation log and diagnostics. Library integrity checks.

## 11. Testing Strategy

- Treat the device communication module as a swappable interface. Provide a fake backed by a local directory tree that mimics the device. Most of the app's logic can be tested against the fake without ever touching hardware.
- Property-test the blob store: any sequence of writes and reads must preserve content; any stored blob is retrievable by its hash; manifests reconstruct exactly.
- Scenario-test the sync engine end-to-end against the fake device: clean pull, clean push, both-side edit, deletion races, interrupted sync resumed, corrupted blob detected.
- Manual testing matrix on real hardware on each supported OS for each release. The fake covers logic; real hardware covers transport and platform quirks.
- Maintain a privacy invariant: an automated check that no build artifact attempts outbound network activity to anything other than the device. The expected count of external network destinations is, and stays, zero.

## 12. Deferred to v2

The following are explicitly out of scope for v1 and reserved for a future version. The implementation should not paint itself into a corner that makes them harder later, but should not implement them now either.

- **Encryption at rest** for the library. v1 relies on OS-level full-disk encryption for users who need it. The library directory itself is unencrypted. The data model should be designed such that wrapping blob and manifest reads/writes in an encryption layer later is straightforward.
- **Multi-computer sync against the same device.** v1 assumes one library, one device. Document this limitation visibly in the UI. The sync state model should not actively prevent this from being added later.
- **Note rendering, OCR, and full-text search.** Not in v1. The blob store keeps the raw files, so all of these become possible to add later without touching the storage layer.
- **Command-line interface.** v1 is GUI only. Internal modules should be cleanly separated from the UI, so that adding a CLI later is a matter of building a new entry point on top of existing logic, not rearchitecting.
- **Mobile clients.** Not in v1.

---

End of plan.
