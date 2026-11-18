//! `/api/v1/office/{kind}/{file_id}/…`: what the editor page and the Document Server ask Orbit
//! about one office file.

use axum::extract::{Extension, Path, Query, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use orbit_platform::{AuthorizedAttachment, Id, RequestId, TimestampMillis};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};

use super::files::{self, OfficeAccess, OfficeFile, OfficeFormat, office_format};
use super::jwt;
use super::routes::OfficeState;
use super::settings::OfficeSettings;
use super::token::{OfficeTokens, TOKEN_LIFETIME_MS};
use super::versions::OfficeFileKind;
use crate::attachment_routes::download_response;
use crate::task_routes::{ApiError, authenticate_session};

/// The longest `back` path the editor config carries.
const MAX_BACK_BYTES: usize = 2048;

pub(super) fn editor_router() -> Router<OfficeState> {
    Router::new()
        .route("/api/v1/office/status", get(get_office_status))
        .route(
            "/api/v1/office/save-failures",
            get(super::failures::list_office_save_failures),
        )
        .route(
            "/api/v1/office/{kind}/{file_id}/config",
            get(get_office_editor_config),
        )
        .route(
            "/api/v1/office/{kind}/{file_id}/key",
            post(renew_office_document_key),
        )
        .route(
            "/api/v1/office/{kind}/{file_id}/download",
            get(download_office_file),
        )
        .route(
            "/api/v1/office/{kind}/{file_id}/callback",
            post(super::callback::office_callback),
        )
}

/// Whether this Orbit opens office files in the editor.
#[derive(Serialize, ToSchema)]
pub(crate) struct OfficeStatusView {
    /// A Document Server is set and office editing is on. The apps then offer "Open in editor"
    /// on office files.
    enabled: bool,
}

/// Whether office editing is on, for everyone who is signed in. It says nothing else about the
/// Document Server.
#[utoipa::path(get, path = "/api/v1/office/status", responses((status = 200, body = OfficeStatusView)))]
pub(crate) async fn get_office_status(
    State(state): State<OfficeState>,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
) -> Result<Json<OfficeStatusView>, ApiError> {
    let instance = "/api/v1/office/status";
    let request_id = request_id.as_ref();
    authenticate_session(
        &state.identity,
        state.cookie_mode,
        &headers,
        instance,
        request_id,
    )
    .await?;
    let enabled = state
        .repository()
        .get()
        .await
        .map_err(|error| internal(instance, request_id).cause(error))?
        .is_some_and(|settings| settings.enabled)
        && state.app_key.is_some();
    Ok(Json(OfficeStatusView { enabled }))
}

/// What the editor page needs to open one file.
#[derive(Serialize, ToSchema)]
pub(crate) struct OfficeEditorView {
    /// The Document Server as browsers reach it. The page loads
    /// `{document_server_url}/web-apps/apps/api/documents/api.js` from it.
    document_server_url: String,
    /// The config for `new DocsAPI.DocEditor(…)`, signed in its `token` field. Pass it as it is.
    #[schema(value_type = Object)]
    config: Value,
}

#[derive(Deserialize, IntoParams)]
pub(crate) struct OfficeEditorQuery {
    /// The language of the editor, for example `en` or `pt-BR`.
    lang: Option<String>,
    /// `light` or `dark`.
    theme: Option<String>,
    /// The path in Orbit that the editor's "Open file location" goes back to, for example
    /// `/w/acme/tasks/OO-7`. It starts with one `/`.
    back: Option<String>,
}

/// The config that opens a file of a task (`attachment`) or of a Docs page (`page_file`) in the
/// office editor. The person edits when they may change the file and the editor saves its format
/// (`docx`, `xlsx`, `pptx`); otherwise the file opens read-only.
#[utoipa::path(get, path = "/api/v1/office/{kind}/{file_id}/config", params(("kind" = String, Path, description = "`attachment` or `page_file`"), ("file_id" = String, Path), OfficeEditorQuery), responses((status = 200, body = OfficeEditorView)))]
pub(crate) async fn get_office_editor_config(
    State(state): State<OfficeState>,
    Path((kind, file_id)): Path<(String, String)>,
    Query(query): Query<OfficeEditorQuery>,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
) -> Result<Json<OfficeEditorView>, ApiError> {
    let instance = format!("/api/v1/office/{kind}/{file_id}/config");
    let request_id = request_id.as_ref();
    let session = authenticate_session(
        &state.identity,
        state.cookie_mode,
        &headers,
        &instance,
        request_id,
    )
    .await?;
    let database = state.identity.database();
    let (Some(kind), Ok(file_id)) = (OfficeFileKind::parse(&kind), file_id.parse::<Id>()) else {
        return Err(not_found(&instance, request_id));
    };
    let file = files::find(database, kind, file_id)
        .await
        .map_err(|error| internal(&instance, request_id).cause(error))?
        .ok_or_else(|| not_found(&instance, request_id))?;
    // Who cannot read the file learns nothing about it, also not that office editing is off.
    let access = files::access(database, &state.uploads, &session, &file)
        .await
        .map_err(|error| internal(&instance, request_id).cause(error))?
        .ok_or_else(|| not_found(&instance, request_id))?;
    if file.deleted {
        return Err(not_found(&instance, request_id));
    }
    let settings = state
        .repository()
        .get()
        .await
        .map_err(|error| internal(&instance, request_id).cause(error))?
        .filter(|settings| settings.enabled)
        .ok_or_else(|| not_configured(&instance, request_id))?;
    let app_key = state
        .app_key
        .as_ref()
        .ok_or_else(|| not_configured(&instance, request_id))?;
    let secret = settings.jwt_secret(app_key).ok_or_else(|| {
        internal(&instance, request_id).cause("the office secret cannot be decrypted")
    })?;
    let format = office_format(&file.name).ok_or_else(|| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "office_file_unsupported",
            "Not an office file",
            "The office editor does not open this kind of file.",
            &instance,
            request_id,
        )
    })?;

    let now = database
        .database_now()
        .await
        .map_err(|error| internal(&instance, request_id).cause(error))?;
    let token = OfficeTokens::new(app_key).issue(
        file.kind,
        file.id,
        file.blob_id,
        TimestampMillis::from_millis(now.as_millis().saturating_add(TOKEN_LIFETIME_MS)),
    );
    let mut config = editor_config(&EditorConfig {
        file: &file,
        format,
        edit: access == OfficeAccess::Edit && format.editable,
        orbit_url: orbit_url(&settings, &state.public_origin),
        public_origin: state.public_origin.trim_end_matches('/'),
        download_token: &token,
        user_id: session.user.id,
        user_name: &session.user.display_name,
        query: &query,
    });
    let signed = jwt::sign(&config, &secret);
    config["token"] = Value::String(signed);
    Ok(Json(OfficeEditorView {
        document_server_url: settings.url,
        config,
    }))
}

/// A key that the editor refused.
#[derive(Deserialize, ToSchema)]
pub(crate) struct OfficeKeyRenewal {
    /// The `document.key` of the config that the editor would not open.
    outdated_key: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct OfficeKeyRenewalView {
    /// The file has a new key. `false`: the reported key was not the one in use, so the next
    /// config already has another key.
    renewed: bool,
}

/// The editor page reports that the Document Server refused the key of a file as outdated: it has
/// already seen a file saved under that key. That happens when a backup restore brings back the
/// bytes a file had before an edit. Orbit gives the file a new key, the same for everyone; ask for
/// the config again to open the file. Only people who may change the file get a new key: a new
/// key while others edit under the old one would split them. A key that the file no longer has
/// changes nothing, whoever reports it.
#[utoipa::path(post, path = "/api/v1/office/{kind}/{file_id}/key", params(("kind" = String, Path, description = "`attachment` or `page_file`"), ("file_id" = String, Path)), request_body = OfficeKeyRenewal, responses((status = 200, body = OfficeKeyRenewalView)))]
pub(crate) async fn renew_office_document_key(
    State(state): State<OfficeState>,
    Path((kind, file_id)): Path<(String, String)>,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
    Json(body): Json<OfficeKeyRenewal>,
) -> Result<Json<OfficeKeyRenewalView>, ApiError> {
    let instance = format!("/api/v1/office/{kind}/{file_id}/key");
    let request_id = request_id.as_ref();
    let session = authenticate_session(
        &state.identity,
        state.cookie_mode,
        &headers,
        &instance,
        request_id,
    )
    .await?;
    let database = state.identity.database();
    let (Some(kind), Ok(file_id)) = (OfficeFileKind::parse(&kind), file_id.parse::<Id>()) else {
        return Err(not_found(&instance, request_id));
    };
    let file = files::find(database, kind, file_id)
        .await
        .map_err(|error| internal(&instance, request_id).cause(error))?
        .filter(|file| !file.deleted)
        .ok_or_else(|| not_found(&instance, request_id))?;
    let access = files::access(database, &state.uploads, &session, &file)
        .await
        .map_err(|error| internal(&instance, request_id).cause(error))?
        .ok_or_else(|| not_found(&instance, request_id))?;
    // The file has another key by now: the next config opens it, whoever asks.
    if body.outdated_key != file.document_key() {
        return Ok(Json(OfficeKeyRenewalView { renewed: false }));
    }
    if access != OfficeAccess::Edit {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "office_file_read_only",
            "Read-only file",
            "Only people who can change this file can renew its editor key.",
            &instance,
            request_id,
        ));
    }
    let now = database
        .database_now()
        .await
        .map_err(|error| internal(&instance, request_id).cause(error))?;
    let renewed = files::renew_key(database, &file, &body.outdated_key, now)
        .await
        .map_err(|error| internal(&instance, request_id).cause(error))?;
    if renewed {
        tracing::info!(
            kind = file.kind.as_str(),
            file_id = %file.id,
            user_id = %session.user.id,
            "the editor key of an office file was renewed"
        );
    }
    Ok(Json(OfficeKeyRenewalView { renewed }))
}

#[derive(Deserialize)]
pub(crate) struct OfficeDownloadQuery {
    /// The office token of the config's `document.url`.
    t: Option<String>,
}

/// The bytes of a file, for the Document Server: it has no session, so the office token of the
/// address is what allows the download. The token names one version of the file; once the file
/// holds other bytes, or office editing is off, the answer is 404. Not in the OpenAPI document:
/// no client of Orbit calls it.
pub(crate) async fn download_office_file(
    State(state): State<OfficeState>,
    Path((kind, file_id)): Path<(String, String)>,
    Query(query): Query<OfficeDownloadQuery>,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
) -> Result<Response, ApiError> {
    let instance = format!("/api/v1/office/{kind}/{file_id}/download");
    let request_id = request_id.as_ref();
    let database = state.identity.database();
    let (Some(kind), Ok(file_id), Some(app_key), Some(token)) = (
        OfficeFileKind::parse(&kind),
        file_id.parse::<Id>(),
        state.app_key.as_ref(),
        query.t.as_deref(),
    ) else {
        return Err(not_found(&instance, request_id));
    };
    let now = database
        .database_now()
        .await
        .map_err(|error| internal(&instance, request_id).cause(error))?;
    let blob_id = OfficeTokens::new(app_key)
        .verify(token, kind, file_id, now)
        .ok_or_else(|| not_found(&instance, request_id))?;
    let settings = state
        .repository()
        .get()
        .await
        .map_err(|error| internal(&instance, request_id).cause(error))?
        .filter(|settings| settings.enabled)
        .ok_or_else(|| not_found(&instance, request_id))?;
    // The Document Server signs its requests when it is set up to. A signature that is there
    // must be its own.
    if let Some(authorization) = headers.get(AUTHORIZATION) {
        let secret = settings.jwt_secret(app_key).ok_or_else(|| {
            internal(&instance, request_id).cause("the office secret cannot be decrypted")
        })?;
        authorization
            .to_str()
            .ok()
            .and_then(|value| value.strip_prefix("Bearer "))
            .and_then(|token| jwt::verify(token.trim(), &secret, now))
            .ok_or_else(|| not_found(&instance, request_id))?;
    }
    let file = files::find(database, kind, file_id)
        .await
        .map_err(|error| internal(&instance, request_id).cause(error))?
        .filter(|file| file.blob_id == blob_id && !file.deleted)
        .ok_or_else(|| not_found(&instance, request_id))?;
    let download = state
        .uploads
        .download(|| async {
            Ok(AuthorizedAttachment {
                storage_key: file.storage_key,
                media_type: file.media_type,
                display_name: file.name,
                deleted: false,
            })
        })
        .await
        .map_err(|error| internal(&instance, request_id).cause(error))?;
    download_response(download)
        .ok_or_else(|| internal(&instance, request_id).cause("invalid stored file headers"))
}

/// Orbit as the Document Server reaches it, without a trailing slash.
pub(super) fn orbit_url<'a>(settings: &'a OfficeSettings, public_origin: &'a str) -> &'a str {
    settings
        .orbit_url
        .as_deref()
        .unwrap_or(public_origin)
        .trim_end_matches('/')
}

struct EditorConfig<'a> {
    file: &'a OfficeFile,
    format: OfficeFormat,
    edit: bool,
    orbit_url: &'a str,
    /// Orbit as browsers reach it.
    public_origin: &'a str,
    download_token: &'a str,
    user_id: Id,
    user_name: &'a str,
    query: &'a OfficeEditorQuery,
}

/// The config without its token. Every value the Document Server acts on is here, so the token
/// covers it.
fn editor_config(input: &EditorConfig<'_>) -> Value {
    let file = input.file;
    let base = format!(
        "{}/api/v1/office/{}/{}",
        input.orbit_url,
        file.kind.as_str(),
        file.id
    );
    let mut editor = json!({
        "callbackUrl": format!("{base}/callback"),
        "mode": if input.edit { "edit" } else { "view" },
        "user": {"id": input.user_id.to_string(), "name": input.user_name},
    });
    if let Some(lang) = input.query.lang.as_deref().filter(|lang| valid_lang(lang)) {
        editor["lang"] = json!(lang);
    }
    let mut customization = serde_json::Map::new();
    match input.query.theme.as_deref() {
        Some("light") => {
            customization.insert("uiTheme".to_owned(), json!("default-light"));
        }
        Some("dark") => {
            customization.insert("uiTheme".to_owned(), json!("default-dark"));
        }
        _ => {}
    }
    if let Some(back) = input.query.back.as_deref().filter(|back| valid_back(back)) {
        // Always a page of this Orbit: the path is appended to Orbit's own address.
        customization.insert(
            "goback".to_owned(),
            json!({"url": format!("{}{back}", input.public_origin)}),
        );
    }
    if !customization.is_empty() {
        editor["customization"] = Value::Object(customization);
    }
    json!({
        "documentType": input.format.document_type,
        "document": {
            "fileType": input.format.file_type,
            "key": file.document_key(),
            "title": file.name,
            "url": format!("{base}/download?t={}", input.download_token),
            "permissions": {
                "edit": input.edit,
                "review": input.edit,
                "comment": input.edit,
                "download": true,
                "print": true,
                "copy": true,
            },
        },
        "editorConfig": editor,
    })
}

/// A language tag such as `en` or `pt-BR`.
fn valid_lang(lang: &str) -> bool {
    (2..=12).contains(&lang.len())
        && lang
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// A path of this Orbit: one leading `/`, and nothing that a browser reads as another host.
fn valid_back(back: &str) -> bool {
    back.len() <= MAX_BACK_BYTES
        && back.starts_with('/')
        && !back.starts_with("//")
        && !back.contains('\\')
        && !back.bytes().any(|byte| byte.is_ascii_control())
}

fn not_found(instance: &str, request_id: Option<&Extension<RequestId>>) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "office_file_not_found",
        "File not found",
        "The file does not exist or you cannot open it.",
        instance,
        request_id,
    )
}

fn not_configured(instance: &str, request_id: Option<&Extension<RequestId>>) -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "office_not_configured",
        "Office editing is off",
        "This Orbit has no Document Server, or office editing is turned off.",
        instance,
        request_id,
    )
}

pub(super) fn internal(instance: &str, request_id: Option<&Extension<RequestId>>) -> ApiError {
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        "Internal server error",
        "An unexpected error occurred. Use the request ID when contacting support.",
        instance,
        request_id,
    )
}

#[cfg(test)]
mod tests {
    use super::{valid_back, valid_lang};

    #[test]
    fn the_way_back_is_a_path_of_this_orbit() {
        assert!(valid_back("/w/acme/tasks/OO-7?tab=files"));
        assert!(!valid_back("https://evil.example"));
        assert!(!valid_back("//evil.example"));
        assert!(!valid_back("/\\evil.example"));
        assert!(!valid_back("/a\nb"));
        assert!(!valid_back(""));
    }

    #[test]
    fn the_language_is_a_short_tag() {
        assert!(valid_lang("en"));
        assert!(valid_lang("pt-BR"));
        assert!(!valid_lang("e"));
        assert!(!valid_lang("en_US"));
        assert!(!valid_lang("\"><script>"));
    }
}
