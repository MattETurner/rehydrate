import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DocumentSummary, LibrarySummary } from "./types";

export function App() {
  const [defaultPath, setDefaultPath] = useState<string | null>(null);
  const [opened, setOpened] = useState(false);
  const [summary, setSummary] = useState<LibrarySummary | null>(null);
  const [documents, setDocuments] = useState<DocumentSummary[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<string | null>("default_library_path").then(setDefaultPath).catch(() => {});
  }, []);

  async function openLibrary() {
    if (!defaultPath) return;
    setError(null);
    try {
      await invoke("open_library", { path: defaultPath });
      const s = await invoke<LibrarySummary>("library_summary");
      const d = await invoke<DocumentSummary[]>("list_documents");
      setSummary(s);
      setDocuments(d);
      setOpened(true);
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="app">
      <header className="toolbar">
        <div className="brand">Marginalia</div>
        <div className="status">
          {opened ? (
            <span className="pill pill-ok">Library open</span>
          ) : (
            <span className="pill">No library</span>
          )}
        </div>
        <div className="spacer" />
        <button onClick={openLibrary} disabled={!defaultPath}>
          Open default library
        </button>
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
              <li className="muted">No device connected</li>
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
          {error && <div className="error">{error}</div>}
          {!opened ? (
            <div className="empty">
              <h2>Welcome</h2>
              <p>
                Marginalia mirrors your reMarkable 2 to a local content-addressed
                library. No cloud, no telemetry. Open a library to get started.
              </p>
              {defaultPath && (
                <p className="path">
                  Default library: <code>{defaultPath}</code>
                </p>
              )}
            </div>
          ) : (
            <DocumentList documents={documents} />
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
          </>
        ) : (
          <span className="muted">Library not open</span>
        )}
      </footer>
    </div>
  );
}

function DocumentList({ documents }: { documents: DocumentSummary[] }) {
  if (documents.length === 0) {
    return (
      <div className="empty">
        <h2>No documents yet</h2>
        <p>Connect a reMarkable and pull to populate the library.</p>
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
          <tr key={d.document_id}>
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
