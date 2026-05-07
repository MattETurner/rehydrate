import { useCallback, useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ipc, onDeviceReachable } from "./ipc";
import { HistoryDrawer } from "./components/HistoryDrawer";
import { LogDrawer } from "./components/LogDrawer";
import { PasswordDialog } from "./components/PasswordDialog";
import { StatusPill } from "./components/StatusPill";
import { SyncDrawer } from "./components/SyncDrawer";
import type { DeviceState, DocumentSummary, FolderEntry, LibrarySummary } from "./types";

export function App() {
  const [defaultPath, setDefaultPath] = useState<string | null>(null);
  const [libraryOpen, setLibraryOpen] = useState(false);
  const [summary, setSummary] = useState<LibrarySummary | null>(null);
  const [documents, setDocuments] = useState<DocumentSummary[]>([]);
  const [folders, setFolders] = useState<FolderEntry[]>([]);
  const [device, setDevice] = useState<DeviceState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showPassword, setShowPassword] = useState(false);
  const [showSync, setShowSync] = useState(false);
  const [historyDoc, setHistoryDoc] = useState<DocumentSummary | null>(null);
  const [showLogs, setShowLogs] = useState(false);
  const [view, setView] = useState<View>("all");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
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
          const [s, d, f] = await Promise.all([
            ipc.librarySummary(),
            ipc.listDocuments(),
            ipc.listFolders(),
          ]);
          if (cancelled) return;
          setSummary(s);
          setDocuments(d);
          setFolders(f);
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
      const [s, d, f] = await Promise.all([
        ipc.librarySummary(),
        ipc.listDocuments(),
        ipc.listFolders(),
      ]);
      setSummary(s);
      setDocuments(d);
      setFolders(f);
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

  async function openInViewer(d: DocumentSummary) {
    setError(null);
    try {
      await ipc.openDocument(d.document_id);
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
              <SidebarItem label="All Documents" v="all" view={view} setView={setView} count={documents.length} />
              <SidebarItem label="Recently Synced" v="recent" view={view} setView={setView} count={recentCount(documents)} />
              <SidebarItem label="Notebooks" v="notebooks" view={view} setView={setView} count={countByKind(documents, "Notebook")} />
              <SidebarItem label="PDFs" v="pdfs" view={view} setView={setView} count={countByKind(documents, "DocumentType.Pdf")} />
              <SidebarItem label="EPUBs" v="epubs" view={view} setView={setView} count={countByKind(documents, "DocumentType.Epub")} />
            </ul>
          </section>
          {folders.length > 0 && (
            <section>
              <h3>Folders</h3>
              <FolderTree
                folders={folders}
                documents={documents}
                view={view}
                setView={setView}
                expanded={expanded}
                setExpanded={setExpanded}
              />
            </section>
          )}
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
            <DocumentList
              documents={filterDocuments(documents, view)}
              onOpen={setHistoryDoc}
              onOpenInViewer={openInViewer}
              emptyHint={emptyHintFor(view)}
            />
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

type View =
  | "all"
  | "recent"
  | "notebooks"
  | "pdfs"
  | "epubs"
  | { kind: "folder"; id: string };

function viewKey(v: View): string {
  if (typeof v === "string") return v;
  return `folder:${v.id}`;
}

const RECENT_WINDOW_MS = 24 * 60 * 60 * 1000;

function recentCount(docs: DocumentSummary[]): number {
  return filterRecent(docs).length;
}

function countByKind(docs: DocumentSummary[], kindMatch: string): number {
  return docs.filter((d) => d.doc_type === kindMatch).length;
}

function filterRecent(docs: DocumentSummary[]): DocumentSummary[] {
  const cutoff = Date.now() - RECENT_WINDOW_MS;
  return docs
    .filter((d) => {
      const t = Date.parse(d.last_observed_at);
      return Number.isFinite(t) && t >= cutoff;
    })
    .sort((a, b) => b.last_observed_at.localeCompare(a.last_observed_at));
}

function filterDocuments(docs: DocumentSummary[], view: View): DocumentSummary[] {
  if (typeof view === "object") {
    return docs.filter((d) => d.parent === view.id);
  }
  switch (view) {
    case "all":
      return docs;
    case "recent":
      return filterRecent(docs);
    case "notebooks":
      return docs.filter((d) => d.doc_type === "Notebook");
    case "pdfs":
      return docs.filter((d) => d.doc_type === "DocumentType.Pdf");
    case "epubs":
      return docs.filter((d) => d.doc_type === "DocumentType.Epub");
  }
}

function emptyHintFor(view: View): { title: string; body: string } {
  if (typeof view === "object") {
    return {
      title: "Empty folder",
      body: "Documents in this folder will appear here once they sync.",
    };
  }
  switch (view) {
    case "all":
      return {
        title: "No documents yet",
        body: "Connect a reMarkable and sync to populate the library.",
      };
    case "recent":
      return {
        title: "Nothing synced recently",
        body: "Documents synced in the last 24 hours appear here.",
      };
    case "notebooks":
      return {
        title: "No notebooks",
        body: "reMarkable notebooks (handwritten or imported templates) appear here.",
      };
    case "pdfs":
      return {
        title: "No PDFs",
        body: "Drop a PDF on the Import button or sync one from the device to see it here.",
      };
    case "epubs":
      return {
        title: "No EPUBs",
        body: "Drop an EPUB on the Import button or sync one from the device to see it here.",
      };
  }
}

function SidebarItem({
  label,
  v,
  view,
  setView,
  count,
}: {
  label: string;
  v: View;
  view: View;
  setView: (v: View) => void;
  count: number;
}) {
  return (
    <li
      className={viewKey(view) === viewKey(v) ? "active" : ""}
      onClick={() => setView(v)}
      title={`${count} document${count === 1 ? "" : "s"}`}
    >
      <span>{label}</span>
      <span className="sidebar-count">{count}</span>
    </li>
  );
}

interface FolderTreeNode {
  folder: FolderEntry;
  children: FolderTreeNode[];
  docCount: number;
}

function buildFolderTree(
  folders: FolderEntry[],
  docs: DocumentSummary[],
): FolderTreeNode[] {
  const docCounts = new Map<string, number>();
  for (const d of docs) {
    if (d.parent) docCounts.set(d.parent, (docCounts.get(d.parent) ?? 0) + 1);
  }
  const byId = new Map<string, FolderTreeNode>();
  for (const f of folders) {
    byId.set(f.folder_id, {
      folder: f,
      children: [],
      docCount: docCounts.get(f.folder_id) ?? 0,
    });
  }
  const roots: FolderTreeNode[] = [];
  for (const node of byId.values()) {
    const parentId = node.folder.parent;
    if (parentId && byId.has(parentId)) {
      byId.get(parentId)!.children.push(node);
    } else {
      roots.push(node);
    }
  }
  // Sort siblings by visible_name.
  const sortRec = (nodes: FolderTreeNode[]) => {
    nodes.sort((a, b) =>
      a.folder.visible_name.localeCompare(b.folder.visible_name),
    );
    for (const n of nodes) sortRec(n.children);
  };
  sortRec(roots);
  return roots;
}

function FolderTree({
  folders,
  documents,
  view,
  setView,
  expanded,
  setExpanded,
}: {
  folders: FolderEntry[];
  documents: DocumentSummary[];
  view: View;
  setView: (v: View) => void;
  expanded: Set<string>;
  setExpanded: (s: Set<string>) => void;
}) {
  const roots = buildFolderTree(folders, documents);
  const toggle = (id: string) => {
    const next = new Set(expanded);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setExpanded(next);
  };
  return (
    <ul>
      {roots.map((node) => (
        <FolderRow
          key={node.folder.folder_id}
          node={node}
          depth={0}
          view={view}
          setView={setView}
          expanded={expanded}
          toggle={toggle}
        />
      ))}
    </ul>
  );
}

function FolderRow({
  node,
  depth,
  view,
  setView,
  expanded,
  toggle,
}: {
  node: FolderTreeNode;
  depth: number;
  view: View;
  setView: (v: View) => void;
  expanded: Set<string>;
  toggle: (id: string) => void;
}) {
  const v: View = { kind: "folder", id: node.folder.folder_id };
  const isActive = viewKey(view) === viewKey(v);
  const hasChildren = node.children.length > 0;
  const isOpen = expanded.has(node.folder.folder_id);
  return (
    <>
      <li
        className={isActive ? "active folder-row" : "folder-row"}
        onClick={() => setView(v)}
        style={{ paddingLeft: 24 + depth * 12 }}
        title={`${node.docCount} document${node.docCount === 1 ? "" : "s"}`}
      >
        <span
          className="folder-icon"
          onClick={(e) => {
            if (!hasChildren) return;
            e.stopPropagation();
            toggle(node.folder.folder_id);
          }}
          role={hasChildren ? "button" : undefined}
          aria-label={hasChildren ? (isOpen ? "Collapse" : "Expand") : undefined}
          style={hasChildren ? { cursor: "pointer" } : undefined}
        >
          {hasChildren ? (isOpen ? "▾" : "▸") : "·"}
        </span>
        <span className="folder-name">{node.folder.visible_name}</span>
        {node.docCount > 0 && (
          <span className="sidebar-count">{node.docCount}</span>
        )}
      </li>
      {hasChildren && isOpen &&
        node.children.map((child) => (
          <FolderRow
            key={child.folder.folder_id}
            node={child}
            depth={depth + 1}
            view={view}
            setView={setView}
            expanded={expanded}
            toggle={toggle}
          />
        ))}
    </>
  );
}

function DocumentList({
  documents,
  onOpen,
  onOpenInViewer,
  emptyHint,
}: {
  documents: DocumentSummary[];
  onOpen: (d: DocumentSummary) => void;
  onOpenInViewer: (d: DocumentSummary) => void;
  emptyHint: { title: string; body: string };
}) {
  // A single click opens history; a double-click opens the file in the
  // OS viewer. Browsers fire onClick *before* onDoubleClick, so without
  // this delay the history drawer pops up on the first click and you
  // never get the open-in-viewer behaviour. 250ms matches the macOS
  // double-click threshold closely enough to feel natural.
  const clickTimerRef = useRef<number | null>(null);
  const handleClick = (d: DocumentSummary) => {
    if (clickTimerRef.current !== null) {
      window.clearTimeout(clickTimerRef.current);
    }
    clickTimerRef.current = window.setTimeout(() => {
      clickTimerRef.current = null;
      onOpen(d);
    }, 250);
  };
  const handleDoubleClick = (d: DocumentSummary) => {
    if (clickTimerRef.current !== null) {
      window.clearTimeout(clickTimerRef.current);
      clickTimerRef.current = null;
    }
    onOpenInViewer(d);
  };

  if (documents.length === 0) {
    return (
      <div className="empty">
        <h2>{emptyHint.title}</h2>
        <p>{emptyHint.body}</p>
      </div>
    );
  }
  return (
    <table className="docs">
      <thead>
        <tr>
          <th>Title</th>
          <th>Type</th>
          <th>Synced</th>
          <th>Manifest</th>
        </tr>
      </thead>
      <tbody>
        {documents.map((d) => (
          <tr
            key={d.document_id}
            onClick={() => handleClick(d)}
            onDoubleClick={() => handleDoubleClick(d)}
            className="clickable"
            title="Click to view history · double-click to open"
          >
            <td>{d.visible_name}</td>
            <td className="muted">{prettyType(d.doc_type)}</td>
            <td className="muted">{prettyDate(d.last_observed_at)}</td>
            <td className="mono">{d.current_manifest.slice(0, 12)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function prettyType(s: string): string {
  if (s === "Notebook") return "Notebook";
  if (s === "DocumentType.Pdf") return "PDF";
  if (s === "DocumentType.Epub") return "EPUB";
  return s;
}

function prettyDate(iso: string): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const now = Date.now();
  const diffMs = now - d.getTime();
  const minutes = Math.floor(diffMs / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  return d.toLocaleDateString();
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
