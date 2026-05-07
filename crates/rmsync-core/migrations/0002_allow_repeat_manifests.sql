-- Drop the (document_id, manifest_hash) unique index. A restored version, or
-- a pull of a document whose state happened to revert to a previous one,
-- legitimately produces a new versions row with the same manifest hash. The
-- design doc explicitly calls for "a fresh version that happens to equal the
-- old one" on restore.
--
-- The non-unique idx_versions_doc index already covers the lookups we need.
DROP INDEX IF EXISTS idx_versions_doc_manifest;
