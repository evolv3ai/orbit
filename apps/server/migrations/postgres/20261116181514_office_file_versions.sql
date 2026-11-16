-- Postgres: the same change as apps/server/migrations/20261116181514_office_file_versions.sql
CREATE TABLE office_file_versions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_kind TEXT NOT NULL CHECK (file_kind IN ('attachment', 'page_file')),
    file_id TEXT NOT NULL,
    blob_id TEXT NOT NULL REFERENCES attachment_blobs(id) ON DELETE RESTRICT,
    saved_by TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at BIGINT NOT NULL
);

CREATE INDEX office_file_versions_file ON office_file_versions (file_kind, file_id, created_at, id);
CREATE INDEX office_file_versions_blob ON office_file_versions (blob_id);
CREATE INDEX office_file_versions_workspace ON office_file_versions (workspace_id);
CREATE INDEX office_file_versions_saved_by ON office_file_versions (saved_by);

CREATE FUNCTION office_file_versions_attachment_deleted_fn() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM office_file_versions WHERE file_kind = 'attachment' AND file_id = OLD.id;
    RETURN NULL;
END;
$$;
CREATE TRIGGER office_file_versions_attachment_deleted
    AFTER DELETE ON attachment_references
    FOR EACH ROW
    EXECUTE FUNCTION office_file_versions_attachment_deleted_fn();

CREATE FUNCTION office_file_versions_page_file_deleted_fn() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM office_file_versions WHERE file_kind = 'page_file' AND file_id = OLD.id;
    RETURN NULL;
END;
$$;
CREATE TRIGGER office_file_versions_page_file_deleted
    AFTER DELETE ON page_files
    FOR EACH ROW
    EXECUTE FUNCTION office_file_versions_page_file_deleted_fn();

-- The scope triggers keep their conditions (a trigger WHEN cannot hold a subquery); their
-- functions now allow a new `blob_id` when the blob belongs to the file's workspace.
CREATE OR REPLACE FUNCTION attachment_references_scope_immutable_fn() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.workspace_id <> OLD.workspace_id
        OR NEW.task_id <> OLD.task_id
        OR NEW.comment_id IS DISTINCT FROM OLD.comment_id
        OR NOT EXISTS (
            SELECT 1 FROM attachment_blobs
            WHERE attachment_blobs.id = NEW.blob_id AND attachment_blobs.workspace_id = NEW.workspace_id
        )
    THEN
        RAISE EXCEPTION 'attachment reference scope is immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION page_files_scope_immutable_fn() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.workspace_id <> OLD.workspace_id
        OR NEW.page_id <> OLD.page_id
        OR NOT EXISTS (
            SELECT 1 FROM attachment_blobs
            WHERE attachment_blobs.id = NEW.blob_id AND attachment_blobs.workspace_id = NEW.workspace_id
        )
    THEN
        RAISE EXCEPTION 'page file scope is immutable';
    END IF;
    RETURN NEW;
END;
$$;
