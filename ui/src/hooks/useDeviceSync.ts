// Device connection state + the three actions that mutate it,
// extracted from App.tsx.
//
// `device: DeviceState` carries reachability + connected + stored-
// password flags + the cached `DeviceInfo` block. The hook keeps it
// in sync with three sources:
//
//   - The Tauri `device:reachable` event (`onDeviceReachable`). Fires
//     whenever the OS sees the USB-ethernet endpoint at 10.11.99.1
//     appear or disappear; we refetch the full state to pick up
//     side effects (e.g. an auto-cleared `connected` after unplug).
//   - The explicit `tryConnect` / `disconnect` / `submitPassword`
//     actions wired to the StatusPill / PasswordDialog.
//   - Whatever the parent component does — e.g. the initial-load
//     `deviceState()` fetch in App.tsx still uses `setDevice`.
//
// The hook deliberately does NOT own the initial-load fetch; App
// batches that with the library + recent-libraries hydrate so all
// three land before the first render commits.

import { useCallback, useEffect, useState } from "react";

import { formatError } from "../formatError";
import { ipc, onDeviceReachable } from "../ipc";
import type { DeviceState } from "../types";

export interface UseDeviceSyncInjections {
  /// App-level error sink. `tryConnect` routes failures through it.
  setError: (msg: string | null) => void;
  /// Triggers the PasswordDialog when the user clicks Connect and we
  /// have no stored password yet.
  openPasswordDialog: () => void;
  /// Lets the password-dialog success path dismiss itself.
  closePasswordDialog: () => void;
}

export interface UseDeviceSyncResult {
  device: DeviceState | null;
  setDevice: React.Dispatch<React.SetStateAction<DeviceState | null>>;
  tryConnect: () => Promise<void>;
  disconnect: () => Promise<void>;
  submitPassword: (password: string, remember: boolean) => Promise<void>;
}

export function useDeviceSync(
  injections: UseDeviceSyncInjections,
): UseDeviceSyncResult {
  const { setError, openPasswordDialog, closePasswordDialog } = injections;
  const [device, setDevice] = useState<DeviceState | null>(null);

  // Single subscription to the Tauri-emitted reachability event.
  // The payload itself isn't strictly necessary — we just refetch
  // the full `DeviceState` because the event also flips connected /
  // disconnected as a side effect of the watcher.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    onDeviceReachable(() => {
      ipc
        .deviceState()
        .then(setDevice)
        .catch(() => {
          /* deviceState is best-effort; a transient failure is fine */
        });
    }).then((u) => {
      unlisten = u;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  const tryConnect = useCallback(async () => {
    setError(null);
    try {
      if (device?.has_stored_password) {
        await ipc.connectDevice();
        setDevice(await ipc.deviceState());
      } else {
        openPasswordDialog();
      }
    } catch (e) {
      setError(formatError(e));
    }
  }, [device?.has_stored_password, openPasswordDialog, setError]);

  const disconnect = useCallback(async () => {
    await ipc.disconnectDevice();
    setDevice(await ipc.deviceState());
  }, []);

  const submitPassword = useCallback(
    async (password: string, remember: boolean) => {
      await ipc.connectDevice(password, remember);
      setDevice(await ipc.deviceState());
      closePasswordDialog();
    },
    [closePasswordDialog],
  );

  return { device, setDevice, tryConnect, disconnect, submitPassword };
}
