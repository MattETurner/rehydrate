-- Locally-deleted folders are tombstoned with `deleted_locally = 1`
-- rather than removed from the table outright. We need to keep the
-- row around until the next sync so the push engine can ship the
-- `<uuid>.metadata` file with `deleted: true` and let xochitl GC
-- the folder on the tablet. After a successful push,
-- `mark_folder_pushed` deletes the row entirely.
--
-- The sidebar's `list_folders` query filters tombstoned rows out, so
-- the folder disappears immediately even though the row lingers
-- across a sync cycle.

ALTER TABLE folders ADD COLUMN deleted_locally INTEGER NOT NULL DEFAULT 0;
