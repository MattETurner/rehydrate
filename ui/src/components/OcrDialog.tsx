import { useEffect, useState } from "react";
import { ipc, onOcrProgress } from "../ipc";
import type {
  DocumentSummary,
  OcrProgressEvent,
  OcrStatusReport,
  TranscriptSummary,
} from "../types";

interface Props {
  document: DocumentSummary;
  onCancel: () => void;
  onDone: (summary: TranscriptSummary) => void;
}

type Phase =
  | { kind: "loading" }
  | { kind: "missing"; descriptor: OcrStatusReport["descriptor"] }
  | { kind: "downloading"; bytes_done: number; bytes_total: number | null }
  | { kind: "ready"; descriptor: OcrStatusReport["descriptor"] }
  | { kind: "running"; pages_done: number; current_chars: number }
  | { kind: "done"; summary: TranscriptSummary }
  | { kind: "error"; message: string };

/* "Convert to text…" dialog. Walks the user through (a) downloading
 * the model on first run, (b) running the OCR with per-page progress,
 * and (c) reporting the resulting transcript summary. */
export function OcrDialog({ document, onCancel, onDone }: Props) {
  const [phase, setPhase] = useState<Phase>({ kind: "loading" });
  const [language, setLanguage] = useState<string>("");

  // Load model status on mount.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const status = await ipc.ocrStatus();
        if (cancelled) return;
        if (status.kind === "ready") {
          setPhase({ kind: "ready", descriptor: status.descriptor });
        } else if (status.kind === "partial") {
          // Treat resumable as missing so the user re-triggers
          // download with the same range-resume support in Rust.
          setPhase({ kind: "missing", descriptor: status.descriptor });
        } else {
          setPhase({ kind: "missing", descriptor: status.descriptor });
        }
      } catch (e) {
        if (!cancelled) setPhase({ kind: "error", message: String(e) });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Stream progress events.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    onOcrProgress((ev: OcrProgressEvent) => {
      setPhase((current) => {
        switch (ev.kind) {
          case "download_progress":
            return {
              kind: "downloading",
              bytes_done: ev.done,
              bytes_total: ev.total,
            };
          case "download_done":
            // Status query at finish settles the descriptor.
            ipc.ocrStatus().then((s) => {
              if (s.kind === "ready") {
                setPhase({ kind: "ready", descriptor: s.descriptor });
              }
            });
            return current;
          case "page_done":
            if (current.kind === "running") {
              return {
                kind: "running",
                pages_done: ev.page_index + 1,
                current_chars: current.current_chars + ev.chars,
              };
            }
            return {
              kind: "running",
              pages_done: ev.page_index + 1,
              current_chars: ev.chars,
            };
          default:
            return current;
        }
      });
    }).then((u) => {
      unlisten = u;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  async function startDownload() {
    try {
      setPhase({ kind: "downloading", bytes_done: 0, bytes_total: null });
      await ipc.ocrDownloadDefaultModel();
      const status = await ipc.ocrStatus();
      if (status.kind === "ready") {
        setPhase({ kind: "ready", descriptor: status.descriptor });
      }
    } catch (e) {
      setPhase({ kind: "error", message: String(e) });
    }
  }

  async function startTranscribe() {
    try {
      setPhase({ kind: "running", pages_done: 0, current_chars: 0 });
      const summary = await ipc.transcribeDocument(
        document.document_id,
        language.trim() || undefined,
      );
      setPhase({ kind: "done", summary });
      onDone(summary);
    } catch (e) {
      setPhase({ kind: "error", message: String(e) });
    }
  }

  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>Convert to text</h2>
        <p className="muted">
          On-device handwriting OCR for <strong>{document.visible_name}</strong>.
        </p>

        {phase.kind === "loading" && <p>Checking model…</p>}

        {phase.kind === "missing" && (
          <>
            <p>
              First-time setup: download{" "}
              <strong>{phase.descriptor.display_name}</strong> (
              {(phase.descriptor.size_bytes / 1e9).toFixed(1)} GB). Runs
              entirely on your machine — no notes leave your device.
            </p>
            <div className="actions">
              <button type="button" onClick={onCancel}>
                Cancel
              </button>
              <button className="primary" onClick={startDownload}>
                Download model
              </button>
            </div>
          </>
        )}

        {phase.kind === "downloading" && (
          <>
            <p>Downloading and loading the model…</p>
            <progress
              value={phase.bytes_done > 0 ? phase.bytes_done : undefined}
              max={phase.bytes_total ?? undefined}
            />
            <p className="muted">
              {phase.bytes_done > 0
                ? `${(phase.bytes_done / 1e9).toFixed(2)} GB${
                    phase.bytes_total
                      ? ` / ${(phase.bytes_total / 1e9).toFixed(2)} GB`
                      : ""
                  }`
                : "First run downloads several GB from HuggingFace; runs entirely on your machine afterwards."}
            </p>
          </>
        )}

        {phase.kind === "ready" && (
          <>
            <p>
              Using <strong>{phase.descriptor.display_name}</strong>. Optional
              language hint helps with short or ambiguous handwriting.
            </p>
            <input
              type="text"
              placeholder="en, de, ja, ar… (auto-detected if empty)"
              value={language}
              onChange={(e) => setLanguage(e.target.value)}
            />
            <div className="actions">
              <button type="button" onClick={onCancel}>
                Cancel
              </button>
              <button className="primary" onClick={startTranscribe}>
                Convert
              </button>
            </div>
          </>
        )}

        {phase.kind === "running" && (
          <>
            <p>OCR in progress…</p>
            <p className="muted">
              {phase.pages_done} {phase.pages_done === 1 ? "page" : "pages"}{" "}
              done · {phase.current_chars} characters so far
            </p>
            <progress />
          </>
        )}

        {phase.kind === "done" && (
          <>
            <p>
              Done. Transcribed {phase.summary.page_count}{" "}
              {phase.summary.page_count === 1 ? "page" : "pages"} (
              {phase.summary.char_count} characters) using{" "}
              <code>{phase.summary.model}</code>.
            </p>
            <div className="actions">
              <button className="primary" onClick={onCancel}>
                Close
              </button>
            </div>
          </>
        )}

        {phase.kind === "error" && (
          <>
            <div className="error inline">{phase.message}</div>
            <div className="actions">
              <button type="button" onClick={onCancel}>
                Close
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
