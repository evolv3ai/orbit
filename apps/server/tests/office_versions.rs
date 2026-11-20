//! Office file versions (ONLYOFFICE integration, spec section 4.6): saving new bytes keeps the
//! previous blob as a version, and reconciliation must not reclaim it.

use std::sync::Arc;

use orbit_platform::{
    AttachmentMutationCoordinator, Id, LocalBlobStore, NewAttachmentReference, PasswordService,
    TestDatabase, TimestampMillis, UploadLimits, UploadService,
};
use orbit_server::office::versions::{
    MAX_VERSIONS_PER_FILE, OfficeFileKind, OfficeFileVersions, OfficeVersionError, SaveOutcome,
};
use orbit_server::repositories::identity::{IdentityRepository, SetupRequest};

const DAY_MILLIS: i64 = 24 * 60 * 60 * 1_000;

struct Fixture {
    _root: tempfile::TempDir,
    database: TestDatabase,
    store: LocalBlobStore,
    uploads: UploadService,
    versions: OfficeFileVersions,
    workspace_id: Id,
    owner_id: Id,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_limits(UploadLimits::default()).await
    }

    async fn with_limits(limits: UploadLimits) -> Self {
        let root = tempfile::tempdir().unwrap();
        let database = TestDatabase::new().await.unwrap();
        let identity = Arc::new(IdentityRepository::new((*database).clone()));
        let now = TimestampMillis::now();
        identity
            .store_setup_token(
                "operator-secret",
                TimestampMillis::from_millis(now.as_millis() + 60_000),
            )
            .await
            .unwrap();
        let setup = identity
            .complete_setup(
                SetupRequest {
                    token: "operator-secret".to_owned(),
                    email: "owner@example.com".to_owned(),
                    display_name: "Owner".to_owned(),
                    password_hash: PasswordService::default()
                        .hash("correct horse battery")
                        .unwrap(),
                    workspace_name: "Orbit".to_owned(),
                    project_name: "General".to_owned(),
                },
                now,
            )
            .await
            .unwrap();
        let store = LocalBlobStore::new(root.path().join("attachments"));
        let uploads = UploadService::new(
            (*database).clone(),
            Arc::new(store.clone()),
            AttachmentMutationCoordinator::default(),
            limits,
        );
        let versions = OfficeFileVersions::new((*database).clone(), uploads.clone());
        Self {
            _root: root,
            database,
            store,
            uploads,
            versions,
            workspace_id: setup.workspace_id,
            owner_id: setup.user_id,
        }
    }

    /// A task attachment with `bytes`. Returns the reference id.
    async fn attachment(&self, bytes: &[u8]) -> Id {
        let staged = self
            .uploads
            .stage(self.workspace_id, self.owner_id, "report.docx", bytes)
            .await
            .unwrap();
        let reference = NewAttachmentReference::new(Id::new_v7(), None);
        let id = reference.id;
        self.uploads.finalize(&staged, reference).await.unwrap();
        id
    }

    /// A Docs page with one file holding `bytes`. Returns the page id and the file id.
    async fn page_file(&self, bytes: &[u8]) -> (Id, Id) {
        let now = TimestampMillis::now().as_millis();
        let page_id = Id::new_v7();
        sqlx::query(
            "INSERT INTO pages (id, workspace_id, title, owner_id, creator_id, updated_by, created_at, updated_at) \
             VALUES ($1, $2, 'Budget', $3, $4, $5, $6, $7)",
        )
        .bind(page_id.to_string())
        .bind(self.workspace_id.to_string())
        .bind(self.owner_id.to_string())
        .bind(self.owner_id.to_string())
        .bind(self.owner_id.to_string())
        .bind(now)
        .bind(now)
        .execute(self.database.pool())
        .await
        .unwrap();
        let staged = self
            .uploads
            .stage(self.workspace_id, self.owner_id, "budget.xlsx", bytes)
            .await
            .unwrap();
        let finalization = self.uploads.begin_finalization().await;
        let prepared = finalization.prepare(&staged).await.unwrap();
        let mut tx = self.database.immediate_transaction().await.unwrap();
        let blob = finalization
            .finalize_blob_in_transaction(&mut tx, &prepared, TimestampMillis::from_millis(now))
            .await
            .unwrap();
        let file_id = Id::new_v7();
        sqlx::query(
            "INSERT INTO page_files (id, workspace_id, page_id, blob_id, file_name, mime_type, \
             size_bytes, uploaded_by, created_at) VALUES ($1, $2, $3, $4, 'budget.xlsx', \
             'application/octet-stream', $5, $6, $7)",
        )
        .bind(file_id.to_string())
        .bind(self.workspace_id.to_string())
        .bind(page_id.to_string())
        .bind(blob.id.to_string())
        .bind(i64::try_from(bytes.len()).unwrap())
        .bind(self.owner_id.to_string())
        .bind(now)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        (page_id, file_id)
    }

    async fn save(&self, kind: OfficeFileKind, file_id: Id, bytes: &[u8]) -> SaveOutcome {
        self.versions
            .save(kind, file_id, bytes, Some(self.owner_id))
            .await
            .unwrap()
    }

    async fn text(&self, sql: &'static str, id: Id) -> String {
        sqlx::query_scalar(sql)
            .bind(id.to_string())
            .fetch_one(self.database.pool())
            .await
            .unwrap()
    }

    async fn number(&self, sql: &'static str, id: Id) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id.to_string())
            .fetch_one(self.database.pool())
            .await
            .unwrap()
    }

    async fn version_count(&self, file_id: Id) -> i64 {
        self.number(
            "SELECT COUNT(*) FROM office_file_versions WHERE file_id = $1",
            file_id,
        )
        .await
    }

    async fn reconcile_after_quarantine(&self) -> u64 {
        let later = self.database.database_now().await.unwrap().as_millis() + 2 * DAY_MILLIS;
        self.uploads.reconcile(later).await.unwrap().deleted_blobs
    }
}

const ATTACHMENT_BLOB: &str = "SELECT blob_id FROM attachment_references WHERE id = $1";

#[tokio::test]
async fn a_save_points_the_attachment_at_the_new_bytes_and_keeps_the_old_blob_as_a_version() {
    let fixture = Fixture::new().await;
    let file_id = fixture.attachment(b"first draft").await;
    let old_blob = fixture.text(ATTACHMENT_BLOB, file_id).await;

    let outcome = fixture
        .save(OfficeFileKind::Attachment, file_id, b"second draft, longer")
        .await;

    let new_blob = fixture.text(ATTACHMENT_BLOB, file_id).await;
    assert_ne!(new_blob, old_blob);
    assert_eq!(
        outcome,
        SaveOutcome::Saved {
            blob_id: new_blob.parse().unwrap(),
            previous_blob_id: old_blob.parse().unwrap(),
        }
    );
    assert_eq!(
        fixture
            .number(
                "SELECT byte_size FROM attachment_references WHERE id = $1",
                file_id
            )
            .await,
        b"second draft, longer".len() as i64
    );
    let row = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT workspace_id, file_kind, blob_id, saved_by FROM office_file_versions WHERE file_id = $1",
    )
    .bind(file_id.to_string())
    .fetch_one(fixture.database.pool())
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            fixture.workspace_id.to_string(),
            "attachment".to_owned(),
            old_blob,
            Some(fixture.owner_id.to_string()),
        )
    );
}

#[tokio::test]
async fn a_save_points_the_page_file_at_the_new_bytes_and_keeps_the_old_blob_as_a_version() {
    let fixture = Fixture::new().await;
    let (_, file_id) = fixture.page_file(b"q1 numbers").await;
    let page_blob = "SELECT blob_id FROM page_files WHERE id = $1";
    let old_blob = fixture.text(page_blob, file_id).await;

    fixture
        .save(OfficeFileKind::PageFile, file_id, b"q1 and q2 numbers")
        .await;

    assert_ne!(fixture.text(page_blob, file_id).await, old_blob);
    assert_eq!(
        fixture
            .number("SELECT size_bytes FROM page_files WHERE id = $1", file_id)
            .await,
        b"q1 and q2 numbers".len() as i64
    );
    assert_eq!(
        fixture
            .text(
                "SELECT blob_id FROM office_file_versions WHERE file_kind = 'page_file' AND file_id = $1",
                file_id
            )
            .await,
        old_blob
    );
}

#[tokio::test]
async fn saving_the_same_bytes_again_creates_no_version() {
    let fixture = Fixture::new().await;
    let file_id = fixture.attachment(b"first draft").await;
    fixture
        .save(OfficeFileKind::Attachment, file_id, b"second draft")
        .await;
    let blob = fixture.text(ATTACHMENT_BLOB, file_id).await;

    // The Document Server can repeat a callback.
    let repeated = fixture
        .save(OfficeFileKind::Attachment, file_id, b"second draft")
        .await;

    assert_eq!(repeated, SaveOutcome::Unchanged);
    assert_eq!(fixture.version_count(file_id).await, 1);
    assert_eq!(fixture.text(ATTACHMENT_BLOB, file_id).await, blob);
}

#[tokio::test]
async fn reconciliation_keeps_a_blob_that_only_a_version_holds() {
    let fixture = Fixture::new().await;
    let file_id = fixture.attachment(b"first draft").await;
    fixture
        .save(OfficeFileKind::Attachment, file_id, b"second draft")
        .await;
    assert_eq!(fixture.store.blobs().await.unwrap().len(), 2);

    assert_eq!(fixture.reconcile_after_quarantine().await, 0);

    assert_eq!(fixture.store.blobs().await.unwrap().len(), 2);
}

#[tokio::test]
async fn only_the_newest_versions_of_a_file_are_kept() {
    let fixture = Fixture::new().await;
    let file_id = fixture.attachment(b"draft 0").await;
    let first_blob = fixture.text(ATTACHMENT_BLOB, file_id).await;
    let other_file = fixture.attachment(b"another file").await;
    fixture
        .save(
            OfficeFileKind::Attachment,
            other_file,
            b"another file, edited",
        )
        .await;

    for save in 1..=MAX_VERSIONS_PER_FILE + 2 {
        fixture
            .save(
                OfficeFileKind::Attachment,
                file_id,
                format!("draft {save}").as_bytes(),
            )
            .await;
    }

    assert_eq!(fixture.version_count(file_id).await, MAX_VERSIONS_PER_FILE);
    assert_eq!(
        fixture
            .number(
                "SELECT COUNT(*) FROM office_file_versions WHERE blob_id = $1",
                first_blob.parse().unwrap()
            )
            .await,
        0,
        "the oldest version goes first"
    );
    assert_eq!(fixture.version_count(other_file).await, 1);
    // The two dropped versions' blobs are reclaimed; the current blobs and kept versions stay.
    assert_eq!(fixture.reconcile_after_quarantine().await, 2);
}

#[tokio::test]
async fn versions_go_with_their_attachment_and_their_blobs_are_reclaimed() {
    let fixture = Fixture::new().await;
    let file_id = fixture.attachment(b"first draft").await;
    fixture
        .save(OfficeFileKind::Attachment, file_id, b"second draft")
        .await;

    sqlx::query("DELETE FROM attachment_references WHERE id = $1")
        .bind(file_id.to_string())
        .execute(fixture.database.pool())
        .await
        .unwrap();

    assert_eq!(fixture.version_count(file_id).await, 0);
    assert_eq!(fixture.reconcile_after_quarantine().await, 2);
    assert!(fixture.store.blobs().await.unwrap().is_empty());
}

#[tokio::test]
async fn versions_go_with_their_page_when_the_page_is_deleted() {
    let fixture = Fixture::new().await;
    let (page_id, file_id) = fixture.page_file(b"q1 numbers").await;
    fixture
        .save(OfficeFileKind::PageFile, file_id, b"q1 and q2 numbers")
        .await;

    // What the trash purge does; page_files follow through their foreign key.
    sqlx::query("DELETE FROM pages WHERE id = $1")
        .bind(page_id.to_string())
        .execute(fixture.database.pool())
        .await
        .unwrap();

    assert_eq!(fixture.version_count(file_id).await, 0);
    assert_eq!(fixture.reconcile_after_quarantine().await, 2);
}

#[tokio::test]
async fn saving_a_file_that_does_not_exist_is_refused_and_stores_nothing() {
    let fixture = Fixture::new().await;
    let attachment_id = fixture.attachment(b"first draft").await;

    // An attachment id is not a page file id.
    for (kind, id) in [
        (OfficeFileKind::Attachment, Id::new_v7()),
        (OfficeFileKind::PageFile, attachment_id),
    ] {
        let error = fixture
            .versions
            .save(kind, id, b"orphan bytes", None)
            .await
            .unwrap_err();
        assert!(matches!(error, OfficeVersionError::NotFound), "{error:?}");
    }
    assert_eq!(
        fixture
            .number(
                "SELECT COUNT(*) FROM attachment_blobs WHERE workspace_id = $1",
                fixture.workspace_id
            )
            .await,
        1
    );
}

#[tokio::test]
async fn a_save_over_the_file_size_limit_is_refused() {
    let fixture = Fixture::with_limits(UploadLimits::new(16, 4096).unwrap()).await;
    let file_id = fixture.attachment(b"small").await;

    let error = fixture
        .versions
        .save(
            OfficeFileKind::Attachment,
            file_id,
            b"seventeen bytes!!",
            None,
        )
        .await
        .unwrap_err();

    assert!(
        matches!(error, OfficeVersionError::TooLarge { limit: 16 }),
        "{error:?}"
    );
    assert_eq!(fixture.version_count(file_id).await, 0);
}

#[tokio::test]
async fn a_file_still_cannot_move_or_take_a_blob_of_another_workspace() {
    let fixture = Fixture::new().await;
    let attachment_id = fixture.attachment(b"first draft").await;
    let (_, page_file_id) = fixture.page_file(b"q1 numbers").await;
    let foreign_blob = Id::new_v7();
    sqlx::query(
        "INSERT INTO attachment_blobs (id, workspace_id, sha256, byte_size, storage_key, created_at, quarantine_until)          VALUES ($1, $2, 'foreign', 1, 'blobs/foreign', 0, 0)",
    )
    .bind(foreign_blob.to_string())
    .bind(Id::new_v7().to_string())
    .execute(fixture.database.pool())
    .await
    .unwrap();

    for (sql, id) in [
        (
            "UPDATE attachment_references SET blob_id = $1 WHERE id = $2",
            attachment_id,
        ),
        (
            "UPDATE page_files SET blob_id = $1 WHERE id = $2",
            page_file_id,
        ),
        (
            "UPDATE attachment_references SET task_id = $1 WHERE id = $2",
            attachment_id,
        ),
        (
            "UPDATE page_files SET page_id = $1 WHERE id = $2",
            page_file_id,
        ),
    ] {
        let error = sqlx::query(sql)
            .bind(foreign_blob.to_string())
            .bind(id.to_string())
            .execute(fixture.database.pool())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("scope is immutable"),
            "{sql}: {error}"
        );
    }
}
