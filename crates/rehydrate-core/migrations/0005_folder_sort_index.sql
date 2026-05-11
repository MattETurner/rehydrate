-- Local-only ordering for folders in the sidebar.
--
-- The reMarkable does not represent folder order on the device, so this
-- column is never pushed. Initial backfill seeds existing rows with a
-- monotonic integer sequence using ROWID, preserving whatever the
-- user happened to be looking at on first launch after the upgrade.
-- We use REAL so the UI can drop a folder between two siblings by
-- assigning the midpoint of their indices, avoiding a full
-- renumbering on every drag-and-drop.

ALTER TABLE folders ADD COLUMN sort_index REAL NOT NULL DEFAULT 0.0;

UPDATE folders
SET sort_index = (
    SELECT COUNT(*) FROM folders f2 WHERE f2.rowid <= folders.rowid
);

CREATE INDEX idx_folders_parent_sort
    ON folders(parent, sort_index);
