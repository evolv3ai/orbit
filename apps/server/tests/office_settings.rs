//! The office Document Server's settings in Admin (`/api/v1/admin/settings/office`): who may change them, what a
//! save checks, the health check, `ORBIT_ONLYOFFICE_*`, and the content security policy that follows them.

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::routing::get;
use orbit_platform::{
    AttachmentMutationCoordinator, Config, Database, HttpPlatformLayer, LocalBlobStore,
    OriginPolicy, PasswordService, TestDatabase, TimestampMillis, UploadLimits, UploadService,
};
use orbit_server::auth_routes::CookieMode;
use orbit_server::office::routes::{OfficeState, office_router};
use orbit_server::office::settings::{self, EnvironmentOffice, OfficeSettingsRepository};
use orbit_server::repositories::identity::{IdentityRepository, SetupRequest};
use serde_json::{Value, json};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:8080";
const SETTINGS: &str = "/api/v1/admin/settings/office";
const TEST: &str = "/api/v1/admin/settings/office/test";
const APP_KEY: [u8; 32] = [7; 32];

/// Completes the setup and returns the root user's session cookie.
async fn root_cookie(database: &Database) -> String {
    let identity = IdentityRepository::new(database.clone());
    let now = TimestampMillis::now();
    identity
        .store_setup_token(
            "operator-secret",
            TimestampMillis::from_millis(now.as_millis() + 60_000),
        )
        .await
        .unwrap();
    let token = identity
        .complete_setup(
            SetupRequest {
                token: "operator-secret".to_owned(),
                email: "root@example.com".to_owned(),
                display_name: "Root".to_owned(),
                password_hash: PasswordService::default()
                    .hash("correct horse battery")
                    .unwrap(),
                workspace_name: "Orbit".to_owned(),
                project_name: "General".to_owned(),
            },
            now,
        )
        .await
        .unwrap()
        .session
        .token;
    format!("__Host-orbit_session={token}")
}

/// The office routes behind the HTTP layer, with the content security policy that the settings drive.
fn app(database: &Database, app_key: Option<[u8; 32]>) -> Router {
    // The settings routes store no file: the folder is never created.
    let uploads = UploadService::new(
        database.clone(),
        Arc::new(LocalBlobStore::new(
            std::env::temp_dir().join("orbit-office-settings-test-unused"),
        )),
        AttachmentMutationCoordinator::default(),
        UploadLimits::default(),
    );
    office_router(OfficeState::new(
        Arc::new(IdentityRepository::new(database.clone())),
        uploads,
        CookieMode::secure(),
        app_key,
        ORIGIN,
    ))
    .layer(
        HttpPlatformLayer::new(OriginPolicy::new(ORIGIN))
            .with_csp_office_origin(settings::csp_handle(database)),
    )
}

struct Answer {
    status: StatusCode,
    policy: String,
    body: Value,
}

async fn call(app: &Router, cookie: &str, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN);
    let request = match body {
        Some(body) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let policy = response
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .map(|value| value.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    Answer {
        status,
        policy,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

/// A server that answers `/healthcheck` with `body`; returns its `http://127.0.0.1:{port}` URL.
async fn document_server(body: &'static str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/healthcheck", get(move || async move { body })),
        )
        .await
        .unwrap();
    });
    format!("http://{address}")
}

#[tokio::test]
async fn an_administrator_saves_and_removes_the_document_server() {
    let database = TestDatabase::new().await.unwrap();
    let cookie = root_cookie(&database).await;
    let app = app(&database, Some(APP_KEY));

    let empty = json!({
        "enabled": false, "url": null, "internal_url": null, "orbit_url": null,
        "jwt_secret_set": false, "set_by_environment": false,
    });
    let answer = call(&app, &cookie, "GET", SETTINGS, None).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.body, empty);
    assert!(
        !answer.policy.contains("office.example.com"),
        "{}",
        answer.policy
    );

    // The first save needs the secret.
    let answer = call(
        &app,
        &cookie,
        "PUT",
        SETTINGS,
        Some(json!({"enabled": true, "url": "https://office.example.com"})),
    )
    .await;
    assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(answer.body["code"], "invalid_office_settings");

    let answer = call(
        &app,
        &cookie,
        "PUT",
        SETTINGS,
        Some(json!({
            "enabled": true,
            "url": " https://office.example.com/ ",
            "internal_url": "http://documentserver/",
            "orbit_url": "",
            "jwt_secret": "shared-secret",
        })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(
        answer.body,
        json!({
            "enabled": true, "url": "https://office.example.com",
            "internal_url": "http://documentserver", "orbit_url": null,
            "jwt_secret_set": true, "set_by_environment": false,
        })
    );
    // The answer never holds the secret, and the stored one is encrypted.
    assert!(!answer.body.to_string().contains("shared-secret"));
    let saved = OfficeSettingsRepository::new((*database).clone())
        .get()
        .await
        .unwrap()
        .unwrap();
    assert_ne!(saved.jwt_secret, b"shared-secret");
    assert_eq!(saved.jwt_secret(&APP_KEY).unwrap(), "shared-secret");
    assert_eq!(saved.server_url(), "http://documentserver");

    // From the next response on, pages may load the editor from the Document Server.
    let answer = call(&app, &cookie, "GET", SETTINGS, None).await;
    assert!(
        answer.policy.contains("https://office.example.com"),
        "{}",
        answer.policy
    );
    assert!(
        !answer.policy.contains("documentserver"),
        "{}",
        answer.policy
    );

    // A save without the secret keeps it; turning the integration off takes the origin out again.
    let answer = call(
        &app,
        &cookie,
        "PUT",
        SETTINGS,
        Some(json!({"enabled": false, "url": "https://office.example.com"})),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(answer.body["jwt_secret_set"], true);
    assert_eq!(answer.body["internal_url"], Value::Null);
    let answer = call(&app, &cookie, "GET", SETTINGS, None).await;
    assert!(
        !answer.policy.contains("office.example.com"),
        "{}",
        answer.policy
    );

    let answer = call(&app, &cookie, "DELETE", SETTINGS, None).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.body, empty);
    assert!(
        OfficeSettingsRepository::new((*database).clone())
            .get()
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_save_checks_its_values() {
    let database = TestDatabase::new().await.unwrap();
    let cookie = root_cookie(&database).await;
    let app = app(&database, Some(APP_KEY));
    let long_secret = "s".repeat(513);
    for (field, body) in [
        (
            "url",
            json!({"enabled": true, "url": "office.example.com", "jwt_secret": "s"}),
        ),
        (
            "url",
            json!({"enabled": true, "url": "https://u:p@office.example.com", "jwt_secret": "s"}),
        ),
        (
            "url",
            json!({"enabled": true, "url": "http://[::1]:8000", "jwt_secret": "s"}),
        ),
        (
            "internal_url",
            json!({"enabled": true, "url": "https://office.example.com",
                   "internal_url": "documentserver", "jwt_secret": "s"}),
        ),
        (
            "orbit_url",
            json!({"enabled": true, "url": "https://office.example.com",
                   "orbit_url": "http://orbit:8080/?a=1", "jwt_secret": "s"}),
        ),
        (
            "jwt_secret",
            json!({"enabled": true, "url": "https://office.example.com", "jwt_secret": long_secret}),
        ),
    ] {
        let answer = call(&app, &cookie, "PUT", SETTINGS, Some(body.clone())).await;
        assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(answer.body["code"], "invalid_office_settings", "{body}");
        assert!(
            answer.body["detail"].as_str().unwrap().starts_with(field),
            "{}",
            answer.body
        );
    }

    // Without an app key, Orbit cannot store the secret.
    let keyless = self::app(&database, None);
    let answer = call(
        &keyless,
        &cookie,
        "PUT",
        SETTINGS,
        Some(json!({"enabled": true, "url": "https://office.example.com", "jwt_secret": "s"})),
    )
    .await;
    assert_eq!(answer.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(answer.body["code"], "app_key_missing");
}

#[tokio::test]
async fn only_an_installation_administrator_reaches_the_settings() {
    let database = TestDatabase::new().await.unwrap();
    let cookie = root_cookie(&database).await;
    let app = app(&database, Some(APP_KEY));

    for (method, uri) in [("GET", SETTINGS), ("DELETE", SETTINGS), ("POST", TEST)] {
        let answer = call(&app, "__Host-orbit_session=nobody", method, uri, None).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }

    sqlx::query("UPDATE users SET installation_admin = 0")
        .execute(database.pool())
        .await
        .unwrap();
    for (method, uri, body) in [
        ("GET", SETTINGS, None),
        (
            "PUT",
            SETTINGS,
            Some(json!({"enabled": true, "url": "https://office.example.com", "jwt_secret": "s"})),
        ),
        ("DELETE", SETTINGS, None),
        ("POST", TEST, None),
    ] {
        let answer = call(&app, &cookie, method, uri, body).await;
        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(answer.body["code"], "installation_admin_required");
    }
}

#[tokio::test]
async fn the_test_asks_the_document_server_for_its_health_check() {
    let database = TestDatabase::new().await.unwrap();
    let cookie = root_cookie(&database).await;
    let app = app(&database, Some(APP_KEY));

    let answer = call(&app, &cookie, "POST", TEST, None).await;
    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(answer.body["code"], "office_not_configured");

    let save = |internal_url: String| {
        json!({
            "enabled": false, "url": "https://office.example.com",
            "internal_url": internal_url, "jwt_secret": "shared-secret",
        })
    };

    // A healthy Document Server, reached through the internal URL while the integration is still off.
    let healthy = document_server("true").await;
    let answer = call(&app, &cookie, "PUT", SETTINGS, Some(save(healthy))).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let answer = call(&app, &cookie, "POST", TEST, None).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);

    // Another web server at that address.
    let other = document_server("<html>hello</html>").await;
    call(&app, &cookie, "PUT", SETTINGS, Some(save(other.clone()))).await;
    let answer = call(&app, &cookie, "POST", TEST, None).await;
    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(answer.body["code"], "office_unreachable");
    assert!(
        answer.body["detail"].as_str().unwrap().contains(&other),
        "{}",
        answer.body
    );

    // A path without a health check, and nothing listening.
    call(
        &app,
        &cookie,
        "PUT",
        SETTINGS,
        Some(save(format!("{other}/missing"))),
    )
    .await;
    let answer = call(&app, &cookie, "POST", TEST, None).await;
    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert!(
        answer.body["detail"].as_str().unwrap().contains("404"),
        "{}",
        answer.body
    );
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    call(&app, &cookie, "PUT", SETTINGS, Some(save(closed))).await;
    let answer = call(&app, &cookie, "POST", TEST, None).await;
    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(answer.body["code"], "office_unreachable");
}

#[tokio::test]
async fn environment_variables_replace_the_saved_settings_and_lock_them() {
    let database = TestDatabase::new().await.unwrap();
    let cookie = root_cookie(&database).await;
    let app = app(&database, Some(APP_KEY));

    // A saved Document Server, before the variables are set.
    let answer = call(
        &app,
        &cookie,
        "PUT",
        SETTINGS,
        Some(json!({"enabled": true, "url": "https://saved.example.com", "jwt_secret": "saved-secret"})),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);

    let config = Config::from_environment(
        [
            ("ORBIT_ENV", "development"),
            (
                "ORBIT_APP_KEY",
                "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",
            ),
            ("ORBIT_ONLYOFFICE_URL", "https://office.example.com"),
            ("ORBIT_ONLYOFFICE_INTERNAL_URL", "http://documentserver"),
            ("ORBIT_ONLYOFFICE_ORBIT_URL", "http://orbit:8080"),
            ("ORBIT_ONLYOFFICE_JWT_SECRET", "environment-secret"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect(),
    )
    .unwrap();
    EnvironmentOffice::from_config(&config.instance, &APP_KEY)
        .unwrap()
        .install(&database);
    settings::reload(&database).await.unwrap();

    let answer = call(&app, &cookie, "GET", SETTINGS, None).await;
    assert_eq!(
        answer.body,
        json!({
            "enabled": true, "url": "https://office.example.com",
            "internal_url": "http://documentserver", "orbit_url": "http://orbit:8080",
            "jwt_secret_set": true, "set_by_environment": true,
        })
    );
    let answer = call(&app, &cookie, "GET", SETTINGS, None).await;
    assert!(
        answer.policy.contains("https://office.example.com"),
        "{}",
        answer.policy
    );
    assert!(
        !answer.policy.contains("saved.example.com"),
        "{}",
        answer.policy
    );
    let used = OfficeSettingsRepository::new((*database).clone())
        .get()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(used.jwt_secret(&APP_KEY).unwrap(), "environment-secret");

    for (method, body) in [
        (
            "PUT",
            Some(json!({"enabled": false, "url": "https://other.example.com", "jwt_secret": "s"})),
        ),
        ("DELETE", None),
    ] {
        let answer = call(&app, &cookie, method, SETTINGS, body).await;
        assert_eq!(answer.status, StatusCode::CONFLICT, "{method}");
        assert_eq!(answer.body["code"], "set_by_environment");
    }
    // The saved values stay, for when the variables are removed.
    let saved: String = sqlx::query_scalar("SELECT url FROM office_settings WHERE id = 1")
        .fetch_one(database.pool())
        .await
        .unwrap();
    assert_eq!(saved, "https://saved.example.com");
}
