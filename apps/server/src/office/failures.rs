//! A save from the editor that failed: the file keeps a mark until the next save that works, and
//! the person who edited last gets an inbox notification (`office_save_failed`).

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::HeaderMap;
use orbit_platform::{Database, Id, RequestId, Row, TimestampMillis};
use serde::Serialize;
use utoipa::ToSchema;

use super::editor::internal;
use super::files::{self, OfficeFile, OfficeFileParent};
use super::routes::OfficeState;
use super::versions::OfficeFileKind;
use crate::task_routes::{ApiError, authenticate_session};

/// The Document Server repeats a callback that Orbit refuses. One notification per file and
/// person in this time.
const NOTICE_WINDOW_MS: i64 = 10 * 60 * 1000;
/// The most marks one answer lists. There are few: a mark goes with the next save that works.
const MAX_LISTED: i64 = 200;

/// Marks `file` as "last save failed" and tells `editor`, the person who edited last, when they
/// are still a member of the workspace (and, for a file of a private page, its owner).
pub async fn record(
    database: &Database,
    file: &OfficeFile,
    editor: Option<Id>,
    now: TimestampMillis,
) -> Result<(), sqlx::Error> {
    let mut transaction = database.immediate_transaction().await?;
    sqlx::query(
        "INSERT INTO office_save_failures (file_kind, file_id, workspace_id, failed_at) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (file_kind, file_id) DO UPDATE SET failed_at = excluded.failed_at",
    )
    .bind(file.kind.as_str())
    .bind(file.id.to_string())
    .bind(file.workspace_id.to_string())
    .bind(now.as_millis())
    .execute(&mut *transaction)
    .await?;
    if let Some(editor) = editor {
        let (task_id, page_id) = match file.parent {
            OfficeFileParent::Task { task_id, .. } => (Some(task_id.to_string()), None),
            OfficeFileParent::Page { page_id } => (None, Some(page_id.to_string())),
        };
        let dedupe = format!("office_save_failed:{}:{editor}:", file.id);
        sqlx::query(
            "INSERT INTO notifications (id, workspace_id, recipient_user_id, actor_user_id, kind, \
             task_id, page_id, office_file_name, dedupe_key, created_at) \
             SELECT $1, $2, $3, $3, 'office_save_failed', $4, $5, $6, $7, $8 \
             WHERE EXISTS (SELECT 1 FROM memberships WHERE workspace_id = $2 AND user_id = $3) \
             AND (CAST($5 AS TEXT) IS NULL OR EXISTS (SELECT 1 FROM pages WHERE id = $5 \
                 AND workspace_id = $2 AND (owner_id IS NULL OR owner_id = $3))) \
             AND NOT EXISTS (SELECT 1 FROM notifications WHERE kind = 'office_save_failed' \
                 AND recipient_user_id = $3 AND dedupe_key LIKE $9 AND created_at > $10) \
             ON CONFLICT DO NOTHING",
        )
        .bind(Id::new_v7().to_string())
        .bind(file.workspace_id.to_string())
        .bind(editor.to_string())
        .bind(task_id)
        .bind(page_id)
        .bind(&file.name)
        .bind(format!("{dedupe}{}", now.as_millis()))
        .bind(now.as_millis())
        .bind(format!("{dedupe}%"))
        .bind(now.as_millis() - NOTICE_WINDOW_MS)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await
}

/// A file whose last save from the editor failed.
#[derive(Serialize, ToSchema)]
pub(crate) struct OfficeSaveFailureView {
    /// `attachment` (a file of a task or a comment) or `page_file`.
    kind: String,
    file_id: String,
    #[schema(value_type = String, format = DateTime)]
    failed_at: TimestampMillis,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct OfficeSaveFailuresView {
    files: Vec<OfficeSaveFailureView>,
}

/// The files you can open whose last save from the office editor failed: the file in Orbit does
/// not have the changes of that editing session. A file leaves the list with its next save that
/// works. At most 200 files, the latest failures first.
#[utoipa::path(get, path = "/api/v1/office/save-failures", responses((status = 200, body = OfficeSaveFailuresView)))]
pub(crate) async fn list_office_save_failures(
    State(state): State<OfficeState>,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
) -> Result<Json<OfficeSaveFailuresView>, ApiError> {
    let instance = "/api/v1/office/save-failures";
    let request_id = request_id.as_ref();
    let session = authenticate_session(
        &state.identity,
        state.cookie_mode,
        &headers,
        instance,
        request_id,
    )
    .await?;
    let database = state.identity.database();
    let rows = sqlx::query(
        "SELECT f.file_kind AS file_kind, f.file_id AS file_id, f.failed_at AS failed_at \
         FROM office_save_failures f JOIN memberships m ON m.workspace_id = f.workspace_id \
         AND m.user_id = $1 ORDER BY f.failed_at DESC, f.file_id LIMIT $2",
    )
    .bind(session.user.id.to_string())
    .bind(MAX_LISTED)
    .fetch_all(database.pool())
    .await
    .map_err(|error| internal(instance, request_id).cause(error))?;
    let mut listed = Vec::new();
    for row in rows {
        let kind: String = row.get("file_kind");
        let file_id: String = row.get("file_id");
        let (Some(parsed), Ok(id)) = (OfficeFileKind::parse(&kind), file_id.parse::<Id>()) else {
            continue;
        };
        let Some(file) = files::find(database, parsed, id)
            .await
            .map_err(|error| internal(instance, request_id).cause(error))?
        else {
            continue;
        };
        // Only for who can read the file.
        if files::access(database, &state.uploads, &session, &file)
            .await
            .map_err(|error| internal(instance, request_id).cause(error))?
            .is_none()
        {
            continue;
        }
        listed.push(OfficeSaveFailureView {
            kind,
            file_id,
            failed_at: TimestampMillis::from_millis(row.get("failed_at")),
        });
    }
    Ok(Json(OfficeSaveFailuresView { files: listed }))
}
