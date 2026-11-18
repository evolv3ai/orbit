//! Migrations after the office ones keep what the office editor wrote. The fork changes an
//! upstream migration that rebuilds `notifications`, so that the kind `office_save_failed` and
//! its file name stay.

use orbit_platform::{Database, DatabaseConfig, MigrationRunner};

/// `20261118030000_office_save_failures`.
const OFFICE_SAVE_FAILURES: i64 = 20_261_118_030_000;

#[tokio::test]
async fn later_migrations_keep_a_failed_save_notification_and_still_take_new_ones() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(&DatabaseConfig::new(directory.path().join("db.sqlite")))
        .await
        .unwrap();
    MigrationRunner::embedded_through("test", OFFICE_SAVE_FAILURES)
        .run(&database)
        .await
        .unwrap();
    let mut transaction = database.transaction().await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO users (id, email, normalized_email, display_name, password_hash, created_at, updated_at)
         VALUES ('u', 'owner@example.com', 'owner@example.com', 'Owner', 'x', 1, 1);
         INSERT INTO workspaces (id, name, version, owner_membership_id, created_at, updated_at)
         VALUES ('w', 'Orbit', 0, 'm', 1, 1);
         INSERT INTO memberships (id, workspace_id, user_id, role, version, created_at, updated_at)
         VALUES ('m', 'w', 'u', 'owner', 0, 1, 1);
         INSERT INTO pages (id, workspace_id, owner_id, creator_id, updated_by, created_at, updated_at)
         VALUES ('p', 'w', 'u', 'u', 'u', 1, 1);
         INSERT INTO notifications (id, workspace_id, recipient_user_id, actor_user_id, kind, page_id,
                                    office_file_name, dedupe_key, created_at)
         VALUES ('n-before', 'w', 'u', 'u', 'office_save_failed', 'p', 'report.docx', 'before', 1);",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();

    MigrationRunner::embedded("test")
        .run(&database)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO notifications (id, workspace_id, recipient_user_id, actor_user_id, kind, page_id, \
         office_file_name, dedupe_key, created_at) \
         VALUES ('n-after', 'w', 'u', 'u', 'office_save_failed', 'p', 'budget.xlsx', 'after', 2)",
    )
    .execute(database.pool())
    .await
    .unwrap();
    let kept: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, kind, office_file_name FROM notifications ORDER BY created_at")
            .fetch_all(database.pool())
            .await
            .unwrap();
    assert_eq!(
        kept,
        [
            (
                "n-before".to_owned(),
                "office_save_failed".to_owned(),
                "report.docx".to_owned()
            ),
            (
                "n-after".to_owned(),
                "office_save_failed".to_owned(),
                "budget.xlsx".to_owned()
            ),
        ]
    );
    // The check that ties the kind to the file name is still there.
    let nameless = sqlx::query(
        "INSERT INTO notifications (id, workspace_id, recipient_user_id, actor_user_id, kind, page_id, \
         dedupe_key, created_at) VALUES ('n-bad', 'w', 'u', 'u', 'office_save_failed', 'p', 'bad', 3)",
    )
    .execute(database.pool())
    .await;
    assert!(nameless.is_err());
}
