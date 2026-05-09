-- Folders are mirrored from the device on pull and (until now) never
-- modified locally. To support local rename, a `pending_push` bit
-- tracks whether the row's metadata_json + visible_name have diverged
-- from the device. The push engine flushes any pending folder by
-- uploading the folder's `<uuid>.metadata` file and clearing the bit.

ALTER TABLE folders ADD COLUMN pending_push INTEGER NOT NULL DEFAULT 0;

CREATE INDEX idx_folders_pending_push
    ON folders(pending_push) WHERE pending_push = 1;
