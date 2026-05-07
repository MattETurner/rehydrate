import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  DeviceInfo,
  DeviceState,
  DocumentSummary,
  LibrarySummary,
  ProgressEvent,
  PullPlan,
  SyncReport,
  VersionEntry,
} from "./types";

export const ipc = {
  ping: () => invoke<string>("ping"),

  defaultLibraryPath: () => invoke<string | null>("default_library_path"),
  openLibrary: (path: string) => invoke<void>("open_library", { path }),
  librarySummary: () => invoke<LibrarySummary>("library_summary"),
  listDocuments: () => invoke<DocumentSummary[]>("list_documents"),
  getHistory: (documentId: string) =>
    invoke<VersionEntry[]>("get_history", { documentId }),

  deviceState: () => invoke<DeviceState>("device_state"),
  saveDevicePassword: (password: string) =>
    invoke<void>("save_device_password", { password }),
  forgetDevicePassword: () => invoke<void>("forget_device_password"),
  connectDevice: (password?: string) =>
    invoke<DeviceInfo>("connect_device", { password: password ?? null }),
  disconnectDevice: () => invoke<void>("disconnect_device"),

  pullPlan: () => invoke<PullPlan>("pull_plan"),
  pullExecute: () => invoke<SyncReport>("pull_execute"),
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
