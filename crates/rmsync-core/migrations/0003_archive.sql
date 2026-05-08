-- Archive ("waste bin"): documents the user has soft-deleted, either
-- locally via the UI ('local') or because the device no longer has them
-- ('device'). Versions remain in the version log so the document can be
-- restored bit-for-bit. Hard delete (purge) drops the rows here AND in
-- versions/sync_state, after which GC can reclaim the orphan blobs.

CREATE TABLE archived_documents (
    document_id          TEXT PRIMARY KEY,
    visible_name         TEXT NOT NULL,
    doc_type             TEXT NOT NULL,
    parent               TEXT,
    manifest_hash        TEXT NOT NULL,
    version_id           INTEGER NOT NULL,
    reason               TEXT NOT NULL,
    archived_at          TEXT NOT NULL
);

CREATE INDEX idx_archived_documents_archived_at
    ON archived_documents(archived_at);
