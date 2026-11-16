//! The two HTTP rules of the ONLYOFFICE integration: the Document Server's save callback passes
//! the origin check, and the content security policy names the Document Server only while the
//! integration is on.

use axum::Router;
use axum::body::Body;
use axum::http::header::CONTENT_SECURITY_POLICY;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use orbit_platform::{CspOfficeOrigin, HttpPlatformLayer, OriginPolicy};
use tower::ServiceExt;

/// The policy of a server without the integration (and without a bootstrap script hash).
const DEFAULT_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU='; style-src-attr 'unsafe-inline'; img-src 'self' data: blob: https:; frame-src blob: https://www.youtube-nocookie.com https://player.vimeo.com; media-src 'self' https:; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'self' https://github.com";

fn app(layer: HttpPlatformLayer) -> Router {
    let reached = || async { StatusCode::NO_CONTENT };
    Router::new()
        .route("/", get(|| async { StatusCode::OK }))
        .route(
            "/api/v1/office/{kind}/{file_id}/callback",
            post(reached).put(reached),
        )
        .route("/api/v1/office/{kind}/{file_id}/config", post(reached))
        .route(
            "/api/v1/office/{kind}/{file_id}/callback/{extra}",
            post(reached),
        )
        .route("/api/v1/workspaces/{id}/tasks", post(reached))
        .layer(layer)
}

fn layer() -> HttpPlatformLayer {
    HttpPlatformLayer::new(OriginPolicy::new("https://orbit.test"))
}

async fn status(app: &Router, method: &str, uri: &str, headers: &[(&str, &str)]) -> StatusCode {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

async fn csp(app: &Router) -> String {
    let response = app
        .clone()
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    response.headers()[CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn the_save_callback_reaches_its_handler_without_an_origin_or_a_cookie() {
    let app = app(layer());

    for uri in [
        "/api/v1/office/attachment/01a1e4fc-c0be-731a-b31b-de737b879976/callback",
        "/api/v1/office/page_file/01a1e4fc-c0be-731a-b31b-de737b879976/callback",
    ] {
        assert_eq!(
            status(&app, "POST", uri, &[]).await,
            StatusCode::NO_CONTENT,
            "{uri}"
        );
        // The Document Server sends its JWT as a bearer token, and may sit behind any origin.
        assert_eq!(
            status(
                &app,
                "POST",
                uri,
                &[
                    ("authorization", "Bearer jwt"),
                    ("origin", "https://office.test")
                ]
            )
            .await,
            StatusCode::NO_CONTENT,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn other_requests_without_an_origin_are_still_refused() {
    let app = app(layer());

    for (method, uri) in [
        ("POST", "/api/v1/workspaces/w/tasks"),
        // Only the callback of the office routes, and only as a POST.
        ("POST", "/api/v1/office/attachment/f/config"),
        ("POST", "/api/v1/office/attachment/f/callback/extra"),
        ("PUT", "/api/v1/office/attachment/f/callback"),
    ] {
        assert_eq!(
            status(&app, method, uri, &[]).await,
            StatusCode::FORBIDDEN,
            "{method} {uri}"
        );
        assert_eq!(
            status(&app, method, uri, &[("origin", "https://office.test")]).await,
            StatusCode::FORBIDDEN,
            "{method} {uri} from a foreign origin"
        );
    }
}

#[tokio::test]
async fn the_policy_names_the_document_server_only_while_the_integration_is_on() {
    let office = CspOfficeOrigin::default();
    let app = app(layer().with_csp_office_origin(office.clone()));
    assert_eq!(csp(&app).await, DEFAULT_CSP);

    office.set("https://office.example.com").unwrap();
    assert_eq!(
        csp(&app).await,
        DEFAULT_CSP
            .replace(
                "script-src 'self';",
                "script-src 'self' https://office.example.com;"
            )
            .replace(
                "frame-src blob:",
                "frame-src https://office.example.com blob:"
            )
    );

    office.clear();
    assert_eq!(csp(&app).await, DEFAULT_CSP);
}

#[tokio::test]
async fn the_document_server_joins_the_bootstrap_script_hash() {
    let office = CspOfficeOrigin::default();
    office.set("http://127.0.0.1:8085").unwrap();
    let app = app(layer()
        .with_csp_script_hash("sha256-YWJj")
        .with_csp_office_origin(office));

    let csp = csp(&app).await;

    assert!(
        csp.contains("; script-src 'self' 'sha256-YWJj' http://127.0.0.1:8085; "),
        "{csp}"
    );
    assert!(
        csp.contains("; frame-src http://127.0.0.1:8085 blob: "),
        "{csp}"
    );
}

#[test]
fn only_a_plain_http_origin_can_join_the_policy() {
    let office = CspOfficeOrigin::default();
    for invalid in [
        "",
        "office.example.com",
        "https://office.example.com/",
        "https://office.example.com/editor",
        "https://office.example.com; script-src *",
        "https://office.example.com 'unsafe-inline'",
        "https://*.example.com",
        "https://user@office.example.com",
        "javascript:alert(1)",
        "*",
    ] {
        assert!(office.set(invalid).is_err(), "{invalid:?}");
    }
    for valid in [
        "https://office.example.com",
        "http://documentserver:8080",
        "http://127.0.0.1",
    ] {
        assert!(office.set(valid).is_ok(), "{valid:?}");
    }
}
