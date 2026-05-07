import { useEffect, useMemo, useState } from "react";
import { ipc, onSyncPhase, onSyncProgress } from "../ipc";
import type {
  PlanItemStatus,
  ProgressEvent,
  PullPlan,
  PushItemStatus,
  PushPlan,
  TwoWayReport,
} from "../types";

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

const PULL_LABEL: Record<PlanItemStatus, string> = {
  new: "New",
  changed: "Changed",
  unchanged: "Unchanged",
  skipped: "Skipped",
};
const PULL_CLASS: Record<PlanItemStatus, string> = {
  new: "badge badge-new",
  changed: "badge badge-changed",
  unchanged: "badge badge-unchanged",
  skipped: "badge badge-skipped",
};

const PUSH_LABEL: Record<PushItemStatus, string> = {
  outbound: "Outbound",
  unchanged: "Unchanged",
  skipped: "Skipped",
};
const PUSH_CLASS: Record<PushItemStatus, string> = {
  outbound: "badge badge-new",
  unchanged: "badge badge-unchanged",
  skipped: "badge badge-skipped",
};

export function SyncDrawer({ onClose, onComplete }: Props) {
  const [pullPlan, setPullPlan] = useState<PullPlan | null>(null);
  const [pushPlan, setPushPlan] = useState<PushPlan | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [phase, setPhase] = useState<"plan" | "pull" | "push" | "done">("plan");
  const [report, setReport] = useState<TwoWayReport | null>(null);
  const [progress, setProgress] = useState<Record<string, DocProgress>>({});

  // Build the plan preview up front so the user can see what's about to
  // happen before they hit Start.
  useEffect(() => {
    let mounted = true;
    Promise.all([ipc.pullPlan(), ipc.pushPlan()])
      .then(([pl, ps]) => {
        if (!mounted) return;
        setPullPlan(pl);
        setPushPlan(ps);
      })
      .catch((e) => {
        if (mounted) setPlanError(String(e));
      });
    return () => {
      mounted = false;
    };
  }, []);

  const counts = useMemo(() => {
    if (!pullPlan || !pushPlan) return null;
    const pull = { new: 0, changed: 0, unchanged: 0, skipped: 0 };
    for (const item of pullPlan.items) pull[item.status]++;
    const push = { outbound: 0, unchanged: 0, skipped: 0 };
    for (const item of pushPlan.items) push[item.status]++;
    return { pull, push };
  }, [pullPlan, pushPlan]);

  async function start() {
    setRunning(true);
    setProgress({});
    setReport(null);
    setPhase("pull");

    const unlistenPhase = await onSyncPhase((p) => setPhase(p));
    const unlistenProgress = await onSyncProgress((ev: ProgressEvent) => {
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
        }
        return next;
      });
    });

    try {
      const r = await ipc.syncTwoWay();
      setReport(r);
      setPhase("done");
      onComplete();
    } catch (e) {
      setPlanError(String(e));
    } finally {
      unlistenPhase();
      unlistenProgress();
      setRunning(false);
    }
  }

  const allItems = useMemo(() => {
    const items: Array<{
      key: string;
      title: string;
      docType: string;
      label: string;
      cls: string;
      uuid: string;
      direction: "pull" | "push";
    }> = [];
    if (pullPlan) {
      for (const i of pullPlan.items) {
        if (i.status === "unchanged") continue;
        items.push({
          key: `pull-${i.entry.uuid}`,
          title: i.entry.visible_name,
          docType: i.entry.doc_type,
          label: PULL_LABEL[i.status],
          cls: PULL_CLASS[i.status],
          uuid: i.entry.uuid,
          direction: "pull",
        });
      }
    }
    if (pushPlan) {
      for (const i of pushPlan.items) {
        if (i.status === "unchanged") continue;
        items.push({
          key: `push-${i.document.document_id}`,
          title: i.document.visible_name,
          docType: i.document.doc_type,
          label: PUSH_LABEL[i.status],
          cls: PUSH_CLASS[i.status],
          uuid: i.document.document_id,
          direction: "push",
        });
      }
    }
    return items;
  }, [pullPlan, pushPlan]);

  const activeProgressKeys = useMemo(() => Object.keys(progress), [progress]);

  return (
    <div className="drawer" onClick={(e) => e.stopPropagation()}>
      <header>
        <h2>Sync with reMarkable</h2>
        <button onClick={onClose} disabled={running} className="close">
          ×
        </button>
      </header>

      {planError && <div className="error">{planError}</div>}

      {(!pullPlan || !pushPlan) && !planError && (
        <div className="empty">Planning…</div>
      )}

      {pullPlan && pushPlan && (
        <>
          <div className="plan-summary">
            {counts && (
              <>
                <span className="muted small">Pull:</span>
                <span className="badge badge-new">{counts.pull.new} new</span>
                <span className="badge badge-changed">
                  {counts.pull.changed} changed
                </span>
                <span className="badge badge-unchanged">
                  {counts.pull.unchanged} unchanged
                </span>
                <span className="muted small">·</span>
                <span className="muted small">Push:</span>
                <span className="badge badge-new">
                  {counts.push.outbound} outbound
                </span>
                <span className="badge badge-unchanged">
                  {counts.push.unchanged} unchanged
                </span>
              </>
            )}
          </div>

          {allItems.length === 0 ? (
            <div className="empty">
              Nothing to sync — library and device match.
            </div>
          ) : (
            <ul className="plan-list">
              {allItems.map((it) => {
                const p = progress[it.uuid];
                const inThisPhase =
                  phase === it.direction || (phase === "done" && p?.state === "done");
                return (
                  <li key={it.key} className={inThisPhase && p?.state === "in-progress" ? "active" : ""}>
                    <span className={it.cls}>{it.label}</span>
                    <span className="muted small">{it.direction === "pull" ? "↓" : "↑"}</span>
                    <span className="title">{it.title}</span>
                    <span className="muted small">{it.docType}</span>
                    {p && p.state === "in-progress" && (
                      <span className="muted small">
                        {p.files} files · {formatBytes(p.bytes)}
                      </span>
                    )}
                    {p && p.state === "done" && <span className="badge badge-ok">✓</span>}
                    {p && p.state === "skipped" && (
                      <span className="badge badge-skipped" title={p.reason}>
                        Skipped
                      </span>
                    )}
                  </li>
                );
              })}
            </ul>
          )}

          <footer>
            {report ? (
              <div className="report">
                Done · pulled {report.pull.recorded}, pushed {report.push.pushed}
                {report.pull.skipped + report.push.skipped > 0
                  ? `, ${report.pull.skipped + report.push.skipped} skipped`
                  : ""}
                <button onClick={onClose}>Close</button>
              </div>
            ) : (
              <>
                {running && (
                  <span className="muted small">
                    {phase === "pull"
                      ? "Pulling from device…"
                      : phase === "push"
                        ? "Pushing to device…"
                        : "Syncing…"}
                  </span>
                )}
                <button
                  onClick={start}
                  disabled={running || allItems.length === 0}
                  className="primary"
                >
                  {running ? "Syncing…" : "Start sync"}
                </button>
              </>
            )}
          </footer>
          {activeProgressKeys.length === 0 || running ? null : null}
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
