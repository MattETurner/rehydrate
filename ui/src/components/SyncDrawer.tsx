import { useEffect, useMemo, useState } from "react";
import { ipc, onSyncProgress } from "../ipc";
import type { PlanItemStatus, ProgressEvent, PullPlan, SyncReport } from "../types";

interface Props {
  onClose: () => void;
  onComplete: () => void;
}

interface DocProgress {
  state: "queued" | "in-progress" | "done" | "skipped";
  files: number;
  bytes: number;
  deduped: number;
  reason?: string;
}

const STATUS_LABEL: Record<PlanItemStatus, string> = {
  new: "New",
  changed: "Changed",
  unchanged: "Unchanged",
  skipped: "Skipped",
};

const STATUS_CLASS: Record<PlanItemStatus, string> = {
  new: "badge badge-new",
  changed: "badge badge-changed",
  unchanged: "badge badge-unchanged",
  skipped: "badge badge-skipped",
};

export function SyncDrawer({ onClose, onComplete }: Props) {
  const [plan, setPlan] = useState<PullPlan | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [report, setReport] = useState<SyncReport | null>(null);
  const [progress, setProgress] = useState<Record<string, DocProgress>>({});
  const [activeDoc, setActiveDoc] = useState<string | null>(null);

  useEffect(() => {
    let mounted = true;
    ipc
      .pullPlan()
      .then((p) => {
        if (mounted) setPlan(p);
      })
      .catch((e) => {
        if (mounted) setPlanError(String(e));
      });
    return () => {
      mounted = false;
    };
  }, []);

  const counts = useMemo(() => {
    if (!plan) return null;
    const c = { new: 0, changed: 0, unchanged: 0, skipped: 0 };
    for (const item of plan.items) c[item.status]++;
    return c;
  }, [plan]);

  async function start() {
    if (!plan) return;
    setRunning(true);
    setProgress({});
    setReport(null);

    const unlisten = await onSyncProgress((ev: ProgressEvent) => {
      setProgress((prev) => {
        const next = { ...prev };
        switch (ev.kind) {
          case "document_started":
            next[ev.document_id] = {
              state: "in-progress",
              files: 0,
              bytes: 0,
              deduped: 0,
            };
            setActiveDoc(ev.document_id);
            break;
          case "file_fetched": {
            const cur = next[ev.document_id] ?? {
              state: "in-progress" as const,
              files: 0,
              bytes: 0,
              deduped: 0,
            };
            next[ev.document_id] = {
              ...cur,
              files: cur.files + 1,
              bytes: cur.bytes + ev.bytes,
              deduped: cur.deduped + (ev.deduped ? 1 : 0),
            };
            break;
          }
          case "document_completed": {
            const cur = next[ev.document_id] ?? {
              state: "done" as const,
              files: 0,
              bytes: 0,
              deduped: 0,
            };
            next[ev.document_id] = { ...cur, state: "done" };
            setActiveDoc(null);
            break;
          }
          case "document_skipped":
            next[ev.document_id] = {
              state: "skipped",
              files: 0,
              bytes: 0,
              deduped: 0,
              reason: ev.reason,
            };
            break;
          case "done":
            setReport({
              recorded: ev.recorded,
              unchanged: ev.unchanged,
              skipped: ev.skipped,
            });
            break;
        }
        return next;
      });
    });

    try {
      const r = await ipc.pullExecute();
      setReport(r);
      onComplete();
    } catch (e) {
      setPlanError(String(e));
    } finally {
      unlisten();
      setRunning(false);
      setActiveDoc(null);
    }
  }

  return (
    <div className="drawer" onClick={(e) => e.stopPropagation()}>
      <header>
        <h2>Sync from reMarkable</h2>
        <button onClick={onClose} disabled={running} className="close">
          ×
        </button>
      </header>

      {planError && <div className="error">{planError}</div>}

      {!plan && !planError && <div className="empty">Planning…</div>}

      {plan && (
        <>
          <div className="plan-summary">
            {counts && (
              <>
                <span className="badge badge-new">{counts.new} new</span>
                <span className="badge badge-changed">{counts.changed} changed</span>
                <span className="badge badge-unchanged">
                  {counts.unchanged} unchanged
                </span>
                {counts.skipped > 0 && (
                  <span className="badge badge-skipped">{counts.skipped} skipped</span>
                )}
              </>
            )}
          </div>

          <ul className="plan-list">
            {plan.items.map((item) => {
              const p = progress[item.entry.uuid];
              const isActive = activeDoc === item.entry.uuid;
              return (
                <li key={item.entry.uuid} className={isActive ? "active" : ""}>
                  <span className={STATUS_CLASS[item.status]}>
                    {STATUS_LABEL[item.status]}
                  </span>
                  <span className="title">{item.entry.visible_name}</span>
                  <span className="muted small">{item.entry.doc_type}</span>
                  {p && p.state === "in-progress" && (
                    <span className="muted small">
                      {p.files} files · {formatBytes(p.bytes)}
                    </span>
                  )}
                  {p && p.state === "done" && (
                    <span className="badge badge-ok">✓</span>
                  )}
                  {p && p.state === "skipped" && (
                    <span className="badge badge-skipped" title={p.reason}>
                      Skipped
                    </span>
                  )}
                </li>
              );
            })}
          </ul>

          <footer>
            {report ? (
              <div className="report">
                Done · {report.recorded} recorded, {report.unchanged} unchanged
                {report.skipped > 0 ? `, ${report.skipped} skipped` : ""}
                <button onClick={onClose}>Close</button>
              </div>
            ) : (
              <button onClick={start} disabled={running} className="primary">
                {running ? "Syncing…" : "Start sync"}
              </button>
            )}
          </footer>
        </>
      )}
    </div>
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
