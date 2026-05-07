import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ipc } from "../ipc";
import type { DocumentSummary, VersionEntry } from "../types";

interface Props {
  document: DocumentSummary;
  onClose: () => void;
}

export function HistoryDrawer({ document, onClose }: Props) {
  const [versions, setVersions] = useState<VersionEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    ipc
      .getHistory(document.document_id)
      .then((v) => {
        if (!cancelled) setVersions(v);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [document.document_id]);

  async function restoreVersion(v: VersionEntry) {
    if (!window.confirm(
      `Restore "${document.visible_name}" to v${v.id}?\n\n` +
        `The current version stays in history; the restored version becomes ` +
        `current and will be pushed to the device on the next sync.`,
    )) {
      return;
    }
    setError(null);
    try {
      await ipc.restoreVersion(v.id);
      const fresh = await ipc.getHistory(document.document_id);
      setVersions(fresh);
    } catch (e) {
      setError(String(e));
    }
  }

  async function exportVersion(v: VersionEntry) {
    setError(null);
    try {
      const dest = await openDialog({
        directory: true,
        multiple: false,
        title: `Export "${document.visible_name}" v${v.id} to…`,
      });
      if (!dest || typeof dest !== "string") return;
      setBusy(v.id);
      const result = await ipc.exportVersion(v.id, dest);
      window.alert(
        `Exported ${result.file_count} file${result.file_count === 1 ? "" : "s"} to:\n${result.path}`,
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function saveNote(v: VersionEntry, note: string) {
    setError(null);
    const trimmed = note.trim();
    try {
      await ipc.setVersionNote(v.id, trimmed === "" ? null : trimmed);
      setVersions((prev) =>
        prev?.map((x) => (x.id === v.id ? { ...x, note: trimmed === "" ? null : trimmed } : x)) ??
        prev,
      );
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="drawer" onClick={(e) => e.stopPropagation()}>
      <header>
        <div>
          <h2>{document.visible_name}</h2>
          <div className="muted small">{document.doc_type}</div>
        </div>
        <button onClick={onClose} className="close" aria-label="Close">
          ×
        </button>
      </header>

      {error && <div className="error">{error}</div>}

      {!versions && !error && <div className="empty">Loading history…</div>}

      {versions && versions.length === 0 && (
        <div className="empty">No versions recorded.</div>
      )}

      {versions && versions.length > 0 && (
        <ul className="history-list">
          {[...versions].reverse().map((v, idx, arr) => {
            const isCurrent = idx === 0; // newest first after reverse
            const versionNum = arr.length - idx;
            return (
              <li key={v.id}>
                <div className="row">
                  <span className={`badge ${isCurrent ? "badge-ok" : "badge-unchanged"}`}>
                    v{versionNum}
                    {isCurrent ? " · current" : ""}
                  </span>
                  <span className="muted small">{formatTimestamp(v.observed_at)}</span>
                  <span className="muted small">{sourceLabel(v.source)}</span>
                  <span className="spacer" />
                  <span className="muted small mono" title={v.manifest_hash}>
                    {v.manifest_hash.slice(0, 12)}
                  </span>
                </div>
                <div className="row meta">
                  {v.file_count !== null && (
                    <span className="muted small">
                      {v.file_count} file{v.file_count === 1 ? "" : "s"}
                    </span>
                  )}
                  {v.total_size_bytes !== null && (
                    <span className="muted small">{formatBytes(v.total_size_bytes)}</span>
                  )}
                  <span className="spacer" />
                  {!isCurrent && (
                    <button
                      onClick={() => restoreVersion(v)}
                      disabled={busy === v.id}
                      className="link"
                    >
                      Restore to current
                    </button>
                  )}
                  <button
                    onClick={() => exportVersion(v)}
                    disabled={busy === v.id}
                    className="link"
                  >
                    {busy === v.id ? "Exporting…" : "Export to disk"}
                  </button>
                </div>
                <NoteField version={v} onSave={(note) => saveNote(v, note)} />
              </li>
            );
          })}
        </ul>
      )}

      <footer>
        <span className="muted small">
          {versions ? `${versions.length} version${versions.length === 1 ? "" : "s"}` : ""}
        </span>
      </footer>
    </div>
  );
}

function NoteField({ version, onSave }: { version: VersionEntry; onSave: (note: string) => void }) {
  const [value, setValue] = useState(version.note ?? "");
  const [editing, setEditing] = useState(false);

  if (!editing && !value) {
    return (
      <button className="link note-add" onClick={() => setEditing(true)}>
        + Add note
      </button>
    );
  }
  if (!editing) {
    return (
      <div className="note" onClick={() => setEditing(true)}>
        {value}
      </div>
    );
  }
  return (
    <input
      type="text"
      autoFocus
      value={value}
      onChange={(e) => setValue(e.target.value)}
      onBlur={() => {
        setEditing(false);
        if (value !== (version.note ?? "")) onSave(value);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        if (e.key === "Escape") {
          setValue(version.note ?? "");
          setEditing(false);
        }
      }}
      placeholder="Add a note for this version…"
    />
  );
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}

function formatTimestamp(iso: string): string {
  try {
    const d = new Date(iso);
    return d.toLocaleString();
  } catch {
    return iso;
  }
}

function sourceLabel(s: VersionEntry["source"]): string {
  switch (s) {
    case "pulled":
      return "Pulled from device";
    case "imported":
      return "Imported";
    case "restored":
      return "Restored";
  }
}
