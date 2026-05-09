import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ArchivedDocument,
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
  OcrProgressEvent,
  OcrStatusReport,
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
  switchLibraryViaDialog: () =>
    invoke<string | null>("switch_library_via_dialog"),
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
  garbageCollect: () => invoke<GarbageCollectReport>("garbage_collect"),
  getRecentLogs: (maxLines?: number) =>
    invoke<LogTail>("get_recent_logs", { maxLines: maxLines ?? null }),

  deviceState: () => invoke<DeviceState>("device_state"),
  saveDevicePassword: (password: string) =>
    invoke<void>("save_device_password", { password }),
  forgetDevicePassword: () => invoke<void>("forget_device_password"),
  connectDevice: (password?: string) =>
    invoke<DeviceInfo>("connect_device", { password: password ?? null }),
  disconnectDevice: () => invoke<void>("disconnect_device"),

  pullPlan: () => invoke<PullPlan>("pull_plan"),
  pullExecute: () => invoke<SyncReport>("pull_execute"),
  pushPlan: () => invoke<PushPlan>("push_plan"),
  pushExecute: () => invoke<PushReport>("push_execute"),
  syncTwoWay: () => invoke<TwoWayReport>("sync_two_way"),
  restoreVersion: (versionId: number) =>
    invoke<number>("restore_version", { versionId }),

  ocrStatus: () => invoke<OcrStatusReport>("ocr_status"),
  ocrDownloadDefaultModel: () =>
    invoke<void>("ocr_download_default_model"),
  transcribeDocument: (documentId: string, language?: string) =>
    invoke<TranscriptSummary>("transcribe_document", {
      documentId,
      language: language ?? null,
    }),
  getTranscript: (versionId: number) =>
    invoke<TranscriptDocument | null>("get_transcript", { versionId }),
  exportTranscript: (versionId: number, format: ExportFormat) =>
    invoke<{ path: string } | null>("export_transcript", {
      versionId,
      format,
    }),
  publishTranscript: (versionId: number, target: PublishKind) =>
    invoke<PublishResult>("publish_transcript", { versionId, target }),
  setGhostCredentials: (creds: GhostCredentials) =>
    invoke<void>("set_ghost_credentials", { creds }),
  forgetGhostCredentials: () => invoke<void>("forget_ghost_credentials"),
  setWordpressCredentials: (creds: WordpressCredentials) =>
    invoke<void>("set_wordpress_credentials", { creds }),
  forgetWordpressCredentials: () =>
    invoke<void>("forget_wordpress_credentials"),
  publishCredentialStatus: () =>
    invoke<PublishCredentialStatus>("publish_credential_status"),
  pingPublishTarget: (target: PublishKind) =>
    invoke<void>("ping_publish_target", { target }),
};

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

export function onOcrProgress(
  cb: (event: OcrProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<OcrProgressEvent>("ocr:progress", (e) => cb(e.payload));
}
