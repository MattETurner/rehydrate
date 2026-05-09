import { useEffect, useState } from "react";
import { ipc } from "../ipc";
import type {
  DocumentSummary,
  PublishCredentialStatus,
  PublishKind,
  TranscriptDocument,
} from "../types";
import { Icon } from "./Icon";

interface Props {
  document: DocumentSummary;
  onClose: () => void;
  /** Hook to surface success / error toasts in the parent. */
  notify: (
    tone: "ok" | "err",
    body: string,
  ) => void;
}

/* Drawer that shows the OCR transcript for the document's CURRENT
 * version (transcripts attach to the manifest, so a re-OCR after
 * edits creates a new version automatically). Mirrors the
 * HistoryDrawer visual style. */
export function TranscriptDrawer({ document, onClose, notify }: Props) {
  const [transcript, setTranscript] = useState<TranscriptDocument | null | undefined>(
    undefined,
  );
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [creds, setCreds] = useState<PublishCredentialStatus | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const t = await ipc.getTranscript(document.current_version_id);
        if (!cancelled) setTranscript(t);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
      try {
        const status = await ipc.publishCredentialStatus();
        if (!cancelled) setCreds(status);
      } catch {
        // Non-fatal; the publish buttons will simply be disabled.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [document.current_version_id]);

  async function exportAs(format: "txt" | "markdown") {
    if (!transcript) return;
    setBusy(format);
    setError(null);
    try {
      const result = await ipc.exportTranscript(transcript.version_id, format);
      if (result) {
        notify(
          "ok",
          `Saved transcript to ${result.path}`,
        );
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function publish(target: PublishKind) {
    if (!transcript) return;
    setBusy(target);
    setError(null);
    try {
      const result = await ipc.publishTranscript(
        transcript.version_id,
        target,
      );
      notify(
        "ok",
        `Draft created on ${target === "ghost" ? "Ghost" : "WordPress"}: ${result.edit_url}`,
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="drawer-backdrop" onClick={onClose}>
      <aside
        className="drawer"
        onClick={(e) => e.stopPropagation()}
        aria-label="Transcript"
      >
        <header className="drawer-header">
          <h2>Transcript</h2>
          <button className="icon ghost" onClick={onClose} aria-label="Close">
            <Icon name="x" />
          </button>
        </header>

        <div className="drawer-body">
          <p className="muted">
            <strong>{document.visible_name}</strong>
          </p>

          {transcript === undefined && <p>Loading…</p>}
          {transcript === null && (
            <p>
              No transcript yet. Use <em>Convert to text…</em> from the
              document menu.
            </p>
          )}

          {transcript && (
            <>
              <p className="muted small">
                {transcript.model && <>Model: <code>{transcript.model}</code> · </>}
                {transcript.language && <>Language: {transcript.language} · </>}
                Version {transcript.version_id}
              </p>

              <pre className="transcript-body">
                {stripFrontmatter(transcript.markdown)}
              </pre>

              <div className="drawer-actions">
                <button
                  disabled={busy !== null}
                  onClick={() => exportAs("txt")}
                >
                  {busy === "txt" ? "Saving…" : "Save as .txt"}
                </button>
                <button
                  disabled={busy !== null}
                  onClick={() => exportAs("markdown")}
                >
                  {busy === "markdown" ? "Saving…" : "Save as .md"}
                </button>
                <button
                  disabled={busy !== null || !creds?.ghost}
                  title={
                    creds?.ghost
                      ? "Publish as a draft to Ghost"
                      : "Configure Ghost credentials in Publishing settings first"
                  }
                  onClick={() => publish("ghost")}
                >
                  {busy === "ghost" ? "Publishing…" : "Publish to Ghost"}
                </button>
                <button
                  disabled={busy !== null || !creds?.wordpress}
                  title={
                    creds?.wordpress
                      ? "Publish as a draft to WordPress"
                      : "Configure WordPress credentials in Publishing settings first"
                  }
                  onClick={() => publish("wordpress")}
                >
                  {busy === "wordpress" ? "Publishing…" : "Publish to WordPress"}
                </button>
              </div>
              {error && <div className="error inline">{error}</div>}
            </>
          )}
        </div>
      </aside>
    </div>
  );
}

function stripFrontmatter(md: string): string {
  if (!md.startsWith("---\n")) return md;
  const end = md.indexOf("\n---", 4);
  if (end < 0) return md;
  return md.slice(end + 4).replace(/^\n+/, "");
}
