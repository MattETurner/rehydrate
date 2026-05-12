import { useDialogA11y } from "../dialogA11y";
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
  [[META, "click"], "Add to selection"],
  [["⇧", "click"], "Extend selection to here"],
  [["F2"], "Rename the selection"],
  [["Space"], "Quick Look the selection"],
  [["Enter"], "Open the selection"],
  [["↑", "↓"], "Move the selection"],
  [["Tab"], "Move between focusable elements"],
  [["Esc"], "Close drawer · clear search · clear selection"],
  [["?"], "Show this list"],
];

export function Cheatsheet({ onClose }: Props) {
  const { dialogProps, rootRef, titleId } = useDialogA11y({
    onEscape: onClose,
  });
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="cheatsheet"
        onClick={(e) => e.stopPropagation()}
        ref={rootRef}
        {...dialogProps}
      >
        <h2 id={titleId}>
          <Icon name="info" /> Keyboard shortcuts
        </h2>
        <dl>
          {SHORTCUTS.map(([keys, label], i) => (
            <span key={i} className="cheatsheet-row">
              <dt>
                {keys.map((k, ki) => (
                  <span key={ki} className="cheatsheet-keys">
                    <span className="kbd">{k}</span>
                    {ki < keys.length - 1 && (
                      <span className="muted small">·</span>
                    )}
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
