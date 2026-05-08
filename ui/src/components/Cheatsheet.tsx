import { Icon } from "./Icon";

interface Props {
  onClose: () => void;
}

const META = navigator.platform.includes("Mac") ? "⌘" : "Ctrl";

const SHORTCUTS: Array<[string[], string]> = [
  [[META, "K"], "Open command palette"],
  [[META, "F"], "Search this view"],
  [[META, "S"], "Sync with the tablet"],
  [[META, "I"], "Import a PDF or EPUB"],
  [[META, "⌫"], "Move selected to Archive"],
  [["Space"], "Quick Look the selection"],
  [["Enter"], "Open the selection"],
  [["↑", "↓"], "Move the selection"],
  [["Esc"], "Close drawer · clear search · clear selection"],
  [["?"], "Show this list"],
];

export function Cheatsheet({ onClose }: Props) {
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="cheatsheet"
        onClick={(e) => e.stopPropagation()}
      >
        <h2>
          <Icon name="info" /> Keyboard shortcuts
        </h2>
        <dl>
          {SHORTCUTS.map(([keys, label], i) => (
            <span key={i} style={{ display: "contents" }}>
              <dt>
                {keys.map((k, ki) => (
                  <span key={ki} style={{ display: "inline-flex", gap: 4 }}>
                    <span className="kbd">{k}</span>
                    {ki < keys.length - 1 && <span className="muted small">·</span>}
                  </span>
                ))}
              </dt>
              <dd>{label}</dd>
            </span>
          ))}
        </dl>
      </div>
    </div>
  );
}
