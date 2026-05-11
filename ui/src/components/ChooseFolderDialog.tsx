import { useMemo, useState } from "react";
import { Icon } from "./Icon";
import { ipc } from "../ipc";
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
  /** Called after the user creates a new folder inline, so the parent
   *  can refresh its folder list. The new folder's id is also pre-
   *  selected as the destination so the user can immediately click
   *  "Move here". */
  onFolderCreated?: (folder: FolderEntry) => void;
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
  onFolderCreated,
}: Props) {
  const [selected, setSelected] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Inline "+ New folder" affordance state. `creating` toggles an
  // input row at the top of the picker; `creatingUnder` records the
  // parent so the user can create a subfolder while their selection
  // is on, e.g., "Receipts" → "Receipts/2026".
  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState("");
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

  async function createInline() {
    const name = newName.trim();
    if (!name) return;
    setBusy(true);
    setError(null);
    try {
      // Selected row at create-time becomes the parent — pick a
      // folder first, then "+ New folder" creates a subfolder
      // there. With no selection we create at the root.
      const created = await ipc.createFolder(name, selected);
      onFolderCreated?.(created);
      setSelected(created.folder_id);
      setCreating(false);
      setNewName("");
    } catch (err) {
      setError(String(err));
    } finally {
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
          {creating ? (
            <li className="folder-picker-row creating">
              <Icon name="folder" />
              <input
                type="text"
                autoFocus
                placeholder={
                  selected === null
                    ? "New folder at root"
                    : "New subfolder name"
                }
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    void createInline();
                  } else if (e.key === "Escape") {
                    e.preventDefault();
                    setCreating(false);
                    setNewName("");
                  }
                }}
                disabled={busy}
              />
              <button
                type="button"
                disabled={busy || !newName.trim()}
                onClick={createInline}
              >
                Create
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => {
                  setCreating(false);
                  setNewName("");
                }}
              >
                Cancel
              </button>
            </li>
          ) : (
            <li
              className="folder-picker-row new-folder"
              onClick={() => setCreating(true)}
              title={
                selected === null
                  ? "Create a new folder at the root"
                  : "Create a subfolder under the selected folder"
              }
            >
              <Icon name="folder" />
              <span>
                {selected === null
                  ? "+ New folder…"
                  : "+ New subfolder here…"}
              </span>
            </li>
          )}
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
