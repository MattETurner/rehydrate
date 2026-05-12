import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ArchivedDocument,
  CuratedOllamaModel,
  DeviceInfo,
  DeviceState,
  DocumentSummary,
  ExportFormat,
  ExportResult,
  FolderEntry,
  GarbageCollectReport,
  GhostCredentials,
  LibrarySummary,
  LogTail,
  OcrCandidate,
  OcrProgressEvent,
  OcrStatusReport,
  OllamaConfig,
  PickedLibraryDirectory,
  PingReport,
  ProgressEvent,
  PublishCredentialStatus,
  PublishKind,
  PublishResult,
  PullPlan,
  PushPlan,
  PushReport,
  RecentLibraryEntry,
  SyncReport,
  TranscriptDocument,
  TranscriptSummary,
  TwoWayReport,
  VerifyReport,
  VersionEntry,
  WordpressCredentials,
} from "./types";

export const ipc = {
  ping: () => invoke<string>("ping"),

  defaultLibraryPath: () => invoke<string | null>("default_library_path"),
  openLibrary: (path: string) => invoke<void>("open_library", { path }),
  autoOpenLibrary: () => invoke<string | null>("auto_open_library"),
  switchLibrary: (path: string) => invoke<string>("switch_library", { path }),
  pickLibraryDirectory: () =>
    invoke<PickedLibraryDirectory | null>("pick_library_directory"),
  listRecentLibraries: () =>
    invoke<RecentLibraryEntry[]>("list_recent_libraries"),
  librarySummary: () => invoke<LibrarySummary>("library_summary"),
  listDocuments: () => invoke<DocumentSummary[]>("list_documents"),
  listFolders: () => invoke<FolderEntry[]>("list_folders"),
  listArchived: () => invoke<ArchivedDocument[]>("list_archived"),
  moveDocument: (documentId: string, parentId: string | null) =>
    invoke<void>("move_document", { documentId, parentId }),
  renameDocument: (documentId: string, newName: string) =>
    invoke<void>("rename_document", { documentId, newName }),
  renameFolder: (folderId: string, newName: string) =>
    invoke<void>("rename_folder", { folderId, newName }),
  createFolder: (visibleName: string, parentId: string | null) =>
    invoke<FolderEntry>("create_folder", { visibleName, parentId }),
  reorderFolder: (
    folderId: string,
    newParent: string | null,
    newSortIndex: number,
  ) =>
    invoke<void>("reorder_folder", { folderId, newParent, newSortIndex }),
  archiveDocument: (documentId: string) =>
    invoke<void>("archive_document", { documentId }),
  unarchiveDocument: (documentId: string) =>
    invoke<DocumentSummary>("unarchive_document", { documentId }),
  purgeArchivedDocument: (documentId: string) =>
    invoke<void>("purge_archived_document", { documentId }),
  openDocument: (documentId: string) =>
    invoke<string>("open_document", { documentId }),
  documentThumbnail: (documentId: string) =>
    invoke<string | null>("document_thumbnail", { documentId }),
  getHistory: (documentId: string) =>
    invoke<VersionEntry[]>("get_history", { documentId }),
  setVersionNote: (versionId: number, note: string | null) =>
    invoke<void>("set_version_note", { versionId, note }),
  exportVersion: (versionId: number) =>
    invoke<ExportResult | null>("export_version", { versionId }),
  verifyLibrary: () => invoke<VerifyReport>("verify_library"),
  importFile: () => invoke<DocumentSummary | null>("import_file"),
  importDroppedFile: (fileName: string, bytes: Uint8Array) =>
    // Tauri 2's JSON IPC marshals a plain `number[]` straight into
    // `Vec<u8>` on the Rust side. We pay a per-byte JSON serialisation
    // cost (a few hundred ms for a 100 MB PDF) but avoid pulling in a
    // base64 dependency on either side. If this ever shows up in a
    // profile, switch to a `tauri::ipc::Channel<Vec<u8>>` for a true
    // streaming path.
    invoke<DocumentSummary>("import_dropped_file", {
      fileName,
      bytes: Array.from(bytes),
    }),
  garbageCollect: () => invoke<GarbageCollectReport>("garbage_collect"),
  getRecentLogs: (maxLines?: number) =>
    invoke<LogTail>("get_recent_logs", { maxLines: maxLines ?? null }),

  deviceState: () => invoke<DeviceState>("device_state"),
  saveDevicePassword: (password: string) =>
    invoke<void>("save_device_password", { password }),
  forgetDevicePassword: () => invoke<void>("forget_device_password"),
  connectDevice: (password?: string, remember?: boolean) =>
    invoke<DeviceInfo>("connect_device", {
      password: password ?? null,
      // The backend defaults to *not* persisting unless the renderer
      // explicitly asks. `undefined` ⇒ null on the wire ⇒ no save.
      remember: remember ?? null,
    }),
  disconnectDevice: () => invoke<void>("disconnect_device"),

  pullPlan: () => invoke<PullPlan>("pull_plan"),
  pullExecute: () => invoke<SyncReport>("pull_execute"),
  pushPlan: () => invoke<PushPlan>("push_plan"),
  pushExecute: () => invoke<PushReport>("push_execute"),
  syncTwoWay: () => invoke<TwoWayReport>("sync_two_way"),
  restoreVersion: (versionId: number) =>
    invoke<number>("restore_version", { versionId }),

  // ---- OCR / Ollama ---------------------------------------------------
  ocrStatus: () => invoke<OcrStatusReport>("ocr_status"),
  transcribeDocument: (documentId: string, language: string | null) =>
    invoke<TranscriptSummary>("transcribe_document", {
      documentId,
      language,
    }),
  getTranscript: (versionId: number) =>
    invoke<TranscriptDocument | null>("get_transcript", { versionId }),
  exportTranscript: (versionId: number, format: ExportFormat) =>
    invoke<{ path: string } | null>("export_transcript", {
      versionId,
      format,
    }),
  getOllamaConfig: () => invoke<OllamaConfig>("get_ollama_config"),
  saveOllamaConfig: (cfg: OllamaConfig) =>
    invoke<void>("save_ollama_config", { cfg }),
  pingOllama: (baseUrl: string) =>
    invoke<PingReport>("ping_ollama", { baseUrl }),
  listCuratedOllamaModels: () =>
    invoke<CuratedOllamaModel[]>("list_curated_ollama_models"),
  defaultOllamaModel: () => invoke<string>("default_ollama_model"),
  listDocumentsNeedingOcr: () =>
    invoke<OcrCandidate[]>("list_documents_needing_ocr"),

  // ---- CMS publish ----------------------------------------------------
  publishTranscript: (versionId: number, target: PublishKind) =>
    invoke<PublishResult>("publish_transcript", { versionId, target }),
  publishCredentialStatus: () =>
    invoke<PublishCredentialStatus>("publish_credential_status"),
  pingPublishTarget: (target: PublishKind) =>
    invoke<void>("ping_publish_target", { target }),
  setGhostCredentials: (creds: GhostCredentials) =>
    invoke<void>("set_ghost_credentials", { creds }),
  forgetGhostCredentials: () => invoke<void>("forget_ghost_credentials"),
  setWordpressCredentials: (creds: WordpressCredentials) =>
    invoke<void>("set_wordpress_credentials", { creds }),
  forgetWordpressCredentials: () =>
    invoke<void>("forget_wordpress_credentials"),
};

/** Subscribe to OCR-progress events emitted from the Rust side. */
export function onOcrProgress(
  cb: (ev: OcrProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<OcrProgressEvent>("ocr:progress", (e) => cb(e.payload));
}

export function onDeviceReachable(
  cb: (reachable: boolean) => void,
): Promise<UnlistenFn> {
  return listen<boolean>("device:reachable", (e) => cb(e.payload));
}

export function onSyncProgress(
  cb: (event: ProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<ProgressEvent>("sync:progress", (e) => cb(e.payload));
}

export function onSyncPhase(
  cb: (phase: "pull" | "push") => void,
): Promise<UnlistenFn> {
  return listen<"pull" | "push">("sync:phase", (e) => cb(e.payload));
}

export function onKeyringWarning(
  cb: (message: string) => void,
): Promise<UnlistenFn> {
  return listen<string>("keyring:warning", (e) => cb(e.payload));
}

export function onLegacyFormatWarning(
  cb: (message: string) => void,
): Promise<UnlistenFn> {
  return listen<string>("document:legacy-format-warning", (e) =>
    cb(e.payload),
  );
}
