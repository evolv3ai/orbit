-- SQLite
-- Earlier bytes of a file that the office editor (ONLYOFFICE) saved over. A task attachment or a
-- Docs page file points at one blob; each save puts the blob it replaces here, so the old bytes
-- are not reclaimed. `saved_by` and `created_at` are the save that replaced the blob: the user
-- who sent the last changes, and when Orbit stored them.
--
-- `file_id` is an `attachment_references.id` or a `page_files.id`, by `file_kind`, so it has no
-- foreign key. The triggers below remove the versions with their file on every delete path
-- (a direct delete, a foreign key cascade, the trash purge, a workspace purge).
CREATE TABLE office_file_versions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_kind TEXT NOT NULL CHECK (file_kind IN ('attachment', 'page_file')),
    file_id TEXT NOT NULL,
    blob_id TEXT NOT NULL REFERENCES attachment_blobs(id) ON DELETE RESTRICT,
    saved_by TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL
);

-- The versions of one file, newest first.
CREATE INDEX office_file_versions_file ON office_file_versions (file_kind, file_id, created_at, id);
-- Blob reference counting (`BLOB_REFERENCE_COUNT`) and the foreign key.
CREATE INDEX office_file_versions_blob ON office_file_versions (blob_id);
CREATE INDEX office_file_versions_workspace ON office_file_versions (workspace_id);
CREATE INDEX office_file_versions_saved_by ON office_file_versions (saved_by);

CREATE TRIGGER office_file_versions_attachment_deleted
AFTER DELETE ON attachment_references
BEGIN
    DELETE FROM office_file_versions WHERE file_kind = 'attachment' AND file_id = OLD.id;
END;

CREATE TRIGGER office_file_versions_page_file_deleted
AFTER DELETE ON page_files
BEGIN
    DELETE FROM office_file_versions WHERE file_kind = 'page_file' AND file_id = OLD.id;
END;

-- A save points the file at another blob, which the scope triggers of 0009 and 0025 refused.
-- They now allow a new `blob_id` when the blob belongs to the file's workspace (what the insert
-- triggers check). The workspace, task, comment and page of a file stay immutable.
DROP TRIGGER attachment_references_scope_immutable;
CREATE TRIGGER attachment_references_scope_immutable
BEFORE UPDATE OF workspace_id, task_id, comment_id, blob_id ON attachment_references
WHEN NEW.workspace_id <> OLD.workspace_id
  OR NEW.task_id <> OLD.task_id
  OR NEW.comment_id IS NOT OLD.comment_id
  OR (NEW.blob_id <> OLD.blob_id AND NOT EXISTS (
        SELECT 1 FROM attachment_blobs
        WHERE attachment_blobs.id = NEW.blob_id AND attachment_blobs.workspace_id = NEW.workspace_id
  ))
BEGIN
    SELECT RAISE(ABORT, 'attachment reference scope is immutable');
END;

DROP TRIGGER page_files_scope_immutable;
CREATE TRIGGER page_files_scope_immutable
BEFORE UPDATE OF workspace_id, page_id, blob_id ON page_files
WHEN NEW.workspace_id <> OLD.workspace_id OR NEW.page_id <> OLD.page_id
    OR (NEW.blob_id <> OLD.blob_id AND NOT EXISTS (
        SELECT 1 FROM attachment_blobs
        WHERE attachment_blobs.id = NEW.blob_id AND attachment_blobs.workspace_id = NEW.workspace_id
    ))
BEGIN
    SELECT RAISE(ABORT, 'page file scope is immutable');
END;
