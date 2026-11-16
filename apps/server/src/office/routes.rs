//! `/api/v1/admin/settings/office`: the installation administrators read, save, remove and test the office
//! Document Server (Admin → Office editing).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use orbit_platform::{RequestId, TimestampMillis};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::settings::{
    self, EnvironmentOffice, InvalidUrl, MAX_SECRET_BYTES, OfficeSettings, OfficeSettingsError,
    OfficeSettingsRepository, SavedOffice, SecretChange,
};
use crate::auth_routes::CookieMode;
use crate::repositories::identity::{AuthenticatedSession, IdentityRepository};
use crate::task_routes::{ApiError, ApiJson, authenticate_session, request_id_value};

const SETTINGS: &str = "/api/v1/admin/settings/office";
const TEST: &str = "/api/v1/admin/settings/office/test";
/// How long the Document Server may take to answer the health check.
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct OfficeState {
    identity: Arc<IdentityRepository>,
    cookie_mode: CookieMode,
    app_key: Option<[u8; 32]>,
}

impl OfficeState {
    #[must_use]
    pub fn new(
        identity: Arc<IdentityRepository>,
        cookie_mode: CookieMode,
        app_key: Option<[u8; 32]>,
    ) -> Self {
        Self {
            identity,
            cookie_mode,
            app_key,
        }
    }

    fn repository(&self) -> OfficeSettingsRepository {
        OfficeSettingsRepository::new(self.identity.database().clone())
    }
}

pub fn office_router(state: OfficeState) -> Router {
    Router::new()
        .route(
            SETTINGS,
            get(get_office_settings)
                .put(save_office_settings)
                .delete(remove_office_settings),
        )
        .route(TEST, post(test_office_connection))
        .with_state(state)
}

/// The office Document Server in use. The secret is never returned.
#[derive(Serialize, ToSchema)]
pub(crate) struct OfficeSettingsView {
    /// Office files open in the editor. `false` also while no Document Server is saved.
    enabled: bool,
    /// The Document Server as browsers reach it; `null` until one is saved.
    url: Option<String>,
    /// The Document Server as Orbit reaches it; `null`: `url`.
    internal_url: Option<String>,
    /// Orbit as the Document Server reaches it; `null`: Orbit's public address.
    orbit_url: Option<String>,
    /// A secret is saved.
    jwt_secret_set: bool,
    /// `ORBIT_ONLYOFFICE_*` variables set these values. Changing them here gives 409 `set_by_environment`.
    set_by_environment: bool,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OfficeSettingsBody {
    enabled: bool,
    /// The Document Server as browsers reach it, for example `https://office.example.com`.
    url: String,
    /// Absent or empty: Orbit uses `url`.
    internal_url: Option<String>,
    /// Absent or empty: the Document Server uses Orbit's public address.
    orbit_url: Option<String>,
    /// The Document Server's `JWT_SECRET`. Absent or empty: keep the saved secret. Required the first time.
    jwt_secret: Option<String>,
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

/// The session of an installation administrator.
async fn require_admin(
    state: &OfficeState,
    headers: &HeaderMap,
    instance: &str,
    request_id: Option<&Extension<RequestId>>,
) -> Result<AuthenticatedSession, ApiError> {
    let session = authenticate_session(
        &state.identity,
        state.cookie_mode,
        headers,
        instance,
        request_id,
    )
    .await?;
    if state
        .identity
        .is_installation_admin(session.user.id)
        .await
        .map_err(|error| internal(instance, request_id).cause(error))?
    {
        Ok(session)
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "installation_admin_required",
            "Installation administrator required",
            "Only an installation administrator may do this.",
            instance,
            request_id,
        ))
    }
}

async fn view(
    state: &OfficeState,
    instance: &str,
    request_id: Option<&Extension<RequestId>>,
) -> Result<Json<OfficeSettingsView>, ApiError> {
    let settings = state
        .repository()
        .get()
        .await
        .map_err(|error| internal(instance, request_id).cause(error))?;
    let set_by_environment = EnvironmentOffice::of(state.identity.database())
        .settings()
        .is_some();
    Ok(Json(match settings {
        Some(settings) => OfficeSettingsView {
            enabled: settings.enabled,
            url: Some(settings.url),
            internal_url: settings.internal_url,
            orbit_url: settings.orbit_url,
            jwt_secret_set: true,
            set_by_environment,
        },
        None => OfficeSettingsView {
            enabled: false,
            url: None,
            internal_url: None,
            orbit_url: None,
            jwt_secret_set: false,
            set_by_environment,
        },
    }))
}

/// 409 `set_by_environment` while `ORBIT_ONLYOFFICE_*` variables set the Document Server.
fn refuse_environment(
    state: &OfficeState,
    instance: &str,
    request_id: Option<&Extension<RequestId>>,
) -> Result<(), ApiError> {
    if EnvironmentOffice::of(state.identity.database())
        .settings()
        .is_none()
    {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::CONFLICT,
        "set_by_environment",
        "Set by the environment",
        "ORBIT_ONLYOFFICE_* sets this on the server. Change the environment variables and restart Orbit.",
        instance,
        request_id,
    ))
}

/// Makes this server and the other servers of the database use the saved settings.
async fn changed(
    state: &OfficeState,
    instance: &str,
    request_id: Option<&Extension<RequestId>>,
) -> Result<(), ApiError> {
    let database = state.identity.database();
    settings::reload(database)
        .await
        .map_err(|error| internal(instance, request_id).cause(error))?;
    crate::cluster::Cluster::of(database).send(crate::cluster::Message::OfficeChanged);
    Ok(())
}

#[utoipa::path(get, path = "/api/v1/admin/settings/office", responses((status = 200, body = OfficeSettingsView)))]
pub(crate) async fn get_office_settings(
    State(state): State<OfficeState>,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
) -> Result<Json<OfficeSettingsView>, ApiError> {
    require_admin(&state, &headers, SETTINGS, request_id.as_ref()).await?;
    view(&state, SETTINGS, request_id.as_ref()).await
}

/// Saves the Document Server. The content security policy follows at once: while `enabled` is `true`, pages
/// may load the editor from the origin of `url`.
#[utoipa::path(put, path = "/api/v1/admin/settings/office", request_body = OfficeSettingsBody, responses((status = 200, body = OfficeSettingsView)))]
pub(crate) async fn save_office_settings(
    State(state): State<OfficeState>,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
    ApiJson(body): ApiJson<OfficeSettingsBody>,
) -> Result<Json<OfficeSettingsView>, ApiError> {
    let request_id = request_id.as_ref();
    let session = require_admin(&state, &headers, SETTINGS, request_id).await?;
    refuse_environment(&state, SETTINGS, request_id)?;
    let invalid = |field: &str, detail: &str| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_office_settings",
            "Invalid office settings",
            "",
            SETTINGS,
            request_id,
        )
        .with_detail(format!("{field}: {detail}"))
    };
    let url = settings::clean_public_url(&body.url)
        .map_err(|InvalidUrl(detail)| invalid("url", detail))?;
    let optional_url = |field: &str, value: Option<String>| match value {
        Some(value) if !value.trim().is_empty() => settings::clean_url(&value)
            .map(Some)
            .map_err(|InvalidUrl(detail)| invalid(field, detail)),
        _ => Ok(None),
    };
    let internal_url = optional_url("internal_url", body.internal_url)?;
    let orbit_url = optional_url("orbit_url", body.orbit_url)?;
    let secret =
        match body.jwt_secret.filter(|secret| !secret.is_empty()) {
            None => SecretChange::Keep,
            Some(secret) if secret.len() > MAX_SECRET_BYTES => {
                return Err(invalid("jwt_secret", "use at most 512 characters."));
            }
            Some(secret) => {
                let key = state.app_key.as_ref().ok_or_else(|| {
                    ApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "app_key_missing",
                        "App key missing",
                        "The server has no app key to encrypt the secret with.",
                        SETTINGS,
                        request_id,
                    )
                })?;
                SecretChange::Set(crate::secret_box::encrypt_secret(key, &secret).map_err(
                    |()| internal(SETTINGS, request_id).cause("could not encrypt the secret"),
                )?)
            }
        };
    state
        .repository()
        .save(
            session.user.id,
            SavedOffice {
                enabled: body.enabled,
                url,
                internal_url,
                orbit_url,
            },
            secret,
            request_id_value(request_id),
            TimestampMillis::now(),
        )
        .await
        .map_err(|error| match error {
            OfficeSettingsError::SecretRequired => {
                invalid("jwt_secret", "enter the Document Server's JWT secret.")
            }
            OfficeSettingsError::Unavailable(error) => internal(SETTINGS, request_id).cause(error),
        })?;
    changed(&state, SETTINGS, request_id).await?;
    view(&state, SETTINGS, request_id).await
}

/// Removes the Document Server and its secret. Office files then only download.
#[utoipa::path(delete, path = "/api/v1/admin/settings/office", responses((status = 200, body = OfficeSettingsView)))]
pub(crate) async fn remove_office_settings(
    State(state): State<OfficeState>,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
) -> Result<Json<OfficeSettingsView>, ApiError> {
    let request_id = request_id.as_ref();
    let session = require_admin(&state, &headers, SETTINGS, request_id).await?;
    refuse_environment(&state, SETTINGS, request_id)?;
    state
        .repository()
        .clear(
            session.user.id,
            request_id_value(request_id),
            TimestampMillis::now(),
        )
        .await
        .map_err(|error| internal(SETTINGS, request_id).cause(error))?;
    changed(&state, SETTINGS, request_id).await?;
    view(&state, SETTINGS, request_id).await
}

/// Asks the Document Server in use for its health check (`GET {internal_url or url}/healthcheck`) and waits
/// for the answer. Works while the integration is off, so it can be tested before it is turned on.
#[utoipa::path(post, path = "/api/v1/admin/settings/office/test", responses((status = 204)))]
pub(crate) async fn test_office_connection(
    State(state): State<OfficeState>,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
) -> Result<StatusCode, ApiError> {
    let request_id = request_id.as_ref();
    require_admin(&state, &headers, TEST, request_id).await?;
    let settings = state
        .repository()
        .get()
        .await
        .map_err(|error| internal(TEST, request_id).cause(error))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "office_not_configured",
                "No Document Server",
                "Save a Document Server first.",
                TEST,
                request_id,
            )
        })?;
    health_check(&settings).await.map_err(|reason| {
        tracing::warn!(%reason, "office Document Server health check failed");
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "office_unreachable",
            "Document Server unreachable",
            "",
            TEST,
            request_id,
        )
        .with_detail(format!(
            "Orbit could not use the Document Server at {}: {reason}",
            settings.server_url()
        ))
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// A healthy ONLYOFFICE Document Server answers `GET /healthcheck` with 200 and the body `true`.
async fn health_check(settings: &OfficeSettings) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(HEALTH_CHECK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "the HTTP client could not start".to_owned())?;
    let response = client
        .get(format!("{}/healthcheck", settings.server_url()))
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "it did not answer in 10 seconds".to_owned()
            } else if error.is_connect() {
                "the connection failed".to_owned()
            } else {
                "the request failed".to_owned()
            }
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("the health check answered {}", status.as_u16()));
    }
    let body = response
        .text()
        .await
        .map_err(|_| "the health check's answer could not be read".to_owned())?;
    if body.trim() == "true" {
        Ok(())
    } else {
        Err(
            "the health check did not answer `true`: is this an ONLYOFFICE Document Server?"
                .to_owned(),
        )
    }
}
