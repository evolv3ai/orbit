//! Versions of an office file. A task attachment or a Docs page file points at one blob and has
//! no history, so a save from the editor would lose the earlier bytes once the blob is reclaimed.
//! [`OfficeFileVersions::save`] stores the new bytes, keeps the blob they replace as an
//! `office_file_versions` row, and points the file at the new blob, in one transaction.
//!
//! A version row holds its blob like any other reference (`orbit_platform::BLOB_REFERENCE_COUNT`),
//! so reconciliation keeps the bytes. Database triggers remove the rows with their file.

use orbit_platform::{Database, Id, Row, UploadError, UploadService};
use thiserror::Error;

/// How many earlier versions of one file are kept; a save drops the oldest ones over this.
/// (Open question 3 of the spec: this number, or a workspace setting.)
pub const MAX_VERSIONS_PER_FILE: i64 = 20;

/// The kinds of file the editor can save. A chat file is view only: it has no versions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfficeFileKind {
    /// An `attachment_references` row: a file of a task or of a task comment.
    Attachment,
    /// A `page_files` row: a file of a Docs page.
    PageFile,
}

impl OfficeFileKind {
    /// The value of `office_file_versions.file_kind`, also the `{kind}` of the office routes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Attachment => "attachment",
            Self::PageFile => "page_file",
        }
    }

    /// The table of the file and its size column.
    const fn table(self) -> (&'static str, &'static str) {
        match self {
            Self::Attachment => ("attachment_references", "byte_size"),
            Self::PageFile => ("page_files", "size_bytes"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveOutcome {
    /// The file already holds these bytes (a repeated save): nothing changed.
    Unchanged,
    /// The file now points at `blob_id`, and `previous_blob_id` is kept as a version.
    Saved { blob_id: Id, previous_blob_id: Id },
}

#[derive(Debug, Error)]
pub enum OfficeVersionError {
    #[error("the office file does not exist")]
    NotFound,
    #[error("the saved file exceeds the configured {limit}-byte limit")]
    TooLarge { limit: u64 },
    #[error(transparent)]
    Upload(#[from] UploadError),
    #[error("office file version database operation failed: {0}")]
    Database(#[from] sqlx::Error),
}

#[derive(Clone)]
pub struct OfficeFileVersions {
    database: Database,
    uploads: UploadService,
}

impl OfficeFileVersions {
    #[must_use]
    pub fn new(database: Database, uploads: UploadService) -> Self {
        Self { database, uploads }
    }

    /// Stores `bytes` as the new content of the file and keeps the blob they replace as a
    /// version, attributed to `saved_by`. Saving the bytes the file already holds changes nothing.
    /// The caller has checked that the save is allowed; this checks only the size limit and the
    /// workspace's storage quota.
    pub async fn save(
        &self,
        kind: OfficeFileKind,
        file_id: Id,
        bytes: &[u8],
        saved_by: Option<Id>,
    ) -> Result<SaveOutcome, OfficeVersionError> {
        let limit = self.uploads.limits().max_file_bytes();
        if u64::try_from(bytes.len()).map_or(true, |size| size > limit) {
            return Err(OfficeVersionError::TooLarge { limit });
        }
        let size = i64::try_from(bytes.len()).map_err(|_| UploadError::SizeOverflow)?;
        let (table, size_column) = kind.table();

        let finalization = self.uploads.begin_finalization().await;
        let now = self.database.database_now().await?;
        let mut tx = self.database.immediate_transaction().await?;
        let file = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT workspace_id, blob_id FROM {table} WHERE id = $1"
        )))
        .bind(file_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(OfficeVersionError::NotFound)?;
        let workspace_id: String = file.try_get("workspace_id")?;
        let previous_blob: String = file.try_get("blob_id")?;
        let workspace_id: Id = workspace_id
            .parse()
            .map_err(|_| UploadError::InvalidState)?;
        let previous_blob_id: Id = previous_blob
            .parse()
            .map_err(|_| UploadError::InvalidState)?;

        let blob = finalization
            .finalize_bytes_in_transaction(&mut tx, workspace_id, bytes, now)
            .await?;
        // The file holds what the editor saved: an earlier failed save no longer counts.
        sqlx::query("DELETE FROM office_save_failures WHERE file_kind = $1 AND file_id = $2")
            .bind(kind.as_str())
            .bind(file_id.to_string())
            .execute(&mut *tx)
            .await?;
        if blob.id == previous_blob_id {
            tx.commit().await?;
            return Ok(SaveOutcome::Unchanged);
        }

        sqlx::query(
            "INSERT INTO office_file_versions \
             (id, workspace_id, file_kind, file_id, blob_id, saved_by, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(Id::new_v7().to_string())
        .bind(workspace_id.to_string())
        .bind(kind.as_str())
        .bind(file_id.to_string())
        .bind(&previous_blob)
        .bind(saved_by.map(|id| id.to_string()))
        .bind(now.as_millis())
        .execute(&mut *tx)
        .await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET blob_id = $1, {size_column} = $2 WHERE id = $3"
        )))
        .bind(blob.id.to_string())
        .bind(size)
        .bind(file_id.to_string())
        .execute(&mut *tx)
        .await?;
        // Over the limit, the oldest versions go. Reconciliation reclaims their blobs when
        // nothing else holds them.
        sqlx::query(
            "DELETE FROM office_file_versions WHERE file_kind = $1 AND file_id = $2 AND id NOT IN (\
                SELECT id FROM office_file_versions WHERE file_kind = $1 AND file_id = $2 \
                ORDER BY created_at DESC, id DESC LIMIT $3)",
        )
        .bind(kind.as_str())
        .bind(file_id.to_string())
        .bind(MAX_VERSIONS_PER_FILE)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(SaveOutcome::Saved {
            blob_id: blob.id,
            previous_blob_id,
        })
    }
}
