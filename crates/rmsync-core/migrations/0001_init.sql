CREATE TABLE documents (
    document_id          TEXT PRIMARY KEY,
    current_manifest     TEXT NOT NULL,
    current_version_id   INTEGER NOT NULL
);

CREATE TABLE versions (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id          TEXT NOT NULL,
    manifest_hash        TEXT NOT NULL,
    parent_version_id    INTEGER,
    observed_at          TEXT NOT NULL,
    source               TEXT NOT NULL,
    note                 TEXT,
    FOREIGN KEY (parent_version_id) REFERENCES versions(id) ON DELETE SET NULL
);
CREATE INDEX idx_versions_doc ON versions(document_id, id);
CREATE UNIQUE INDEX idx_versions_doc_manifest ON versions(document_id, manifest_hash);

CREATE TABLE folders (
    folder_id            TEXT PRIMARY KEY,
    parent               TEXT,
    visible_name         TEXT NOT NULL,
    metadata_json        TEXT NOT NULL
);

CREATE TABLE sync_state (
    document_id          TEXT PRIMARY KEY,
    device_mtime_hint    TEXT,
    last_seen_manifest   TEXT,
    last_synced_at       TEXT
);

CREATE TABLE blob_refs (
    blob_hash            TEXT NOT NULL,
    manifest_hash        TEXT NOT NULL,
    PRIMARY KEY (blob_hash, manifest_hash)
);
CREATE INDEX idx_blob_refs_manifest ON blob_refs(manifest_hash);
