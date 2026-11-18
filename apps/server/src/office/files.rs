//! The files the editor opens: where a file is, which bytes it holds now, and what a person may do
//! with it. Access follows the task or the Docs page of the file, through the checks that the
//! attachment and page file routes use: this module adds no rule of its own.

use orbit_platform::{Database, Id, Row, TimestampMillis, UploadService};
use thiserror::Error;

use super::versions::OfficeFileKind;
use crate::repositories::attachments::{AttachmentRepository, AttachmentRepositoryError};
use crate::repositories::identity::AuthenticatedSession;
use crate::repositories::page_files::{PageFileError, PageFileRepository};

impl OfficeFileKind {
    /// The kind that the `{kind}` of an office route names.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Attachment, Self::PageFile]
            .into_iter()
            .find(|kind| kind.as_str() == value)
    }
}

/// What a file belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfficeFileParent {
    /// A task, or a comment of it.
    Task {
        task_id: Id,
        comment_id: Option<Id>,
    },
    Page {
        page_id: Id,
    },
}

/// A file and the bytes it holds now.
#[derive(Clone, Debug)]
pub struct OfficeFile {
    pub kind: OfficeFileKind,
    pub id: Id,
    pub workspace_id: Id,
    pub parent: OfficeFileParent,
    pub blob_id: Id,
    /// Hex SHA-256 of the bytes.
    pub sha256: String,
    pub name: String,
    pub media_type: String,
    pub storage_key: String,
    /// An administrator deleted the bytes (Admin → Storage).
    pub deleted: bool,
    /// The stored suffix of the key of these bytes (`renew_key`), when there is one.
    pub key_suffix: Option<String>,
}

impl OfficeFile {
    /// The editor's `document.key`: it changes when the stored bytes change, and is the same for
    /// everyone who opens these bytes, so they edit together.
    #[must_use]
    pub fn document_key(&self) -> String {
        match &self.key_suffix {
            Some(suffix) => format!("{}-{suffix}", self.base_key()),
            None => self.base_key(),
        }
    }

    /// The key without a suffix: the file and its bytes.
    fn base_key(&self) -> String {
        let digest = self.sha256.get(..16).unwrap_or(&self.sha256);
        format!("{}-{digest}", self.id)
    }
}

/// What a person may do with a file in the editor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfficeAccess {
    View,
    Edit,
}

#[derive(Debug, Error)]
pub enum OfficeFileError {
    #[error("the office file has an invalid record")]
    InvalidRecord,
    #[error("office file database operation failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("office file access check failed: {0}")]
    Access(String),
}

/// The file, whoever asks. `None` when there is none.
pub async fn find(
    database: &Database,
    kind: OfficeFileKind,
    id: Id,
) -> Result<Option<OfficeFile>, OfficeFileError> {
    let sql = match kind {
        OfficeFileKind::Attachment => {
            "SELECT f.workspace_id AS workspace_id, f.task_id AS parent_id, f.comment_id AS comment_id, \
             f.blob_id AS blob_id, f.display_name AS name, f.media_type AS media_type, \
             b.sha256 AS sha256, b.storage_key AS storage_key, b.deleted_at AS deleted_at \
             FROM attachment_references f JOIN attachment_blobs b ON b.id = f.blob_id \
             AND b.workspace_id = f.workspace_id WHERE f.id = $1"
        }
        OfficeFileKind::PageFile => {
            "SELECT f.workspace_id AS workspace_id, f.page_id AS parent_id, CAST(NULL AS TEXT) AS comment_id, \
             f.blob_id AS blob_id, f.file_name AS name, f.mime_type AS media_type, \
             b.sha256 AS sha256, b.storage_key AS storage_key, b.deleted_at AS deleted_at \
             FROM page_files f JOIN attachment_blobs b ON b.id = f.blob_id \
             AND b.workspace_id = f.workspace_id WHERE f.id = $1"
        }
    };
    let Some(row) = sqlx::query(sql)
        .bind(id.to_string())
        .fetch_optional(database.pool())
        .await?
    else {
        return Ok(None);
    };
    let parent_id = parse_id(row.try_get("parent_id")?)?;
    let parent = match kind {
        OfficeFileKind::Attachment => OfficeFileParent::Task {
            task_id: parent_id,
            comment_id: row
                .try_get::<Option<String>, _>("comment_id")?
                .map(parse_id)
                .transpose()?,
        },
        OfficeFileKind::PageFile => OfficeFileParent::Page { page_id: parent_id },
    };
    let mut file = OfficeFile {
        kind,
        id,
        workspace_id: parse_id(row.try_get("workspace_id")?)?,
        parent,
        blob_id: parse_id(row.try_get("blob_id")?)?,
        sha256: row.try_get("sha256")?,
        name: row.try_get("name")?,
        media_type: row.try_get("media_type")?,
        storage_key: row.try_get("storage_key")?,
        deleted: row.try_get::<Option<i64>, _>("deleted_at")?.is_some(),
        key_suffix: None,
    };
    // A suffix belongs to the bytes it was made for: after a save it no longer applies.
    file.key_suffix = sqlx::query_scalar(
        "SELECT suffix FROM office_document_keys \
         WHERE file_kind = $1 AND file_id = $2 AND base_key = $3",
    )
    .bind(kind.as_str())
    .bind(id.to_string())
    .bind(file.base_key())
    .fetch_optional(database.pool())
    .await?;
    Ok(Some(file))
}

/// Gives `file` another editor key, when `outdated` is the key it has now. The Document Server
/// refuses a key that it has already seen saved, which a file gets back when a backup restore
/// brings back earlier bytes; an editor then reports the key.
///
/// The new key is stored, so everyone who opens the file gets the same one and they edit
/// together. `false`: `outdated` is not the key in use (the file has other bytes by now, or
/// someone else has already renewed the key), and nothing changes.
pub async fn renew_key(
    database: &Database,
    file: &OfficeFile,
    outdated: &str,
    now: TimestampMillis,
) -> Result<bool, OfficeFileError> {
    if outdated != file.document_key() {
        return Ok(false);
    }
    // The random end of a new id.
    let suffix: String = Id::new_v7()
        .to_string()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .rev()
        .take(12)
        .collect();
    let changed = match &file.key_suffix {
        // A row of other bytes is replaced. A row of these bytes means someone else was first.
        None => sqlx::query(
            "INSERT INTO office_document_keys \
             (file_kind, file_id, workspace_id, base_key, suffix, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (file_kind, file_id) DO UPDATE SET base_key = excluded.base_key, \
             suffix = excluded.suffix, updated_at = excluded.updated_at \
             WHERE office_document_keys.base_key <> excluded.base_key",
        )
        .bind(file.kind.as_str())
        .bind(file.id.to_string())
        .bind(file.workspace_id.to_string())
        .bind(file.base_key())
        .bind(&suffix)
        .bind(now.as_millis())
        .execute(database.pool())
        .await?
        .rows_affected(),
        Some(current) => sqlx::query(
            "UPDATE office_document_keys SET suffix = $1, updated_at = $2 \
             WHERE file_kind = $3 AND file_id = $4 AND base_key = $5 AND suffix = $6",
        )
        .bind(&suffix)
        .bind(now.as_millis())
        .bind(file.kind.as_str())
        .bind(file.id.to_string())
        .bind(file.base_key())
        .bind(current)
        .execute(database.pool())
        .await?
        .rows_affected(),
    };
    Ok(changed > 0)
}

/// What the person of `session` may do with `file`; `None` when they cannot read it.
///
/// - A file of a task: every member who sees the task may read and change it (they may also
///   replace or delete it).
/// - A file of a comment: the people who may edit the comment change it; the others read it.
/// - A file of a Docs page: every member who sees the page may read and change it.
pub async fn access(
    database: &Database,
    uploads: &UploadService,
    session: &AuthenticatedSession,
    file: &OfficeFile,
) -> Result<Option<OfficeAccess>, OfficeFileError> {
    match file.parent {
        OfficeFileParent::Task {
            task_id,
            comment_id,
        } => {
            let attachments = AttachmentRepository::new(database.clone(), uploads.clone());
            let checked = match comment_id {
                Some(comment_id) => {
                    attachments
                        .require_comment_editor(session, file.workspace_id, task_id, comment_id)
                        .await
                }
                None => {
                    attachments
                        .authorize_task(session, file.workspace_id, task_id)
                        .await
                }
            };
            match checked {
                Ok(()) => Ok(Some(OfficeAccess::Edit)),
                Err(AttachmentRepositoryError::Forbidden) => Ok(Some(OfficeAccess::View)),
                Err(AttachmentRepositoryError::NotFound) => Ok(None),
                Err(error) => Err(OfficeFileError::Access(error.to_string())),
            }
        }
        OfficeFileParent::Page { page_id } => {
            let pages = PageFileRepository::new(database.clone(), uploads.clone());
            match pages
                .authorize_upload(file.workspace_id, page_id, session.user.id)
                .await
            {
                Ok(()) => Ok(Some(OfficeAccess::Edit)),
                Err(PageFileError::NotFound) => Ok(None),
                Err(error) => Err(OfficeFileError::Access(error.to_string())),
            }
        }
    }
}

/// How the editor opens a file, by the extension of its name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OfficeFormat {
    /// The editor's `document.fileType`: the extension in lower case.
    pub file_type: &'static str,
    /// The editor's `documentType`.
    pub document_type: &'static str,
    /// The editor saves this format as it is. The other formats open read-only: the Document
    /// Server would save them in another format than the file has.
    pub editable: bool,
}

const FORMATS: &[OfficeFormat] = &[
    format("docx", "word", true),
    format("docm", "word", false),
    format("dotx", "word", false),
    format("doc", "word", false),
    format("odt", "word", false),
    format("rtf", "word", false),
    format("xlsx", "cell", true),
    format("xlsm", "cell", false),
    format("xls", "cell", false),
    format("ods", "cell", false),
    format("pptx", "slide", true),
    format("ppsx", "slide", false),
    format("ppt", "slide", false),
    format("odp", "slide", false),
    format("pdf", "pdf", false),
];

const fn format(
    file_type: &'static str,
    document_type: &'static str,
    editable: bool,
) -> OfficeFormat {
    OfficeFormat {
        file_type,
        document_type,
        editable,
    }
}

/// The format of a file with this name; `None` when the editor does not open it.
#[must_use]
pub fn office_format(file_name: &str) -> Option<OfficeFormat> {
    let (_, extension) = file_name.rsplit_once('.')?;
    FORMATS
        .iter()
        .copied()
        .find(|format| format.file_type.eq_ignore_ascii_case(extension))
}

fn parse_id(value: String) -> Result<Id, OfficeFileError> {
    value.parse().map_err(|_| OfficeFileError::InvalidRecord)
}

#[cfg(test)]
mod tests {
    use super::office_format;

    #[test]
    fn the_extension_chooses_the_editor() {
        let docx = office_format("Report.final.DOCX").unwrap();
        assert_eq!(
            (docx.file_type, docx.document_type, docx.editable),
            ("docx", "word", true)
        );
        let xls = office_format("budget.xls").unwrap();
        assert_eq!((xls.document_type, xls.editable), ("cell", false));
        assert_eq!(office_format("deck.pptx").unwrap().document_type, "slide");
        assert_eq!(office_format("scan.pdf").unwrap().document_type, "pdf");
    }

    #[test]
    fn other_files_do_not_open_in_the_editor() {
        assert_eq!(office_format("photo.png"), None);
        assert_eq!(office_format("docx"), None);
        assert_eq!(office_format("notes."), None);
    }
}
