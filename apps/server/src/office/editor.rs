//! `/api/v1/office/{kind}/{file_id}/…`: what the editor page and the Document Server ask Orbit
//! about one office file.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use orbit_platform::{Id, RequestId, TimestampMillis};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};

use super::files::{self, OfficeAccess, OfficeFile, OfficeFormat, office_format};
use super::jwt;
use super::routes::OfficeState;
use super::settings::OfficeSettings;
use super::token::{OfficeTokens, TOKEN_LIFETIME_MS};
use super::versions::OfficeFileKind;
use crate::task_routes::{ApiError, authenticate_session};

/// The longest `back` path the editor config carries.
const MAX_BACK_BYTES: usize = 2048;

pub(super) fn editor_router() -> Router<OfficeState> {
    Router::new().route(
        "/api/v1/office/{kind}/{file_id}/config",
        get(get_office_editor_config),
    )
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

fn internal(instance: &str, request_id: Option<&Extension<RequestId>>) -> ApiError {
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
