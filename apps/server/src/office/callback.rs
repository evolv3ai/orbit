//! `POST /api/v1/office/{kind}/{file_id}/callback`: the Document Server tells Orbit what happens
//! to a file that is open in the editor, and hands over the bytes to save. It has no session: the
//! token it signs with the shared secret is the credential, and the token's payload, not the
//! unsigned body, is what Orbit acts on.
//!
//! The answer is `{"error":0}` when Orbit has done its part; anything else makes the editor show
//! an error to the people in the document. Not in the OpenAPI document: no client of Orbit calls it.
//!
//! When a save fails, on the Document Server or in Orbit, the file gets a mark and the person who
//! edited last is told (`failures::record`): the file in Orbit stays as it was.

use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use orbit_platform::{Database, Id, RequestId, TimestampMillis};
use serde_json::{Value, json};
use thiserror::Error;

use super::failures;
use super::files::{self, OfficeFile, office_format};
use super::jwt;
use super::routes::OfficeState;
use super::settings::OfficeSettings;
use super::versions::{OfficeFileKind, OfficeFileVersions, OfficeVersionError, SaveOutcome};
use crate::audit::{self, AuditOutcome};
use crate::task_routes::request_id_value;

/// How long the Document Server may take to hand over a saved file.
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// The document is ready to save: everyone closed it.
const STATUS_SAVE: i64 = 2;
/// Saving failed on the Document Server.
const STATUS_SAVE_FAILED: i64 = 3;
/// A save while people still edit.
const STATUS_FORCE_SAVE: i64 = 6;
/// A save while people still edit failed on the Document Server.
const STATUS_FORCE_SAVE_FAILED: i64 = 7;

/// Why a callback is refused. Each is logged; the Document Server sees only that it failed.
#[derive(Debug, Error)]
enum Refusal {
    #[error("office editing is off, or the file does not exist")]
    NotFound,
    #[error("no token, or one that the shared secret did not sign")]
    Unsigned,
    /// The request is not what the Document Server sends for this file.
    #[error("{0}")]
    Invalid(&'static str),
    #[error("the address of the saved file is not on the Document Server")]
    ForeignSource,
    #[error("the Document Server did not hand over the saved file: {0}")]
    Fetch(String),
    #[error("the saved file is over the file size limit")]
    TooLarge,
    #[error("{0}")]
    Internal(String),
}

impl Refusal {
    /// A short name for the audit log.
    const fn name(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Unsigned => "unsigned",
            Self::Invalid(_) => "invalid",
            Self::ForeignSource => "foreign_source",
            Self::Fetch(_) => "fetch",
            Self::TooLarge => "too_large",
            Self::Internal(_) => "internal",
        }
    }

    const fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unsigned | Self::ForeignSource => StatusCode::FORBIDDEN,
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::Fetch(_) => StatusCode::BAD_GATEWAY,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

pub(crate) async fn office_callback(
    State(state): State<OfficeState>,
    Path((kind, file_id)): Path<(String, String)>,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
    body: Bytes,
) -> Response {
    let request_id = request_id_value(request_id.as_ref()).to_owned();
    match handle(&state, &kind, &file_id, &headers, &body, &request_id).await {
        Ok(()) => Json(json!({"error": 0})).into_response(),
        Err(refusal) => {
            tracing::warn!(
                kind,
                file_id,
                request_id,
                reason = %refusal,
                "office callback refused"
            );
            (refusal.status(), Json(json!({"error": 1}))).into_response()
        }
    }
}

async fn handle(
    state: &OfficeState,
    kind: &str,
    file_id: &str,
    headers: &HeaderMap,
    body: &[u8],
    request_id: &str,
) -> Result<(), Refusal> {
    let database = state.identity.database();
    let (Some(kind), Ok(file_id), Some(app_key)) = (
        OfficeFileKind::parse(kind),
        file_id.parse::<Id>(),
        state.app_key.as_ref(),
    ) else {
        return Err(Refusal::NotFound);
    };
    let settings = state
        .repository()
        .get()
        .await
        .map_err(internal)?
        .filter(|settings| settings.enabled)
        .ok_or(Refusal::NotFound)?;
    let secret = settings
        .jwt_secret(app_key)
        .ok_or_else(|| Refusal::Internal("the office secret cannot be decrypted".to_owned()))?;
    let now = database.database_now().await.map_err(internal)?;
    let claims = signed_claims(headers, body, &secret, now).ok_or(Refusal::Unsigned)?;

    let status = claims
        .get("status")
        .and_then(Value::as_i64)
        .ok_or(Refusal::Invalid("no status"))?;
    let key = claims
        .get("key")
        .and_then(Value::as_str)
        .ok_or(Refusal::Invalid("no key"))?;
    // A key of this file: `{file_id}-{digest}`, with a suffix after a renewal
    // (`OfficeFile::document_key`).
    if key
        .strip_prefix(&file_id.to_string())
        .is_none_or(|rest| !rest.starts_with('-'))
    {
        return Err(Refusal::Invalid("the key is of another file"));
    }

    match status {
        STATUS_SAVE | STATUS_FORCE_SAVE => {
            let file = files::find(database, kind, file_id)
                .await
                .map_err(internal)?
                .ok_or(Refusal::NotFound)?;
            let saved = save(
                state, &settings, &file, &claims, key, status, request_id, now,
            )
            .await;
            // The edits did not reach Orbit. A file that is gone has nobody to tell.
            if let Err(refusal) = &saved
                && !matches!(refusal, Refusal::NotFound)
            {
                save_failed(
                    database,
                    &file,
                    &claims,
                    json!({"kind": kind.as_str(), "status": status, "reason": refusal.name()}),
                    request_id,
                    now,
                )
                .await;
            }
            saved
        }
        STATUS_SAVE_FAILED | STATUS_FORCE_SAVE_FAILED => {
            let Some(file) = files::find(database, kind, file_id)
                .await
                .map_err(internal)?
            else {
                return Ok(());
            };
            tracing::warn!(
                kind = kind.as_str(),
                %file_id,
                key,
                status,
                "the office Document Server could not save a file"
            );
            save_failed(
                database,
                &file,
                &claims,
                json!({"kind": kind.as_str(), "status": status}),
                request_id,
                now,
            )
            .await;
            Ok(())
        }
        // Being edited, closed without changes, or a status Orbit does not act on.
        _ => Ok(()),
    }
}

/// The payload of the request's token: in the body's `token` field, or in the `Authorization`
/// header, where the Document Server wraps the body in `payload`.
fn signed_claims(
    headers: &HeaderMap,
    body: &[u8],
    secret: &str,
    now: TimestampMillis,
) -> Option<Value> {
    let body: Value = serde_json::from_slice(body).ok()?;
    if let Some(token) = body.get("token").and_then(Value::as_str) {
        return jwt::verify(token, secret, now);
    }
    let token = headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    let mut claims = jwt::verify(token.trim(), secret, now)?;
    Some(match claims.get_mut("payload") {
        Some(payload) if payload.is_object() => payload.take(),
        _ => claims,
    })
}

#[allow(clippy::too_many_arguments)]
async fn save(
    state: &OfficeState,
    settings: &OfficeSettings,
    file: &OfficeFile,
    claims: &Value,
    key: &str,
    status: i64,
    request_id: &str,
    now: TimestampMillis,
) -> Result<(), Refusal> {
    let database = state.identity.database();
    // The editor opens only these formats for editing (`OfficeFormat::editable`), and saves them
    // as they are.
    let format = office_format(&file.name)
        .filter(|format| format.editable)
        .ok_or(Refusal::Invalid("the file is read-only in the editor"))?;
    if claims
        .get("filetype")
        .and_then(Value::as_str)
        .is_some_and(|saved| !saved.eq_ignore_ascii_case(format.file_type))
    {
        return Err(Refusal::Invalid("the saved file has another format"));
    }
    let url = claims
        .get("url")
        .and_then(Value::as_str)
        .ok_or(Refusal::Invalid("no url"))?;
    if !from_document_server(settings, url) {
        return Err(Refusal::ForeignSource);
    }
    let bytes = fetch(url, state.uploads.limits().max_file_bytes()).await?;
    let saved_by = last_editor(database, claims).await.map_err(internal)?;

    let outcome = OfficeFileVersions::new(database.clone(), state.uploads.clone())
        .save(file.kind, file.id, &bytes, saved_by)
        .await
        .map_err(|error| match error {
            OfficeVersionError::NotFound => Refusal::NotFound,
            OfficeVersionError::TooLarge { .. } => Refusal::TooLarge,
            error => Refusal::Internal(error.to_string()),
        })?;
    // A repeated callback saves the same bytes: nothing changed, nothing to record.
    if outcome == SaveOutcome::Unchanged {
        return Ok(());
    }
    let stale_key = key != file.document_key();
    if stale_key {
        // The file got other bytes while it was open. They are kept as a version.
        tracing::warn!(
            kind = file.kind.as_str(),
            file_id = %file.id,
            key,
            "an office save replaced bytes that changed while the file was open"
        );
    }
    record(
        database,
        file,
        saved_by,
        "office.file_saved",
        AuditOutcome::Success,
        json!({
            "kind": file.kind.as_str(),
            "while_editing": status == STATUS_FORCE_SAVE,
            "replaced_newer_bytes": stale_key,
        }),
        request_id,
        now,
    )
    .await
    .map_err(internal)
}

/// A save failed: an `office.save_failed` audit event, the mark on the file and the notification
/// for the person who edited last. An error here is logged; the answer to the Document Server
/// stays what the save made it.
async fn save_failed(
    database: &Database,
    file: &OfficeFile,
    claims: &Value,
    metadata: Value,
    request_id: &str,
    now: TimestampMillis,
) {
    let editor = match last_editor(database, claims).await {
        Ok(editor) => editor,
        Err(error) => {
            tracing::warn!(file_id = %file.id, %error, "office save failure: no editor");
            None
        }
    };
    if let Err(error) = record(
        database,
        file,
        editor,
        "office.save_failed",
        AuditOutcome::Failure,
        metadata,
        request_id,
        now,
    )
    .await
    {
        tracing::warn!(file_id = %file.id, %error, "office save failure not audited");
    }
    if let Err(error) = failures::record(database, file, editor, now).await {
        tracing::warn!(file_id = %file.id, %error, "office save failure not recorded");
    }
}

/// True when `url` is on the configured Document Server (its public or its internal address).
/// Orbit fetches nothing else: a callback cannot make it ask another host.
fn from_document_server(settings: &OfficeSettings, url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() {
        return false;
    }
    let origin = url.origin();
    [
        Some(settings.url.as_str()),
        settings.internal_url.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|server| url::Url::parse(server).ok())
    .any(|server| server.origin() == origin)
}

/// The saved file, at most `limit` bytes of it.
async fn fetch(url: &str, limit: u64) -> Result<Vec<u8>, Refusal> {
    let failed = |error: reqwest::Error| Refusal::Fetch(error.without_url().to_string());
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        // A redirect could lead off the Document Server.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(failed)?;
    let mut response = client.get(url).send().await.map_err(failed)?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(Refusal::Fetch(format!("status {}", response.status())));
    }
    if response.content_length().is_some_and(|size| size > limit) {
        return Err(Refusal::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(failed)? {
        if u64::try_from(bytes.len() + chunk.len()).map_or(true, |size| size > limit) {
            return Err(Refusal::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err(Refusal::Fetch("the saved file is empty".to_owned()));
    }
    Ok(bytes)
}

/// `users[0]`, the last person who sent changes, when it is an account of this Orbit.
async fn last_editor(database: &Database, claims: &Value) -> Result<Option<Id>, sqlx::Error> {
    let Some(id) = claims
        .get("users")
        .and_then(|users| users.get(0))
        .and_then(Value::as_str)
        .and_then(|id| id.parse::<Id>().ok())
    else {
        return Ok(None);
    };
    let known: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = $1")
        .bind(id.to_string())
        .fetch_one(database.pool())
        .await?;
    Ok((known == 1).then_some(id))
}

#[allow(clippy::too_many_arguments)]
async fn record(
    database: &Database,
    file: &OfficeFile,
    actor_id: Option<Id>,
    action: &str,
    outcome: AuditOutcome,
    metadata: Value,
    request_id: &str,
    now: TimestampMillis,
) -> Result<(), sqlx::Error> {
    let mut transaction = database.immediate_transaction().await?;
    audit::record(
        &mut transaction,
        file.workspace_id,
        actor_id,
        action,
        outcome,
        "office_file",
        Some(file.id),
        request_id,
        metadata,
        now,
    )
    .await?;
    transaction.commit().await
}

fn internal(error: impl std::fmt::Display) -> Refusal {
    Refusal::Internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::from_document_server;
    use crate::office::settings::OfficeSettings;

    fn settings(internal_url: Option<&str>) -> OfficeSettings {
        OfficeSettings {
            enabled: true,
            url: "https://office.example.com".to_owned(),
            internal_url: internal_url.map(str::to_owned),
            orbit_url: None,
            jwt_secret: Vec::new(),
        }
    }

    #[test]
    fn saved_files_come_only_from_the_document_server() {
        let public = settings(None);
        assert!(from_document_server(
            &public,
            "https://office.example.com/cache/files/data/out.docx?md5=1"
        ));
        assert!(from_document_server(
            &public,
            "https://office.example.com:443/cache/out.docx"
        ));
        for url in [
            "http://office.example.com/cache/out.docx",
            "https://office.example.com:8443/cache/out.docx",
            "https://office.example.com.evil.example/out.docx",
            "https://user@office.example.com/out.docx",
            "https://evil.example/?https://office.example.com/",
            "http://169.254.169.254/latest/meta-data/",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert!(!from_document_server(&public, url), "{url}");
        }

        let internal = settings(Some("http://documentserver:8000"));
        assert!(from_document_server(
            &internal,
            "http://documentserver:8000/cache/out.docx"
        ));
        assert!(from_document_server(
            &internal,
            "https://office.example.com/cache/out.docx"
        ));
        assert!(!from_document_server(
            &internal,
            "http://documentserver/cache/out.docx"
        ));
    }
}
