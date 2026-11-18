-- Postgres: the same change as apps/server/migrations/20261118030000_office_save_failures.sql
-- When a save from the office editor fails, the file keeps a mark until the next save that
-- works, and the person who edited last gets an inbox notification (kind `office_save_failed`,
-- with the name of the file, on its task or its page).
--
-- `file_id` is an `attachment_references.id` or a `page_files.id`, by `file_kind`, so it has no
-- foreign key. The triggers below remove the row with its file.
CREATE TABLE office_save_failures (
    file_kind TEXT NOT NULL CHECK (file_kind IN ('attachment', 'page_file')),
    file_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    failed_at BIGINT NOT NULL,
    PRIMARY KEY (file_kind, file_id)
);

CREATE INDEX office_save_failures_workspace ON office_save_failures (workspace_id);

CREATE FUNCTION office_save_failures_attachment_deleted_fn() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM office_save_failures WHERE file_kind = 'attachment' AND file_id = OLD.id;
    RETURN NULL;
END;
$$;
CREATE TRIGGER office_save_failures_attachment_deleted
    AFTER DELETE ON attachment_references
    FOR EACH ROW
    EXECUTE FUNCTION office_save_failures_attachment_deleted_fn();

CREATE FUNCTION office_save_failures_page_file_deleted_fn() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM office_save_failures WHERE file_kind = 'page_file' AND file_id = OLD.id;
    RETURN NULL;
END;
$$;
CREATE TRIGGER office_save_failures_page_file_deleted
    AFTER DELETE ON page_files
    FOR EACH ROW
    EXECUTE FUNCTION office_save_failures_page_file_deleted_fn();

ALTER TABLE notifications ADD COLUMN office_file_name TEXT;

ALTER TABLE notifications DROP CONSTRAINT notifications_kind_check;
ALTER TABLE notifications DROP CONSTRAINT notifications_scope_check;
ALTER TABLE notifications ADD CONSTRAINT notifications_kind_check
    CHECK (kind IN ('task_assigned', 'comment_mentioned', 'description_mentioned',
                    'page_comment_mentioned', 'page_mentioned', 'chat_mentioned',
                    'import_finished', 'office_save_failed'));
ALTER TABLE notifications ADD CONSTRAINT notifications_scope_check
    CHECK (
        (kind IN ('task_assigned', 'comment_mentioned', 'description_mentioned')
            AND task_id IS NOT NULL
            AND page_id IS NULL AND page_thread_id IS NULL AND page_comment_id IS NULL
            AND page_block_id IS NULL)
        OR (kind = 'page_comment_mentioned'
            AND task_id IS NULL AND comment_id IS NULL
            AND page_id IS NOT NULL AND page_thread_id IS NOT NULL AND page_comment_id IS NOT NULL
            AND page_block_id IS NULL)
        OR (kind = 'page_mentioned'
            AND task_id IS NULL AND comment_id IS NULL
            AND page_id IS NOT NULL AND page_thread_id IS NULL AND page_comment_id IS NULL)
        OR (kind IN ('chat_mentioned', 'import_finished')
            AND task_id IS NULL AND comment_id IS NULL
            AND page_id IS NULL AND page_thread_id IS NULL AND page_comment_id IS NULL
            AND page_block_id IS NULL)
        OR (kind = 'office_save_failed'
            AND comment_id IS NULL AND page_thread_id IS NULL AND page_comment_id IS NULL
            AND page_block_id IS NULL
            AND (task_id IS NULL) <> (page_id IS NULL))
    );
ALTER TABLE notifications ADD CONSTRAINT notifications_office_check
    CHECK ((kind = 'office_save_failed') = (office_file_name IS NOT NULL));
