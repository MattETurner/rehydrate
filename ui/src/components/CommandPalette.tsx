import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";

export interface PaletteItem {
  id: string;
  label: string;
  hint?: string;
  icon?: ReactNode;
  /** Tag shown on the right (e.g. "Sync", "Folder"). */
  tag?: string;
  keywords?: string;
  onRun: () => void | Promise<void>;
}

interface Props {
  items: PaletteItem[];
  onClose: () => void;
}

/* Fuzzy-ish picker. Naive substring score + label-prefix bonus —
 * good enough for a few hundred items, which is the realistic ceiling
 * here (folders + actions + smart filters + every doc, optionally). */
export function CommandPalette({ items, onClose }: Props) {
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);

  const filtered = useMemo(() => {
    const needle = q.trim().toLowerCase();
    if (!needle) return items.slice(0, 60);
    return items
      .map((it) => {
        const hay = (it.label + " " + (it.keywords ?? "") + " " + (it.tag ?? "")).toLowerCase();
        const idx = hay.indexOf(needle);
        if (idx < 0) return null;
        // Cheaper score = better. Prefix on label is best.
        const score = it.label.toLowerCase().startsWith(needle) ? -100 : idx;
        return { it, score };
      })
      .filter((x): x is { it: PaletteItem; score: number } => x !== null)
      .sort((a, b) => a.score - b.score)
      .slice(0, 60)
      .map((x) => x.it);
  }, [q, items]);

  // Reset active row whenever the result set changes shape.
  useEffect(() => {
    setActive(0);
  }, [q]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Scroll the active row into view as the user arrows around.
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLLIElement>("li.active");
    el?.scrollIntoView({ block: "nearest" });
  }, [active]);

  return (
    <div className="palette-backdrop" onClick={onClose}>
      <div className="palette" onClick={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          type="text"
          placeholder="Type a command, folder, or document…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setActive((a) => Math.min(filtered.length - 1, a + 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setActive((a) => Math.max(0, a - 1));
            } else if (e.key === "Enter") {
              e.preventDefault();
              const target = filtered[active];
              if (target) {
                onClose();
                target.onRun();
              }
            } else if (e.key === "Escape") {
              e.preventDefault();
              onClose();
            }
          }}
        />
        {filtered.length === 0 ? (
          <div className="palette-empty">Nothing matches "{q}".</div>
        ) : (
          <ul ref={listRef}>
            {filtered.map((it, i) => (
              <li
                key={it.id}
                className={i === active ? "active" : ""}
                onMouseEnter={() => setActive(i)}
                onClick={() => {
                  onClose();
                  it.onRun();
                }}
              >
                {it.icon}
                <span>{it.label}</span>
                {it.hint && <span className="muted small">— {it.hint}</span>}
                {it.tag && <span className="palette-tag">{it.tag}</span>}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
