import { useCallback, useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ipc, onDeviceReachable } from "./ipc";
import { HistoryDrawer } from "./components/HistoryDrawer";
import { LogDrawer } from "./components/LogDrawer";
import { PasswordDialog } from "./components/PasswordDialog";
import { StatusPill } from "./components/StatusPill";
import { SyncDrawer } from "./components/SyncDrawer";
import type { DeviceState, DocumentSummary, LibrarySummary } from "./types";

export function App() {
  const [defaultPath, setDefaultPath] = useState<string | null>(null);
  const [libraryOpen, setLibraryOpen] = useState(false);
  const [summary, setSummary] = useState<LibrarySummary | null>(null);
  const [documents, setDocuments] = useState<DocumentSummary[]>([]);
  const [device, setDevice] = useState<DeviceState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showPassword, setShowPassword] = useState(false);
  const [showSync, setShowSync] = useState(false);
  const [historyDoc, setHistoryDoc] = useState<DocumentSummary | null>(null);
  const [showLogs, setShowLogs] = useState(false);
  const refreshing = useRef(false);

  // Initial load. Auto-open the previously-used library if there was one;
  // fall back to the welcome screen otherwise.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [defPath, opened, dev] = await Promise.all([
          ipc.defaultLibraryPath(),
          ipc.autoOpenLibrary(),
          ipc.deviceState(),
        ]);
        if (cancelled) return;
        setDefaultPath(defPath);
        setDevice(dev);
        if (opened) {
          setLibraryOpen(true);
          const [s, d] = await Promise.all([ipc.librarySummary(), ipc.listDocuments()]);
          if (cancelled) return;
          setSummary(s);
          setDocuments(d);
        }
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Reachability event.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    onDeviceReachable(() => {
      ipc.deviceState().then(setDevice).catch(() => {});
    }).then((u) => {
      unlisten = u;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  const refreshLibrary = useCallback(async () => {
    if (refreshing.current) return;
    refreshing.current = true;
    try {
      const [s, d] = await Promise.all([ipc.librarySummary(), ipc.listDocuments()]);
      setSummary(s);
      setDocuments(d);
    } catch (e) {
      setError(String(e));
    } finally {
      refreshing.current = false;
    }
  }, []);

  async function openLibrary() {
    if (!defaultPath) return;
    setError(null);
    try {
      await ipc.openLibrary(defaultPath);
      setLibraryOpen(true);
      await refreshLibrary();
    } catch (e) {
      setError(String(e));
    }
  }

  async function tryConnect() {
    setError(null);
    try {
      if (device?.has_stored_password) {
        await ipc.connectDevice();
        setDevice(await ipc.deviceState());
      } else {
        setShowPassword(true);
      }
    } catch (e) {
      setError(String(e));
    }
  }

  async function disconnect() {
    await ipc.disconnectDevice();
    setDevice(await ipc.deviceState());
  }

  async function submitPassword(password: string) {
    await ipc.connectDevice(password);
    setDevice(await ipc.deviceState());
    setShowPassword(false);
  }

  function openSync() {
    if (!device?.connected) {
      setError("Connect the device first.");
      return;
    }
    if (!libraryOpen) {
      setError("Open a library first.");
      return;
    }
    setError(null);
    setShowSync(true);
  }

  async function onSyncComplete() {
    await refreshLibrary();
  }

  async function importFile() {
    setError(null);
    try {
      const picked = await openDialog({
        multiple: false,
        directory: false,
        filters: [{ name: "Documents", extensions: ["pdf", "epub"] }],
        title: "Import a PDF or EPUB",
      });
      if (!picked || typeof picked !== "string") return;
      const summary = await ipc.importFile(picked);
      await refreshLibrary();
      window.alert(
        `Imported "${summary.visible_name}".\n\n` +
          `It will be uploaded to the reMarkable on the next sync.`,
      );
    } catch (e) {
      setError(String(e));
    }
  }

  async function garbageCollect() {
    setError(null);
    try {
      const r = await ipc.garbageCollect();
      await refreshLibrary();
      window.alert(
        r.deleted === 0
          ? `Garbage collection scanned ${r.scanned} blobs; nothing to remove.`
          : `Removed ${r.deleted} unreferenced blob${r.deleted === 1 ? "" : "s"} (${formatBytes(r.bytes_freed)} freed).`,
      );
    } catch (e) {
      setError(String(e));
    }
  }

  async function verify() {
    setError(null);
    try {
      const r = await ipc.verifyLibrary();
      const issues =
        r.manifests_missing + r.manifests_invalid + r.blobs_missing + r.blobs_corrupted;
      const summary =
        issues === 0
          ? `Library is healthy. ${r.manifests_ok} manifests, ${r.blobs_total} blobs, ${r.blobs_orphan} orphans.`
          : `Library has ${issues} issue${issues === 1 ? "" : "s"}: ` +
            `${r.manifests_missing} missing manifests, ${r.blobs_missing} missing blobs, ` +
            `${r.blobs_corrupted} corrupted blobs. Examples: ${[
              ...r.missing_examples,
              ...r.orphan_examples,
            ]
              .slice(0, 3)
              .join("; ")}`;
      setError(null);
      // Reuse the error banner for the message; styled green when healthy.
      // (Quick-and-dirty for now; a proper toast/notification system is
      // future work.)
      window.alert(summary);
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="app">
      <header className="toolbar">
        <div className="brand">Marginalia</div>
        <StatusPill state={device} />
        <div className="spacer" />
        {!libraryOpen ? (
          <button onClick={openLibrary} disabled={!defaultPath}>
            Open library
          </button>
        ) : (
          <>
            <button onClick={importFile}>Import</button>
            <button onClick={openSync} disabled={!device?.connected}>
              Sync
            </button>
          </>
        )}
        {device?.connected ? (
          <button onClick={disconnect}>Disconnect</button>
        ) : (
          <button onClick={tryConnect} disabled={!device?.reachable}>
            Connect
          </button>
        )}
      </header>

      <div className="layout">
        <aside className="sidebar">
          <section>
            <h3>Library</h3>
            <ul>
              <li className="active">All Documents</li>
              <li>Recently Synced</li>
            </ul>
          </section>
          <section>
            <h3>Device</h3>
            <ul>
              {device?.connected && device.info ? (
                <li>
                  {device.info.model}
                  <div className="muted small">
                    {device.info.serial && `S/N ${device.info.serial}`}
                  </div>
                </li>
              ) : (
                <li className="muted">
                  {device?.reachable ? "Detected, not connected" : "No device"}
                </li>
              )}
            </ul>
          </section>
          <section>
            <h3>History</h3>
            <ul>
              <li>All Versions</li>
              <li>Conflicts</li>
            </ul>
          </section>
        </aside>

        <main className="content">
          {error && (
            <div className="error">
              {error}
              <button onClick={() => setError(null)} className="close-inline">
                ×
              </button>
            </div>
          )}
          {!libraryOpen ? (
            <div className="empty">
              <h2>Welcome</h2>
              <p>
                Marginalia mirrors your reMarkable 2 to a local
                content-addressed library. No cloud, no telemetry.
              </p>
              {defaultPath && (
                <p className="path">
                  Default library: <code>{defaultPath}</code>
                </p>
              )}
              <p>
                <button onClick={openLibrary} className="primary">
                  Open default library
                </button>
              </p>
            </div>
          ) : (
            <DocumentList documents={documents} onOpen={setHistoryDoc} />
          )}
        </main>
      </div>

      <footer className="statusbar">
        {summary ? (
          <>
            <span>{summary.document_count} documents</span>
            <span>{summary.version_count} versions</span>
            <span>{summary.blob_count} blobs</span>
            <span>{formatBytes(summary.size_bytes)} on disk</span>
            <span className="spacer" />
            <button onClick={verify} className="link">
              Verify
            </button>
            <button onClick={garbageCollect} className="link">
              Garbage-collect
            </button>
            <button onClick={() => setShowLogs(true)} className="link">
              Logs
            </button>
          </>
        ) : (
          <span className="muted">Library not open</span>
        )}
      </footer>

      {showPassword && (
        <PasswordDialog
          onCancel={() => setShowPassword(false)}
          onSubmit={submitPassword}
        />
      )}
      {showSync && (
        <div className="drawer-backdrop" onClick={() => setShowSync(false)}>
          <SyncDrawer
            onClose={() => setShowSync(false)}
            onComplete={onSyncComplete}
          />
        </div>
      )}
      {historyDoc && (
        <div className="drawer-backdrop" onClick={() => setHistoryDoc(null)}>
          <HistoryDrawer document={historyDoc} onClose={() => setHistoryDoc(null)} />
        </div>
      )}
      {showLogs && (
        <div className="drawer-backdrop" onClick={() => setShowLogs(false)}>
          <LogDrawer onClose={() => setShowLogs(false)} />
        </div>
      )}
    </div>
  );
}

function DocumentList({
  documents,
  onOpen,
}: {
  documents: DocumentSummary[];
  onOpen: (d: DocumentSummary) => void;
}) {
  if (documents.length === 0) {
    return (
      <div className="empty">
        <h2>No documents yet</h2>
        <p>Connect a reMarkable and sync to populate the library.</p>
      </div>
    );
  }
  return (
    <table className="docs">
      <thead>
        <tr>
          <th>Title</th>
          <th>Type</th>
          <th>Manifest</th>
          <th>Version</th>
        </tr>
      </thead>
      <tbody>
        {documents.map((d) => (
          <tr
            key={d.document_id}
            onClick={() => onOpen(d)}
            className="clickable"
            title="Open history"
          >
            <td>{d.visible_name}</td>
            <td className="muted">{d.doc_type}</td>
            <td className="mono">{d.current_manifest.slice(0, 12)}</td>
            <td className="muted">{d.current_version_id}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}
