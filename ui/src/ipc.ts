import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  DeviceInfo,
  DeviceState,
  DocumentSummary,
  ExportResult,
  GarbageCollectReport,
  LibrarySummary,
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
  getHistory: (documentId: string) =>
    invoke<VersionEntry[]>("get_history", { documentId }),
  setVersionNote: (versionId: number, note: string | null) =>
    invoke<void>("set_version_note", { versionId, note }),
  exportVersion: (versionId: number, destDir: string) =>
    invoke<ExportResult>("export_version", { versionId, destDir }),
  verifyLibrary: () => invoke<VerifyReport>("verify_library"),
  importFile: (path: string) => invoke<DocumentSummary>("import_file", { path }),
  garbageCollect: () => invoke<GarbageCollectReport>("garbage_collect"),

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
