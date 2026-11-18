-- SQLite
-- A suffix for the editor key (`document.key`) of an office file. The key is made from the file
-- and its bytes, and the Document Server refuses a key it has already seen saved. After a backup
-- restore a file can hold such bytes again. Then an editor reports the key as outdated, and Orbit
-- stores a random suffix here, so the file gets a key the Document Server has not seen.
--
-- `base_key` is the key without the suffix. A row applies only while the file still has that
-- key: after a save the bytes differ and the row is ignored, then replaced.
--
-- `file_id` is an `attachment_references.id` or a `page_files.id`, by `file_kind`, so it has no
-- foreign key. The triggers below remove the row with its file.
CREATE TABLE office_document_keys (
    file_kind TEXT NOT NULL CHECK (file_kind IN ('attachment', 'page_file')),
    file_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    base_key TEXT NOT NULL,
    suffix TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (file_kind, file_id)
);

CREATE INDEX office_document_keys_workspace ON office_document_keys (workspace_id);

CREATE TRIGGER office_document_keys_attachment_deleted
AFTER DELETE ON attachment_references
BEGIN
    DELETE FROM office_document_keys WHERE file_kind = 'attachment' AND file_id = OLD.id;
END;

CREATE TRIGGER office_document_keys_page_file_deleted
AFTER DELETE ON page_files
BEGIN
    DELETE FROM office_document_keys WHERE file_kind = 'page_file' AND file_id = OLD.id;
END;
