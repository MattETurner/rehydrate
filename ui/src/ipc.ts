import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ArchivedDocument,
  DeviceInfo,
  DeviceState,
  DocumentSummary,
  ExportResult,
  FolderEntry,
  GarbageCollectReport,
  LibrarySummary,
  LogTail,
  ProgressEvent,
  PullPlan,
  PushPlan,
  PushReport,
  SyncReport,
  TwoWayReport,
  VerifyReport,
  VersionEntry,
} from "./types";

export const ipc = {
  ping: () => invoke<string>("ping"),

  defaultLibraryPath: () => invoke<string | null>("default_library_path"),
  openLibrary: (path: string) => invoke<void>("open_library", { path }),
  autoOpenLibrary: () => invoke<string | null>("auto_open_library"),
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
  exportVersion: (versionId: number, destDir: string) =>
    invoke<ExportResult>("export_version", { versionId, destDir }),
  verifyLibrary: () => invoke<VerifyReport>("verify_library"),
  importFile: (path: string) => invoke<DocumentSummary>("import_file", { path }),
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
