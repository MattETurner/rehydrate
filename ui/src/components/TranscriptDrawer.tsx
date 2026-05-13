import { useEffect, useState, type RefObject } from "react";
import { ipc } from "../ipc";
import { useDialogA11y } from "../dialogA11y";
import { useToast } from "./Toast";
import type {
  DocumentSummary,
  PublishCredentialStatus,
  PublishKind,
  TranscriptDocument,
} from "../types";
import { Icon } from "./Icon";
import { parsePublishUnconfigured } from "./SettingsModal";
import { formatError } from "../formatError";
import { Skeleton } from "./Skeleton";

interface Props {
  document: DocumentSummary;
  onClose: () => void;
  /** Hook to surface success / warn / error toasts in the parent. */
  notify: (
    tone: "ok" | "warn" | "err",
    body: string,
  ) => void;
  /** When a publish action fails because credentials aren't saved
   *  yet, the drawer routes the user to the Publishing tab in
   *  Settings instead of just toasting an opaque error. */
  onOpenSettings?: (tab: "ollama" | "publishing", banner?: string) => void;
}

/* Drawer that shows the OCR transcript for the document's CURRENT
 * version (transcripts attach to the manifest, so a re-OCR after
 * edits creates a new version automatically). Mirrors the
 * HistoryDrawer visual style.
 *
 * Layout note: the parent `.drawer` uses
 * `grid-template-rows: auto auto 1fr auto` (header, meta, content,
 * footer). The component renders exactly that shape — a `<header>`,
 * a `<div className="transcript-meta">`, a scrollable
 * `<div className="transcript-scroll">`, and a `<footer>`. An
 * earlier version emitted ad-hoc sibling `<p>` / `<pre>` / `<div>`
 * elements that got slotted into the wrong grid rows and produced a
 * huge empty band between the title and the transcript text. The
 * structure here matches the grid's expectation 1:1.
 */
export function TranscriptDrawer({ document, onClose, notify, onOpenSettings }: Props) {
  const { dialogProps, rootRef, titleId } = useDialogA11y({
    onEscape: onClose,
  });
  const toast = useToast();
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
        if (!cancelled) setError(formatError(e));
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
      setError(formatError(e));
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
      const targetLabel = target === "ghost" ? "Ghost" : "WordPress";
      // Persistent toast (`duration: 0`) with an action button — the
      // user needs time to read the message and click through to the
      // draft, and the URL itself isn't selectable from a transient
      // toast. The `View draft` button routes through the backend's
      // `open_publish_url`, which validates the URL host against the
      // saved credentials for `target` before handing off to the OS
      // browser, so a future bug can't aim this at arbitrary sites.
      toast.show({
        tone: "ok",
        body: `Draft created on ${targetLabel}.`,
        duration: 0,
        action: {
          label: "View draft",
          onClick: async () => {
            try {
              await ipc.openPublishUrl(result.edit_url, target);
            } catch (e) {
              notify("err", `Could not open draft: ${formatError(e)}`);
            }
          },
        },
      });
    } catch (e) {
      // If publish failed because no credentials are saved, hand
      // the user off to Settings → Publishing instead of leaving
      // them staring at a raw "no Ghost credentials saved" toast.
      const unconfigured = parsePublishUnconfigured(e);
      if (unconfigured && onOpenSettings) {
        onOpenSettings("publishing", unconfigured.message);
      } else {
        setError(formatError(e));
      }
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="drawer-backdrop" onClick={onClose}>
      <aside
        className="drawer transcript-drawer"
        onClick={(e) => e.stopPropagation()}
        ref={rootRef as unknown as RefObject<HTMLElement>}
        {...dialogProps}
      >
        <header>
          <h2 id={titleId}>Transcript</h2>
          <button
            className="icon ghost"
            onClick={onClose}
            aria-label="Close transcript"
          >
            <Icon name="x" />
          </button>
        </header>

        <div className="transcript-meta">
          <div className="transcript-meta-title">{document.visible_name}</div>
          {transcript && (
            <div className="muted small transcript-meta-sub">
              {transcript.model && (
                <>
                  Model: <code>{transcript.model}</code>
                </>
              )}
              {transcript.model && transcript.language && " · "}
              {transcript.language && <>Language: {transcript.language}</>}
              {(transcript.model || transcript.language) && " · "}
              Version {transcript.version_id}
            </div>
          )}
        </div>

        <div className="transcript-scroll">
          {transcript === undefined && (
            <div className="drawer-loading">
              <Skeleton width="60%" height={14} mb={8} />
              <Skeleton width="100%" height={14} mb={8} />
              <Skeleton width="92%" height={14} mb={8} />
              <Skeleton width="88%" height={14} mb={8} />
              <Skeleton width="74%" height={14} />
            </div>
          )}
          {transcript === null && (
            <p className="transcript-empty muted">
              No transcript yet. Use <em>Convert to text…</em> from the
              document menu.
            </p>
          )}
          {transcript && (
            <pre className="transcript-body">
              {stripFrontmatter(transcript.markdown)}
            </pre>
          )}
          {error && (
            <div className="error inline" role="alert">
              {error}
            </div>
          )}
        </div>

        {transcript && (
          <footer className="transcript-footer">
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
          </footer>
        )}
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
