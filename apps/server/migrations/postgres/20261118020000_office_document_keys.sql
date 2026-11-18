-- Postgres: the same change as apps/server/migrations/20261118020000_office_document_keys.sql
CREATE TABLE office_document_keys (
    file_kind TEXT NOT NULL CHECK (file_kind IN ('attachment', 'page_file')),
    file_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    base_key TEXT NOT NULL,
    suffix TEXT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (file_kind, file_id)
);

CREATE INDEX office_document_keys_workspace ON office_document_keys (workspace_id);

CREATE FUNCTION office_document_keys_attachment_deleted_fn() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM office_document_keys WHERE file_kind = 'attachment' AND file_id = OLD.id;
    RETURN NULL;
END;
$$;
CREATE TRIGGER office_document_keys_attachment_deleted
    AFTER DELETE ON attachment_references
    FOR EACH ROW
    EXECUTE FUNCTION office_document_keys_attachment_deleted_fn();

CREATE FUNCTION office_document_keys_page_file_deleted_fn() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM office_document_keys WHERE file_kind = 'page_file' AND file_id = OLD.id;
    RETURN NULL;
END;
$$;
CREATE TRIGGER office_document_keys_page_file_deleted
    AFTER DELETE ON page_files
    FOR EACH ROW
    EXECUTE FUNCTION office_document_keys_page_file_deleted_fn();
