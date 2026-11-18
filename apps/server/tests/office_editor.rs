//! The editor config of an office file (`GET /api/v1/office/{kind}/{file_id}/config`): who gets one, what it
//! allows, and what its signature covers.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::routing::get;
use orbit_platform::{
    AttachmentMutationCoordinator, AuthenticatedUser, Id, LocalBlobStore, NewAttachmentReference,
    PasswordService, TestDatabase, TimestampMillis, UploadLimits, UploadService,
};
use orbit_server::attachment_routes::{AttachmentState, attachment_router};
use orbit_server::auth_routes::CookieMode;
use orbit_server::office::jwt;
use orbit_server::office::routes::{OfficeState, office_router};
use orbit_server::office::token::OfficeTokens;
use orbit_server::office::versions::{OfficeFileKind, OfficeFileVersions};
use orbit_server::repositories::identity::{IdentityRepository, SetupRequest};
use orbit_server::task_routes::{TaskState, task_router};
use serde_json::{Value, json};
use tower::ServiceExt;

const PUBLIC_ORIGIN: &str = "https://orbit.example.com";
const APP_KEY: [u8; 32] = [7; 32];
const SECRET: &str = "shared-secret";

struct Fixture {
    _root: tempfile::TempDir,
    database: TestDatabase,
    identity: Arc<IdentityRepository>,
    uploads: UploadService,
    app: Router,
    workspace_id: Id,
    owner_id: Id,
    owner_cookie: String,
    task_id: Id,
}

impl Fixture {
    /// An Orbit with one task and a Document Server that is on.
    async fn new() -> Self {
        let fixture = Self::without_document_server().await;
        fixture.save_settings(true).await;
        fixture
    }

    async fn without_document_server() -> Self {
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
        let owner_cookie = format!("__Host-orbit_session={}", setup.session.token);
        let uploads = UploadService::new(
            (*database).clone(),
            Arc::new(LocalBlobStore::new(root.path().join("attachments"))),
            AttachmentMutationCoordinator::default(),
            UploadLimits::default(),
        );
        let app = task_router(TaskState::new(Arc::clone(&identity), CookieMode::secure()))
            .merge(attachment_router(AttachmentState::new(
                Arc::clone(&identity),
                uploads.clone(),
                CookieMode::secure(),
            )))
            .merge(office_router(OfficeState::new(
                Arc::clone(&identity),
                uploads.clone(),
                CookieMode::secure(),
                Some(APP_KEY),
                PUBLIC_ORIGIN,
            )));

        let status_id: String = sqlx::query_scalar(
            "SELECT id FROM task_statuses WHERE project_id = $1 ORDER BY position LIMIT 1",
        )
        .bind(setup.project_id.to_string())
        .fetch_one(database.pool())
        .await
        .unwrap();
        let (status, task) = send(
            &app,
            json_request(
                "POST",
                &format!("/api/v1/workspaces/{}/tasks", setup.workspace_id),
                &owner_cookie,
                &json!({"project_id": setup.project_id, "status_id": status_id, "title": "Report"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        Self {
            _root: root,
            database,
            identity,
            uploads,
            app,
            workspace_id: setup.workspace_id,
            owner_id: setup.user_id,
            owner_cookie,
            task_id: task["id"].as_str().unwrap().parse().unwrap(),
        }
    }

    /// Saves the Document Server in Admin. The Document Server reaches Orbit at `http://orbit:8080`.
    async fn save_settings(&self, enabled: bool) {
        self.save_settings_with(enabled, None).await;
    }

    /// As `save_settings`, with the address at which Orbit reaches the Document Server.
    async fn save_settings_with(&self, enabled: bool, internal_url: Option<&str>) {
        let (status, _) = send(
            &self.app,
            json_request(
                "PUT",
                "/api/v1/admin/settings/office",
                &self.owner_cookie,
                &json!({
                    "enabled": enabled,
                    "url": "https://office.example.com",
                    "internal_url": internal_url,
                    "orbit_url": "http://orbit:8080/",
                    "jwt_secret": SECRET,
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    /// A file of the task. Returns the attachment id.
    async fn attachment(&self, name: &str, bytes: &[u8]) -> Id {
        let staged = self
            .uploads
            .stage(self.workspace_id, self.owner_id, name, bytes)
            .await
            .unwrap();
        let reference = NewAttachmentReference::new(self.task_id, None);
        let id = reference.id;
        self.uploads.finalize(&staged, reference).await.unwrap();
        id
    }

    /// A Docs page of the owner's with one file. Returns the file id.
    async fn page_file(&self, name: &str, bytes: &[u8]) -> Id {
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
            .stage(self.workspace_id, self.owner_id, name, bytes)
            .await
            .unwrap();
        let finalization = self.uploads.begin_finalization().await;
        let mut tx = self.database.immediate_transaction().await.unwrap();
        let blob = finalization
            .finalize_blob_in_transaction(&mut tx, &staged, TimestampMillis::from_millis(now))
            .await
            .unwrap();
        let file_id = Id::new_v7();
        sqlx::query(
            "INSERT INTO page_files (id, workspace_id, page_id, blob_id, file_name, mime_type, \
             size_bytes, uploaded_by, created_at) VALUES ($1, $2, $3, $4, $5, \
             'application/octet-stream', $6, $7, $8)",
        )
        .bind(file_id.to_string())
        .bind(self.workspace_id.to_string())
        .bind(page_id.to_string())
        .bind(blob.id.to_string())
        .bind(name)
        .bind(i64::try_from(bytes.len()).unwrap())
        .bind(self.owner_id.to_string())
        .bind(now)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        file_id
    }

    /// Another account with a session. `role`: its role in the workspace, or `None` for no membership.
    async fn person(&self, name: &str, role: Option<&str>) -> String {
        let id = Id::new_v7();
        let email = format!("{}@example.com", name.to_lowercase());
        let now = TimestampMillis::now();
        sqlx::query(
            "INSERT INTO users (id, email, normalized_email, display_name, password_hash, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, 'unused', $5, $6)",
        )
        .bind(id.to_string())
        .bind(&email)
        .bind(&email)
        .bind(name)
        .bind(now.as_millis())
        .bind(now.as_millis())
        .execute(self.database.pool())
        .await
        .unwrap();
        if let Some(role) = role {
            sqlx::query(
                "INSERT INTO memberships (id, workspace_id, user_id, role, version, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, 0, $5, $6)",
            )
            .bind(Id::new_v7().to_string())
            .bind(self.workspace_id.to_string())
            .bind(id.to_string())
            .bind(role)
            .bind(now.as_millis())
            .bind(now.as_millis())
            .execute(self.database.pool())
            .await
            .unwrap();
        }
        let session = self
            .identity
            .create_session(
                &AuthenticatedUser {
                    id,
                    email,
                    display_name: name.to_owned(),
                },
                now,
            )
            .await
            .unwrap();
        format!("__Host-orbit_session={}", session.token)
    }

    async fn config(
        &self,
        cookie: &str,
        kind: &str,
        file_id: &str,
        query: &str,
    ) -> (StatusCode, Value) {
        let mut request =
            Request::builder().uri(format!("/api/v1/office/{kind}/{file_id}/config{query}"));
        if !cookie.is_empty() {
            request = request.header(header::COOKIE, cookie);
        }
        send(&self.app, request.body(Body::empty()).unwrap()).await
    }

    async fn blob_of_attachment(&self, id: Id) -> (Id, String) {
        let (blob, sha256): (String, String) = sqlx::query_as(
            "SELECT b.id, b.sha256 FROM attachment_references f JOIN attachment_blobs b ON b.id = f.blob_id \
             WHERE f.id = $1",
        )
        .bind(id.to_string())
        .fetch_one(self.database.pool())
        .await
        .unwrap();
        (blob.parse().unwrap(), sha256)
    }
}

fn json_request(method: &str, uri: &str, cookie: &str, value: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie)
        .body(Body::from(value.to_string()))
        .unwrap()
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn multipart_request(uri: &str, cookie: &str, name: &str, bytes: &[u8]) -> Request<Body> {
    let boundary = "orbit-test-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, cookie)
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn a_member_gets_a_signed_config_to_edit_a_task_file() {
    let fixture = Fixture::new().await;
    let id = fixture.attachment("Report.DOCX", b"first bytes").await;
    let (blob_id, sha256) = fixture.blob_of_attachment(id).await;

    let (status, body) = fixture
        .config(
            &fixture.owner_cookie,
            "attachment",
            &id.to_string(),
            "?lang=pt-BR&theme=dark&back=/w/orbit/tasks/1",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["document_server_url"], "https://office.example.com");
    let config = &body["config"];
    assert_eq!(config["documentType"], "word");
    assert_eq!(config["document"]["fileType"], "docx");
    assert_eq!(config["document"]["title"], "Report.DOCX");
    assert_eq!(config["document"]["key"], format!("{id}-{}", &sha256[..16]));
    assert_eq!(
        config["document"]["permissions"],
        json!({"edit": true, "review": true, "comment": true, "download": true, "print": true, "copy": true})
    );
    assert_eq!(config["editorConfig"]["mode"], "edit");
    assert_eq!(
        config["editorConfig"]["user"],
        json!({"id": fixture.owner_id.to_string(), "name": "Owner"})
    );
    assert_eq!(config["editorConfig"]["lang"], "pt-BR");
    assert_eq!(
        config["editorConfig"]["customization"]["uiTheme"],
        "default-dark"
    );
    // The Document Server calls Orbit at the address the settings give; the person goes back to the public one.
    assert_eq!(
        config["editorConfig"]["callbackUrl"],
        format!("http://orbit:8080/api/v1/office/attachment/{id}/callback")
    );
    assert_eq!(
        config["editorConfig"]["customization"]["goback"]["url"],
        "https://orbit.example.com/w/orbit/tasks/1"
    );

    // The download address carries a token for these bytes of this file.
    let url = config["document"]["url"].as_str().unwrap();
    let token = url
        .strip_prefix(&format!(
            "http://orbit:8080/api/v1/office/attachment/{id}/download?t="
        ))
        .expect(url);
    let tokens = OfficeTokens::new(&APP_KEY);
    let now = TimestampMillis::now();
    assert_eq!(
        tokens.verify(token, OfficeFileKind::Attachment, id, now),
        Some(blob_id)
    );
    assert_eq!(
        tokens.verify(
            token,
            OfficeFileKind::Attachment,
            id,
            TimestampMillis::from_millis(now.as_millis() + 6 * 60_000)
        ),
        None
    );

    // The token signs everything else in the config.
    let mut unsigned = config.clone();
    let signed = unsigned.as_object_mut().unwrap().remove("token").unwrap();
    assert_eq!(
        jwt::verify(signed.as_str().unwrap(), SECRET, now),
        Some(unsigned)
    );
    assert_eq!(jwt::verify(signed.as_str().unwrap(), "another", now), None);
}

#[tokio::test]
async fn the_key_is_shared_by_the_people_on_one_version_and_changes_with_the_bytes() {
    let fixture = Fixture::new().await;
    let id = fixture.attachment("report.docx", b"first bytes").await;
    let member = fixture.person("Member", Some("member")).await;
    let file = id.to_string();

    let (_, owner) = fixture
        .config(&fixture.owner_cookie, "attachment", &file, "")
        .await;
    let (status, other) = fixture.config(&member, "attachment", &file, "").await;
    assert_eq!(status, StatusCode::OK, "{other}");
    assert_eq!(
        owner["config"]["document"]["key"],
        other["config"]["document"]["key"]
    );
    assert_eq!(other["config"]["editorConfig"]["mode"], "edit");
    assert_eq!(other["config"]["editorConfig"]["user"]["name"], "Member");
    // Nothing optional was asked for.
    assert_eq!(other["config"]["editorConfig"].get("lang"), None);
    assert_eq!(other["config"]["editorConfig"].get("customization"), None);

    OfficeFileVersions::new((*fixture.database).clone(), fixture.uploads.clone())
        .save(
            OfficeFileKind::Attachment,
            id,
            b"second bytes",
            Some(fixture.owner_id),
        )
        .await
        .unwrap();
    let (_, saved) = fixture
        .config(&fixture.owner_cookie, "attachment", &file, "")
        .await;
    assert_ne!(
        owner["config"]["document"]["key"],
        saved["config"]["document"]["key"]
    );
    let (blob_id, _) = fixture.blob_of_attachment(id).await;
    let token = saved["config"]["document"]["url"]
        .as_str()
        .unwrap()
        .split_once("?t=")
        .unwrap()
        .1;
    assert_eq!(
        OfficeTokens::new(&APP_KEY).verify(
            token,
            OfficeFileKind::Attachment,
            id,
            TimestampMillis::now()
        ),
        Some(blob_id)
    );
}

impl Fixture {
    /// The `document.key` that `cookie` gets for a task file.
    async fn config_key(&self, cookie: &str, id: Id) -> String {
        let (status, body) = self.config(cookie, "attachment", &id.to_string(), "").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["config"]["document"]["key"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Reports `key` of a task file as outdated.
    async fn report_outdated(&self, cookie: &str, file: &str, key: &str) -> (StatusCode, Value) {
        send(
            &self.app,
            json_request(
                "POST",
                &format!("/api/v1/office/attachment/{file}/key"),
                cookie,
                &json!({"outdated_key": key}),
            ),
        )
        .await
    }
}

#[tokio::test]
async fn a_key_reported_as_outdated_is_renewed_once_for_everyone() {
    let fixture = Fixture::new().await;
    let id = fixture.attachment("report.docx", b"first bytes").await;
    let member = fixture.person("Member", Some("member")).await;
    let owner = fixture.owner_cookie.clone();
    let file = id.to_string();
    let first = fixture.config_key(&owner, id).await;

    // A key that the file does not have changes nothing.
    let (status, answer) = fixture
        .report_outdated(&owner, &file, &format!("{file}-0000"))
        .await;
    assert_eq!(
        (status, answer),
        (StatusCode::OK, json!({"renewed": false}))
    );
    assert_eq!(fixture.config_key(&owner, id).await, first);

    // Two people opened the file and the Document Server refused the key for both.
    let (status, answer) = fixture.report_outdated(&owner, &file, &first).await;
    assert_eq!((status, answer), (StatusCode::OK, json!({"renewed": true})));
    let second = fixture.config_key(&owner, id).await;
    assert_ne!(second, first);
    // Still a key of this file and these bytes: the callback accepts it.
    assert!(second.starts_with(&format!("{first}-")), "{second}");
    let (status, answer) = fixture.report_outdated(&member, &file, &first).await;
    assert_eq!(
        (status, answer),
        (StatusCode::OK, json!({"renewed": false}))
    );
    // Both get the same new key, so they edit together.
    assert_eq!(fixture.config_key(&member, id).await, second);
    assert_eq!(fixture.config_key(&owner, id).await, second);

    // A renewed key can be refused too (a second restore of the same backup).
    let (_, answer) = fixture.report_outdated(&member, &file, &second).await;
    assert_eq!(answer, json!({"renewed": true}));
    let third = fixture.config_key(&owner, id).await;
    assert!(third != second && third != first, "{third}");
    assert!(third.starts_with(&format!("{first}-")), "{third}");

    // A save gives the file other bytes: its key is that of the bytes again.
    OfficeFileVersions::new((*fixture.database).clone(), fixture.uploads.clone())
        .save(
            OfficeFileKind::Attachment,
            id,
            b"second bytes",
            Some(fixture.owner_id),
        )
        .await
        .unwrap();
    let saved = fixture.config_key(&owner, id).await;
    assert_eq!(saved, fixture.key_of_attachment(id).await);
    // And the key of the new bytes can be renewed: the row of the earlier bytes is replaced.
    let (_, answer) = fixture.report_outdated(&owner, &file, &saved).await;
    assert_eq!(answer, json!({"renewed": true}));
    assert!(
        fixture
            .config_key(&member, id)
            .await
            .starts_with(&format!("{saved}-"))
    );

    // The key goes with the file.
    sqlx::query("DELETE FROM attachment_references WHERE id = $1")
        .bind(&file)
        .execute(fixture.database.pool())
        .await
        .unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM office_document_keys")
        .fetch_one(fixture.database.pool())
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn only_people_who_may_change_a_file_renew_its_key() {
    let fixture = Fixture::new().await;
    let author = fixture.person("Author", Some("member")).await;
    let reader = fixture.person("Reader", Some("member")).await;
    let outsider = fixture.person("Outsider", None).await;
    let (status, created) = send(
        &fixture.app,
        multipart_request(
            &format!(
                "/api/v1/workspaces/{}/tasks/{}/comments/attachments",
                fixture.workspace_id, fixture.task_id
            ),
            &author,
            "notes.docx",
            b"comment bytes",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let file = created["attachments"][0]["id"].as_str().unwrap().to_owned();
    let (_, body) = fixture.config(&reader, "attachment", &file, "").await;
    let key = body["config"]["document"]["key"]
        .as_str()
        .unwrap()
        .to_owned();

    // A key that the file does not have: the reader only has to ask for the config again.
    let (status, answer) = fixture
        .report_outdated(&reader, &file, &format!("{file}-0000"))
        .await;
    assert_eq!(
        (status, answer),
        (StatusCode::OK, json!({"renewed": false}))
    );
    // A reader of the comment's file may not: a new key would split the people who edit.
    let (status, answer) = fixture.report_outdated(&reader, &file, &key).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert_eq!(answer["code"], "office_file_read_only");
    // Who cannot read the file learns nothing, and nobody does without a session.
    let (status, answer) = fixture.report_outdated(&outsider, &file, &key).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    let (status, _) = fixture.report_outdated("", &file, &key).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (_, body) = fixture.config(&author, "attachment", &file, "").await;
    assert_eq!(body["config"]["document"]["key"], key.as_str());

    let (status, answer) = fixture.report_outdated(&author, &file, &key).await;
    assert_eq!((status, answer), (StatusCode::OK, json!({"renewed": true})));
    let (_, body) = fixture.config(&reader, "attachment", &file, "").await;
    assert_ne!(body["config"]["document"]["key"], key.as_str());
}

#[tokio::test]
async fn a_file_of_a_comment_is_read_only_for_everyone_but_who_may_edit_the_comment() {
    let fixture = Fixture::new().await;
    let author = fixture.person("Author", Some("member")).await;
    let reader = fixture.person("Reader", Some("member")).await;
    let (status, created) = send(
        &fixture.app,
        multipart_request(
            &format!(
                "/api/v1/workspaces/{}/tasks/{}/comments/attachments",
                fixture.workspace_id, fixture.task_id
            ),
            &author,
            "notes.docx",
            b"comment bytes",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let file = created["attachments"][0]["id"].as_str().unwrap().to_owned();

    let (status, body) = fixture.config(&author, "attachment", &file, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["config"]["editorConfig"]["mode"], "edit");

    let (status, body) = fixture.config(&reader, "attachment", &file, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["config"]["editorConfig"]["mode"], "view");
    assert_eq!(body["config"]["document"]["permissions"]["edit"], false);
    assert_eq!(body["config"]["document"]["permissions"]["comment"], false);
    assert_eq!(body["config"]["document"]["permissions"]["download"], true);
}

#[tokio::test]
async fn the_format_decides_between_editing_viewing_and_nothing() {
    let fixture = Fixture::new().await;
    let cookie = &fixture.owner_cookie;

    let sheet = fixture.page_file("budget.xlsx", b"sheet").await;
    let (status, body) = fixture
        .config(cookie, "page_file", &sheet.to_string(), "")
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["config"]["documentType"], "cell");
    assert_eq!(body["config"]["editorConfig"]["mode"], "edit");
    assert_eq!(
        body["config"]["editorConfig"]["callbackUrl"],
        format!("http://orbit:8080/api/v1/office/page_file/{sheet}/callback")
    );

    // The Document Server would save these in another format: they open read-only.
    for (name, document_type) in [
        ("old.xls", "cell"),
        ("scan.pdf", "pdf"),
        ("deck.odp", "slide"),
    ] {
        let id = fixture.attachment(name, name.as_bytes()).await;
        let (status, body) = fixture
            .config(cookie, "attachment", &id.to_string(), "")
            .await;
        assert_eq!(status, StatusCode::OK, "{name}: {body}");
        assert_eq!(body["config"]["documentType"], document_type, "{name}");
        assert_eq!(body["config"]["editorConfig"]["mode"], "view", "{name}");
        assert_eq!(
            body["config"]["document"]["permissions"]["edit"], false,
            "{name}"
        );
    }

    let text = fixture.attachment("notes.txt", b"plain text").await;
    let (status, body) = fixture
        .config(cookie, "attachment", &text.to_string(), "")
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "office_file_unsupported");
}

#[tokio::test]
async fn what_the_browser_asks_for_cannot_lead_out_of_orbit() {
    let fixture = Fixture::new().await;
    let id = fixture.attachment("report.docx", b"bytes").await;
    for query in [
        "?back=https://evil.example/",
        "?back=//evil.example/",
        "?back=%2F%5Cevil.example",
        "?theme=%22onload%3D&lang=%3Cscript%3E",
    ] {
        let (status, body) = fixture
            .config(&fixture.owner_cookie, "attachment", &id.to_string(), query)
            .await;
        assert_eq!(status, StatusCode::OK, "{query}: {body}");
        assert_eq!(
            body["config"]["editorConfig"].get("customization"),
            None,
            "{query}"
        );
        assert_eq!(body["config"]["editorConfig"].get("lang"), None, "{query}");
    }
}

#[tokio::test]
async fn only_people_who_can_read_the_file_get_a_config() {
    let fixture = Fixture::new().await;
    let id = fixture
        .attachment("report.docx", b"bytes")
        .await
        .to_string();
    let page_file = fixture.page_file("budget.xlsx", b"sheet").await.to_string();
    let outsider = fixture.person("Outsider", None).await;
    let guest = fixture.person("Guest", Some("guest")).await;

    let (status, body) = fixture.config("", "attachment", &id, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");

    for cookie in [&outsider, &guest] {
        for (kind, file) in [("attachment", &id), ("page_file", &page_file)] {
            let (status, body) = fixture.config(cookie, kind, file, "").await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{kind}: {body}");
            assert_eq!(body["code"], "office_file_not_found");
        }
    }

    let cookie = &fixture.owner_cookie;
    for (kind, file) in [
        ("attachment", Id::new_v7().to_string()),
        // The id of the other kind.
        ("page_file", id.clone()),
        ("chat_file", id.clone()),
        ("attachment", "not-an-id".to_owned()),
    ] {
        let (status, body) = fixture.config(cookie, kind, &file, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{kind} {file}: {body}");
        assert_eq!(body["code"], "office_file_not_found");
    }
}

#[tokio::test]
async fn there_is_no_config_while_office_editing_is_off() {
    let fixture = Fixture::without_document_server().await;
    let id = fixture
        .attachment("report.docx", b"bytes")
        .await
        .to_string();
    let (status, body) = fixture
        .config(&fixture.owner_cookie, "attachment", &id, "")
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "office_not_configured");

    fixture.save_settings(false).await;
    let (status, body) = fixture
        .config(&fixture.owner_cookie, "attachment", &id, "")
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "office_not_configured");

    // Who cannot read the file does not learn that either.
    let outsider = fixture.person("Outsider", None).await;
    let (status, _) = fixture.config(&outsider, "attachment", &id, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    fixture.save_settings(true).await;
    let (status, _) = fixture
        .config(&fixture.owner_cookie, "attachment", &id, "")
        .await;
    assert_eq!(status, StatusCode::OK);
}

/// Asks for the address as the Document Server does: no session, and a signed header when `authorization`.
async fn download(app: &Router, url: &str, authorization: Option<&str>) -> (StatusCode, Vec<u8>) {
    let path = url.strip_prefix("http://orbit:8080").expect(url);
    let mut request = Request::builder().uri(path);
    if let Some(authorization) = authorization {
        request = request.header(header::AUTHORIZATION, authorization);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, bytes.to_vec())
}

#[tokio::test]
async fn the_document_server_downloads_the_version_its_address_names() {
    let fixture = Fixture::new().await;
    let id = fixture.attachment("report.docx", b"first bytes").await;
    let file = id.to_string();
    let (_, body) = fixture
        .config(&fixture.owner_cookie, "attachment", &file, "")
        .await;
    let url = body["config"]["document"]["url"]
        .as_str()
        .unwrap()
        .to_owned();

    // No session: the token of the address is enough.
    let (status, bytes) = download(&fixture.app, &url, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"first bytes");

    // A signature, when there is one, must be the Document Server's.
    let signed = jwt::sign(&json!({"payload": {"url": url}}), SECRET);
    let (status, bytes) = download(&fixture.app, &url, Some(&format!("Bearer {signed}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"first bytes");
    let forged = jwt::sign(&json!({"payload": {"url": url}}), "another");
    for authorization in [format!("Bearer {forged}"), "Bearer".to_owned(), signed] {
        let (status, _) = download(&fixture.app, &url, Some(&authorization)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{authorization}");
    }

    // The token opens no other file, no other kind, and nothing without it.
    let other = fixture.attachment("other.docx", b"other bytes").await;
    let (base, token) = url.split_once("?t=").unwrap();
    for address in [
        base.to_owned(),
        format!("{base}?t="),
        format!("{base}?t={token}A"),
        format!("http://orbit:8080/api/v1/office/attachment/{other}/download?t={token}"),
        format!("http://orbit:8080/api/v1/office/page_file/{id}/download?t={token}"),
        format!("http://orbit:8080/api/v1/office/chat_file/{id}/download?t={token}"),
    ] {
        let (status, _) = download(&fixture.app, &address, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{address}");
    }

    // After a save the file holds other bytes: the old address serves neither version.
    OfficeFileVersions::new((*fixture.database).clone(), fixture.uploads.clone())
        .save(
            OfficeFileKind::Attachment,
            id,
            b"second bytes",
            Some(fixture.owner_id),
        )
        .await
        .unwrap();
    let (status, _) = download(&fixture.app, &url, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, body) = fixture
        .config(&fixture.owner_cookie, "attachment", &file, "")
        .await;
    let fresh = body["config"]["document"]["url"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, bytes) = download(&fixture.app, &fresh, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"second bytes");

    // Turning office editing off closes the address.
    fixture.save_settings(false).await;
    let (status, _) = download(&fixture.app, &fresh, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_document_server_downloads_a_page_file() {
    let fixture = Fixture::new().await;
    let id = fixture.page_file("budget.xlsx", b"sheet bytes").await;
    let (_, body) = fixture
        .config(&fixture.owner_cookie, "page_file", &id.to_string(), "")
        .await;
    let url = body["config"]["document"]["url"].as_str().unwrap();
    let (status, bytes) = download(&fixture.app, url, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"sheet bytes");
}

/// A Document Server that hands over `bytes` at `/cache/out.docx`, and redirects at `/cache/moved`.
/// Returns its `http://127.0.0.1:{port}` address and how often the file was asked for.
async fn document_server(bytes: &'static [u8]) -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route(
            "/cache/out.docx",
            get(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async move { bytes }
            }),
        )
        .route(
            "/cache/moved",
            get(|| async { axum::response::Redirect::temporary("/cache/out.docx") }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), hits)
}

/// Sends a callback as the Document Server does. `token`: where the signed copy goes.
async fn callback(
    app: &Router,
    kind: &str,
    file: Id,
    body: &Value,
    token: Token,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/office/{kind}/{file}/callback"))
        .header(header::CONTENT_TYPE, "application/json");
    let mut sent = body.clone();
    match token {
        Token::Body(secret) => sent["token"] = json!(jwt::sign(body, secret)),
        Token::Header(secret) => {
            let signed = jwt::sign(&json!({"payload": body}), secret);
            request = request.header(header::AUTHORIZATION, format!("Bearer {signed}"));
        }
        Token::None => {}
    }
    send(app, request.body(Body::from(sent.to_string())).unwrap()).await
}

enum Token {
    Body(&'static str),
    Header(&'static str),
    None,
}

impl Fixture {
    async fn bytes_of_attachment(&self, id: Id) -> Vec<u8> {
        let (_, body) = self
            .config(&self.owner_cookie, "attachment", &id.to_string(), "")
            .await;
        let url = body["config"]["document"]["url"].as_str().unwrap();
        download(&self.app, url, None).await.1
    }

    async fn key_of_attachment(&self, id: Id) -> String {
        let (_, sha256) = self.blob_of_attachment(id).await;
        format!("{id}-{}", &sha256[..16])
    }

    async fn versions(&self, id: Id) -> Vec<Option<String>> {
        sqlx::query_scalar(
            "SELECT saved_by FROM office_file_versions WHERE file_id = $1 ORDER BY created_at, id",
        )
        .bind(id.to_string())
        .fetch_all(self.database.pool())
        .await
        .unwrap()
    }

    async fn audit_actions(&self) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT action FROM audit_events WHERE action LIKE 'office.%' ORDER BY occurred_at, id",
        )
        .fetch_all(self.database.pool())
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn a_save_from_the_document_server_becomes_the_file_and_keeps_the_earlier_bytes() {
    let fixture = Fixture::without_document_server().await;
    let (server, hits) = document_server(b"edited bytes").await;
    fixture.save_settings_with(true, Some(&server)).await;
    let id = fixture.attachment("report.docx", b"first bytes").await;
    let key = fixture.key_of_attachment(id).await;
    let saved = json!({
        "key": key,
        "status": 2,
        "url": format!("{server}/cache/out.docx"),
        "filetype": "docx",
        "users": [fixture.owner_id.to_string()],
    });

    // While people edit, and when they close without changes, there is nothing to store.
    for status in [1, 4] {
        let body = json!({"key": key, "status": status, "users": [fixture.owner_id.to_string()]});
        let (status, answer) =
            callback(&fixture.app, "attachment", id, &body, Token::Body(SECRET)).await;
        assert_eq!((status, answer), (StatusCode::OK, json!({"error": 0})));
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.bytes_of_attachment(id).await, b"first bytes");

    let (status, answer) =
        callback(&fixture.app, "attachment", id, &saved, Token::Body(SECRET)).await;
    assert_eq!((status, answer), (StatusCode::OK, json!({"error": 0})));
    assert_eq!(fixture.bytes_of_attachment(id).await, b"edited bytes");
    assert_eq!(
        fixture.versions(id).await,
        [Some(fixture.owner_id.to_string())]
    );
    assert_ne!(fixture.key_of_attachment(id).await, key);
    assert_eq!(fixture.audit_actions().await, ["office.file_saved"]);

    // The Document Server repeats a callback it got no answer to: no second version.
    let (status, answer) = callback(
        &fixture.app,
        "attachment",
        id,
        &saved,
        Token::Header(SECRET),
    )
    .await;
    assert_eq!((status, answer), (StatusCode::OK, json!({"error": 0})));
    assert_eq!(fixture.versions(id).await.len(), 1);
    assert_eq!(fixture.audit_actions().await.len(), 1);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_save_with_an_old_key_is_stored_and_an_unknown_editor_is_left_out() {
    let fixture = Fixture::without_document_server().await;
    let (server, _) = document_server(b"edited bytes").await;
    fixture.save_settings_with(true, Some(&server)).await;
    let id = fixture.attachment("report.docx", b"first bytes").await;
    let old_key = fixture.key_of_attachment(id).await;
    // The file gets other bytes while it is open in the editor.
    OfficeFileVersions::new((*fixture.database).clone(), fixture.uploads.clone())
        .save(
            OfficeFileKind::Attachment,
            id,
            b"second bytes",
            Some(fixture.owner_id),
        )
        .await
        .unwrap();

    let body = json!({
        "key": old_key,
        "status": 6,
        "url": format!("{server}/cache/out.docx"),
        "users": [Id::new_v7().to_string()],
    });
    let (status, answer) =
        callback(&fixture.app, "attachment", id, &body, Token::Header(SECRET)).await;
    assert_eq!((status, answer), (StatusCode::OK, json!({"error": 0})));
    assert_eq!(fixture.bytes_of_attachment(id).await, b"edited bytes");
    // Both earlier contents are versions; the second save has no known author.
    assert_eq!(
        fixture.versions(id).await,
        [Some(fixture.owner_id.to_string()), None]
    );
}

#[tokio::test]
async fn a_page_file_is_saved_and_a_failed_save_is_recorded() {
    let fixture = Fixture::without_document_server().await;
    let (server, hits) = document_server(b"edited sheet").await;
    fixture.save_settings_with(true, Some(&server)).await;
    let id = fixture.page_file("budget.xlsx", b"sheet").await;
    let (_, body) = fixture
        .config(&fixture.owner_cookie, "page_file", &id.to_string(), "")
        .await;
    let key = body["config"]["document"]["key"]
        .as_str()
        .unwrap()
        .to_owned();

    let failed = json!({"key": key, "status": 3, "users": [fixture.owner_id.to_string()]});
    let (status, answer) =
        callback(&fixture.app, "page_file", id, &failed, Token::Body(SECRET)).await;
    assert_eq!((status, answer), (StatusCode::OK, json!({"error": 0})));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.audit_actions().await, ["office.save_failed"]);

    let saved = json!({
        "key": key,
        "status": 2,
        "url": format!("{server}/cache/out.docx"),
        "filetype": "xlsx",
        "users": [fixture.owner_id.to_string()],
    });
    let (status, answer) =
        callback(&fixture.app, "page_file", id, &saved, Token::Body(SECRET)).await;
    assert_eq!((status, answer), (StatusCode::OK, json!({"error": 0})));
    let (_, body) = fixture
        .config(&fixture.owner_cookie, "page_file", &id.to_string(), "")
        .await;
    let url = body["config"]["document"]["url"].as_str().unwrap();
    assert_eq!(download(&fixture.app, url, None).await.1, b"edited sheet");
    assert_eq!(fixture.versions(id).await.len(), 1);
}

#[tokio::test]
async fn a_callback_that_is_not_the_document_servers_stores_nothing() {
    let fixture = Fixture::without_document_server().await;
    let (server, hits) = document_server(b"edited bytes").await;
    fixture.save_settings_with(true, Some(&server)).await;
    let id = fixture.attachment("report.docx", b"first bytes").await;
    let other = fixture.attachment("other.docx", b"other bytes").await;
    let legacy = fixture.attachment("old.xls", b"legacy bytes").await;
    let key = fixture.key_of_attachment(id).await;
    let good = format!("{server}/cache/out.docx");
    let save = |key: &str, url: &str| json!({"key": key, "status": 2, "url": url, "users": [fixture.owner_id.to_string()]});
    let app = &fixture.app;

    // Without a token, with a token of another secret, and with a body that is not JSON.
    let body = save(&key, &good);
    for token in [
        Token::None,
        Token::Body("another"),
        Token::Header("another"),
    ] {
        let (status, answer) = callback(app, "attachment", id, &body, token).await;
        assert_eq!(
            (status, answer),
            (StatusCode::FORBIDDEN, json!({"error": 1}))
        );
    }
    // The signed token decides, not the body around it.
    let mut tampered = json!({"key": key, "status": 1});
    tampered["token"] = json!(jwt::sign(&tampered, SECRET));
    tampered["status"] = json!(2);
    tampered["url"] = json!(good);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/office/attachment/{id}/callback"))
        .body(Body::from(tampered.to_string()))
        .unwrap();
    assert_eq!(
        send(app, request).await,
        (StatusCode::OK, json!({"error": 0}))
    );

    // An address that is not on the Document Server is never asked: no request leaves Orbit.
    let (elsewhere, elsewhere_hits) = document_server(b"planted bytes").await;
    for url in [
        format!("{elsewhere}/cache/out.docx"),
        "http://169.254.169.254/latest/meta-data/".to_owned(),
        "file:///etc/passwd".to_owned(),
    ] {
        let (status, answer) = callback(
            app,
            "attachment",
            id,
            &save(&key, &url),
            Token::Body(SECRET),
        )
        .await;
        assert_eq!(
            (status, answer),
            (StatusCode::FORBIDDEN, json!({"error": 1})),
            "{url}"
        );
    }
    assert_eq!(elsewhere_hits.load(Ordering::SeqCst), 0);
    // A redirect is not followed.
    let moved = save(&key, &format!("{server}/cache/moved"));
    let (status, _) = callback(app, "attachment", id, &moved, Token::Body(SECRET)).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);

    // The key of another file, a read-only format, another saved format, and no address.
    let other_key = fixture.key_of_attachment(other).await;
    let (status, _) = callback(
        app,
        "attachment",
        id,
        &save(&other_key, &good),
        Token::Body(SECRET),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let legacy_key = fixture.key_of_attachment(legacy).await;
    let (status, _) = callback(
        app,
        "attachment",
        legacy,
        &save(&legacy_key, &good),
        Token::Body(SECRET),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let mut converted = save(&key, &good);
    converted["filetype"] = json!("pdf");
    let (status, _) = callback(app, "attachment", id, &converted, Token::Body(SECRET)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = callback(
        app,
        "attachment",
        id,
        &json!({"key": key, "status": 2}),
        Token::Body(SECRET),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A file that does not exist, another kind, and office editing off.
    let gone = Id::new_v7();
    let (status, _) = callback(
        app,
        "attachment",
        gone,
        &save(&format!("{gone}-0"), &good),
        Token::Body(SECRET),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = callback(app, "chat_file", id, &body, Token::Body(SECRET)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    assert_eq!(hits.load(Ordering::SeqCst), 0);
    for (file, bytes) in [(id, &b"first bytes"[..]), (legacy, b"legacy bytes")] {
        assert_eq!(fixture.versions(file).await.len(), 0);
        let (_, blob) = fixture.blob_of_attachment(file).await;
        assert_eq!(blob, hex_sha256(bytes));
    }
    assert!(fixture.audit_actions().await.is_empty());

    fixture.save_settings_with(false, Some(&server)).await;
    let (status, answer) = callback(app, "attachment", id, &body, Token::Body(SECRET)).await;
    assert_eq!(
        (status, answer),
        (StatusCode::NOT_FOUND, json!({"error": 1}))
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn everyone_signed_in_learns_whether_office_editing_is_on() {
    let fixture = Fixture::without_document_server().await;
    let status = |cookie: String| {
        let app = fixture.app.clone();
        async move {
            let mut request = Request::builder().uri("/api/v1/office/status");
            if !cookie.is_empty() {
                request = request.header(header::COOKIE, cookie);
            }
            send(&app, request.body(Body::empty()).unwrap()).await
        }
    };
    let guest = fixture.person("Guest", Some("guest")).await;

    assert_eq!(status(String::new()).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        status(guest.clone()).await,
        (StatusCode::OK, json!({"enabled": false}))
    );
    fixture.save_settings(false).await;
    assert_eq!(status(guest.clone()).await.1, json!({"enabled": false}));
    fixture.save_settings(true).await;
    // Nothing about the Document Server itself.
    assert_eq!(status(guest).await.1, json!({"enabled": true}));
    assert_eq!(
        status(fixture.owner_cookie.clone()).await.1,
        json!({"enabled": true})
    );
}
