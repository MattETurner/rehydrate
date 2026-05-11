import { useMemo, useState } from "react";
import { Icon } from "./Icon";
import type { FolderEntry } from "../types";

interface Props {
  title: string;
  subtitle?: string;
  folders: FolderEntry[];
  /** Folder ids that are not valid drop targets — typically the folder
   *  being moved and its descendants (so you can't move it into itself).
   *  Pass [] for document moves where no folder is excluded. */
  excludedIds?: string[];
  onCancel: () => void;
  onChoose: (folderId: string | null) => Promise<void>;
}

interface Node {
  folder: FolderEntry;
  children: Node[];
}

function buildTree(folders: FolderEntry[], excluded: Set<string>): Node[] {
  // Filter excluded ids before tree assembly so the user never sees a
  // node they can't pick. Children of an excluded ancestor become
  // unreachable too because their parent is gone, which is exactly
  // the semantics we want for the cycle case.
  const allowed = folders.filter((f) => !excluded.has(f.folder_id));
  const byId = new Map<string, Node>();
  for (const f of allowed) byId.set(f.folder_id, { folder: f, children: [] });
  const roots: Node[] = [];
  for (const node of byId.values()) {
    const parentId = node.folder.parent;
    if (parentId && byId.has(parentId)) {
      byId.get(parentId)!.children.push(node);
    } else {
      roots.push(node);
    }
  }
  const sortRec = (nodes: Node[]) => {
    nodes.sort((a, b) => {
      const cmp = (a.folder.sort_index ?? 0) - (b.folder.sort_index ?? 0);
      if (cmp !== 0) return cmp;
      return a.folder.visible_name.localeCompare(b.folder.visible_name);
    });
    for (const n of nodes) sortRec(n.children);
  };
  sortRec(roots);
  return roots;
}

export function ChooseFolderDialog({
  title,
  subtitle,
  folders,
  excludedIds = [],
  onCancel,
  onChoose,
}: Props) {
  const [selected, setSelected] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const excluded = useMemo(() => new Set(excludedIds), [excludedIds]);
  const roots = useMemo(() => buildTree(folders, excluded), [folders, excluded]);

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      await onChoose(selected);
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="modal choose-folder-modal"
        onClick={(e) => e.stopPropagation()}
      >
        <h2>{title}</h2>
        {subtitle && <p className="muted">{subtitle}</p>}
        <ul className="folder-picker">
          <li
            className={`folder-picker-row${selected === null ? " active" : ""}`}
            onClick={() => setSelected(null)}
            onDoubleClick={() => {
              setSelected(null);
              submit();
            }}
          >
            <Icon name="library" />
            <span>Library root</span>
          </li>
          {roots.map((node) => (
            <FolderPickerRow
              key={node.folder.folder_id}
              node={node}
              depth={0}
              selected={selected}
              setSelected={setSelected}
              onPick={(id) => {
                setSelected(id);
                submit();
              }}
            />
          ))}
        </ul>
        {error && <div className="error inline">{error}</div>}
        <div className="actions">
          <button type="button" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button type="button" disabled={busy} onClick={submit}>
            {busy ? "Moving…" : "Move here"}
          </button>
        </div>
      </div>
    </div>
  );
}

function FolderPickerRow({
  node,
  depth,
  selected,
  setSelected,
  onPick,
}: {
  node: Node;
  depth: number;
  selected: string | null;
  setSelected: (id: string) => void;
  onPick: (id: string) => void;
}) {
  return (
    <>
      <li
        className={`folder-picker-row${
          selected === node.folder.folder_id ? " active" : ""
        }`}
        style={{ paddingLeft: 14 + depth * 16 }}
        onClick={() => setSelected(node.folder.folder_id)}
        onDoubleClick={() => onPick(node.folder.folder_id)}
      >
        <Icon name="folder" />
        <span>{node.folder.visible_name}</span>
      </li>
      {node.children.map((child) => (
        <FolderPickerRow
          key={child.folder.folder_id}
          node={child}
          depth={depth + 1}
          selected={selected}
          setSelected={setSelected}
          onPick={onPick}
        />
      ))}
    </>
  );
}
