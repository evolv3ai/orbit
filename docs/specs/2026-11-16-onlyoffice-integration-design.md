# ONLYOFFICE integration design

Status: draft, 2026-11-16. Closes the approach spike (OO-1 in the LegacyWorks Orbit workspace).
Tracked in the Orbit project **OnlyOffice Integration** (`OO`). This is a fork feature
(`evolv3ai/orbit`); upstream `coollabsio/orbit` has no plugin system, so section 9 covers how to
keep it mergeable.

## 1. Decision

Orbit embeds the **ONLYOFFICE Docs** editor through the **Docs API**, not WOPI, and not DocSpace.

| Option | Why not, or why |
|---|---|
| **Docs API** (chosen) | Orbit keeps the files in its own blob store, so they stay under Orbit permissions, backups, restore and export. It is the richest surface: JWT-protected server, templates, editor plugins, mentions, `downloadAs`, document comparison, mail merge, content-control and filter permissions. |
| WOPI | No toolbar customization, templates, plugins, mentions or fine-grained permissions (see ONLYOFFICE's [API vs WOPI](https://api.onlyoffice.com/docs/docs-api/using-wopi/api-vs-wopi.md)). Its one gain, a single host for Collabora and Microsoft 365 too, is not a goal. |
| DocSpace embedded (Embed SDK) | A second app with its own users, rooms and storage. Files leave Orbit's search, backups and permissions. |
| DocSpace plugin (Plugins SDK) | Extends DocSpace, not Orbit. Can be added later if DocSpace is ever run alongside. |

**Scope split.** Orbit Docs (BlockNote + Yjs) stays the home for roadmaps, decision records and
Markdown notes. ONLYOFFICE is only for files that must stay in Office formats: `.docx`, `.xlsx`,
`.pptx`, and `.pdf` (view, and fill forms).

**Edition.** ONLYOFFICE Docs Community Edition (AGPL-3.0, Docker image `onlyoffice/documentserver`).
It caps simultaneous editing connections (20 at the time of writing; confirm on the current
licence page). That fits the team; revisit if usage grows.

## 2. How it works

```
Browser ──(1) GET /api/v1/office/{kind}/{file_id}/config ──▶ Orbit
        ◀── signed editor config (JWT) ─────────────────────
Browser ──(2) loads {DS}/web-apps/apps/api/documents/api.js, mounts DocsAPI.DocEditor
Doc Server ──(3) GET document.url (office token) ──▶ Orbit  → file bytes
   … users edit; co-editing is between browsers and the Doc Server …
Doc Server ──(4) POST callbackUrl {status, url, key, users} (JWT) ──▶ Orbit
Orbit ──(5) GET {url} from the Doc Server ─▶ new blob ─▶ new version of the file
```

Network paths that must be open:

- Browser → Document Server (public URL, WebSocket upgrade allowed).
- Document Server → Orbit (download and callback). Can be an internal Coolify network URL.
- Orbit → Document Server (fetch the saved file, health check). Can be internal.

## 3. Files that open in the editor

Orbit has no Files area; files exist as references to a deduplicated blob
(`attachment_blobs`, `blobs/{ws}/{sha256}-{size}`):

| Kind | Table | Phase 1 mode |
|---|---|---|
| `attachment` | `attachment_references` (tasks and comments) | Edit if the user can edit the task, otherwise view |
| `page_file` | `page_files` (Docs pages) | Edit if the user can edit the page, otherwise view |
| `chat_file` | `chat_message_files` | **View only.** A chat message is a record of what was sent; editing its file in place would rewrite history. "Save a copy to a task or page" is a later item. |

Permission checks reuse the existing ones in `attachment_routes.rs`, `page_file_routes.rs` and
`chat_routes/files.rs`. No new permission model.

## 4. Server

New module `apps/server/src/office/` with one router, `office_router(state)`, merged in
`apps/server/src/router.rs` like the other integrations. Everything is inert when the integration
is off.

### 4.1 Settings (OO-7)

Instance settings, following the S3 pattern in `crates/platform/src/config/instance.rs`
(`ORBIT_*` constant, field on `InstanceConfig`, read-only in Admin when set from the environment):

| Variable | Kind | Meaning |
|---|---|---|
| `ORBIT_ONLYOFFICE_ENABLED` | other | On/off. |
| `ORBIT_ONLYOFFICE_URL` | other | Public Document Server URL, loaded by browsers. |
| `ORBIT_ONLYOFFICE_INTERNAL_URL` | other | Optional. How Orbit reaches the Document Server. Default: the public URL. |
| `ORBIT_ONLYOFFICE_ORBIT_URL` | other | Optional. How the Document Server reaches Orbit. Default: Orbit's public URL. |
| `ORBIT_ONLYOFFICE_JWT_SECRET` | secret | Shared JWT secret, stored with `secret_box.rs` when saved in Admin. |

Admin page: "Office editing", with a **Test connection** button that calls the Document Server's
`/healthcheck` through the internal URL.

As built (OO-7, `apps/server/src/office/settings.rs` and `routes.rs`):

- The saved values are in their own one-row table, `office_settings`, not in `instance_settings`,
  so upstream changes to that table do not conflict.
- `ORBIT_ONLYOFFICE_URL` and `ORBIT_ONLYOFFICE_JWT_SECRET` are set together.
  `ORBIT_ONLYOFFICE_ENABLED` is `true` when the group is set without it. While the group is set,
  Admin is read-only (409 `set_by_environment`) and the saved row stays for later.
- Routes, for instance admins: `GET`, `PUT` and `DELETE /api/v1/admin/settings/office`, and
  `POST /api/v1/admin/settings/office/test`. The secret is never returned (`jwt_secret_set`).
- `OfficeSettingsRepository::get` returns the settings in use; `OfficeSettings::jwt_secret(app_key)`
  decrypts the secret and `server_url()` is the internal URL or the public one. OO-8 to OO-10 read
  the settings through these.
- `office::settings::reload` sets or clears the `CspOfficeOrigin` handle (section 5). It runs at
  startup, after each save, and on the other servers of a cluster (`Message::OfficeChanged`).
- The public URL must have a host name or an IPv4 address as its host: the policy cannot name an
  IPv6 literal.
- The web app has no "is office editing on" flag for members yet. OO-12 needs one.
- No mobile Admin screen: mobile is out of Phase 1 (section 6).

### 4.2 Office token (download authentication)

The Document Server has no Orbit session. Copy `socket_ticket.rs`: a stateless HMAC token with an
HKDF subkey (`"orbit office token v1"`) of the app key, binding **file kind, file id and blob id**,
with a short lifetime (5 minutes; the Document Server downloads once when the first editor opens a
given `document.key`). Binding the blob id means a token never serves a newer or older version.

As built (OO-8, `apps/server/src/office/token.rs`): `OfficeTokens::issue` and `verify`. The token
carries the blob id and the expiry; the HMAC also covers the kind and the file id, which the URL
names. `verify` returns the blob id, and OO-9 serves the file only while it still holds that blob.

### 4.3 Editor config (OO-8)

`GET /api/v1/office/{kind}/{file_id}/config` (session auth). Returns the config for
`DocsAPI.DocEditor`, signed as a JWT (HS256) with the shared secret, with the token in the
`token` field:

- `document.fileType`, `document.title`: from the reference row.
- `document.key`: `{file_id}-{sha256[0..16]}` of the current blob. It changes whenever the stored
  bytes change, and stays the same for everyone co-editing that version. (≤128 chars,
  `[0-9A-Za-z._=-]`.)
- `document.url`: `{orbit_url}/api/v1/office/{kind}/{file_id}/download?t={office token}`.
- `document.permissions`: `edit` per section 3; `download`, `print`, `copy` true; `review` and
  `comment` follow edit for now.
- `editorConfig.callbackUrl`: `{orbit_url}/api/v1/office/{kind}/{file_id}/callback`.
- `editorConfig.user`: Orbit user id and display name. `editorConfig.lang` and `uiTheme` from the
  user's settings.
- `editorConfig.customization.goback`: back to the task, page or conversation.

As built (OO-8, `apps/server/src/office/editor.rs`, `files.rs` and `jwt.rs`):

- The answer is `{ "document_server_url", "config" }`. The page loads `api.js` from the first and
  passes the second to `DocsAPI.DocEditor` as it is.
- Kinds: `attachment` and `page_file`. **`chat_file` is not built yet** (404): its access check is
  private to the chat repository.
- Access reuses `AttachmentRepository::authorize_task`, `require_comment_editor` and
  `PageFileRepository::authorize_upload`. A file of a comment is read-only for the people who may
  not edit the comment. Guests get 404, as on the download routes.
- Formats (`files::office_format`, by the extension of the file name): `docx`, `xlsx` and `pptx`
  open for editing. `doc`, `docm`, `dotx`, `odt`, `rtf`, `xls`, `xlsm`, `ods`, `ppt`, `ppsx`,
  `odp` and `pdf` open read-only, because the Document Server would save them in another format
  than the file has. Other files: 422 `office_file_unsupported`.
- `lang`, `theme` (`light` or `dark`) and `back` (a path of this Orbit) are query parameters,
  because the server does not store them. A value that does not pass its check is left out.
  `goback.url` is Orbit's public address plus `back`.
- The token has no `exp`. It is checked when the editor opens; what a late use of it could reach
  is bounded by the 5-minute office token inside `document.url`.
- Errors: 401, 404 `office_file_not_found` (no such file, or no read access), 409
  `office_not_configured` (no Document Server, or the switch is off).

### 4.4 Download (OO-9)

`GET /api/v1/office/{kind}/{file_id}/download?t=…` streams the blob through the existing
`BlobDownload` (local disk and S3). It checks the office token and, when present, the JWT in the
`Authorization` header. GET needs no change to the origin check.

As built (OO-9, `download_office_file` in `apps/server/src/office/editor.rs`):

- The office token alone allows the download. An `Authorization: Bearer` header, when the
  Document Server sends one, must be a token the shared secret signed; its payload is not read.
- 404 `office_file_not_found` for everything else: no or a wrong token, another file or kind, a
  file that holds other bytes than the token names (after a save or a replace), and office
  editing off.
- The route is not in the OpenAPI document: no client of Orbit calls it.
- Tested on local disk storage. S3 goes through the same `UploadService::download`; it was not
  run against a bucket.

### 4.5 Save callback (OO-10)

`POST /api/v1/office/{kind}/{file_id}/callback`. Machine-to-machine, so its path prefix is added to
`permits()` in `crates/platform/src/http/security.rs:84-101`, the same way the GitHub and Stripe
webhooks are. Authentication is the JWT in the body's `token` field (fallback: `Authorization`
header), verified with the shared secret. The token payload, not the unsigned body, is the source
of truth.

| `status` | Action |
|---|---|
| 1 (editing), 4 (closed, no changes) | Nothing to store. |
| 2 (ready to save), 6 (force save) | Fetch `url` and store a new version (4.6). |
| 3, 7 (save errors) | Log, record on the file, notify the last editor. |

- Reply `{"error":0}` on success. Anything else makes the editor show an error and retry.
- **SSRF guard:** fetch `url` only when its origin is the configured Document Server (internal or
  public URL). Refuse anything else.
- **Idempotent:** the Document Server can repeat a callback. Saving the same bytes twice dedupes on
  `sha256` and creates no second version.
- **Stale key:** if the callback's `key` is not the file's current key (the file was replaced
  while open), still store the bytes as a new version, and log the conflict. Nothing is lost.
- The version is attributed to `users[0]`, the last user who sent changes.

As built (OO-10, `apps/server/src/office/callback.rs`):

- Answers: `{"error":0}`, or `{"error":1}` with 400 (not a callback of this file), 403 (no valid
  token, or a `url` that is not on the Document Server), 404 (no such file, or office editing
  off), 413 (over the file size limit), 502 (the saved file could not be fetched) or 500. The
  reason goes to the server log only.
- The `key` must be a key of the file of the address (`{file_id}-…`). The file must be one the
  editor opens for editing (`docx`, `xlsx`, `pptx`), and a `filetype` in the callback must match.
- The saved file is fetched from `url` as it is given, when its origin is the public or the
  internal Document Server URL. No redirect is followed. Orbit does not rewrite a public address
  to the internal one: if the Document Server names an address that Orbit cannot reach, saves
  fail with 502.
- `users[0]` is the author only when it is an account of this Orbit; otherwise the version has
  no author.
- A save writes the audit event `office.file_saved` to the workspace; status 3 and 7 write
  `office.save_failed` and a warning to the log. **Not built:** a mark on the file and a
  notification to the last editor. The editor itself shows the people in the document that the
  save failed.
- **Force save (status 6) is stored like a save, so the file's key changes while people still
  edit.** Someone who opens the file then gets a new editing session, apart from the people
  already in it, and the later save of the old session replaces the newer bytes (they are kept
  as a version). Orbit does not turn force save on, so this happens only when it is turned on in
  the Document Server. A fix needs a stored key per editing session.
- The web app is not told that a file changed: an open task or page shows the new size after a
  reload.

### 4.6 Versions (new)

Orbit references point at one blob and have no history. Saving in place would lose the previous
bytes once the blob is reclaimed. New table (twin SQLite and Postgres migrations via
`just new-migration`):

```sql
CREATE TABLE office_file_versions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_kind TEXT NOT NULL CHECK (file_kind IN ('attachment', 'page_file')),
    file_id TEXT NOT NULL,
    blob_id TEXT NOT NULL REFERENCES attachment_blobs(id) ON DELETE RESTRICT,
    saved_by TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL
);
```

On save: upload the new bytes through `UploadService` (dedupe, quarantine rules), insert the
**previous** blob as a version row, then point the reference row at the new blob and update its
size. One transaction.

The blob reference count and reconciliation in `crates/platform/src/files/upload.rs` (the
`BLOB_REFERENCE_COUNT` query at line 26, the table list at line 35, and the `NOT EXISTS` checks
near line 714) must count `office_file_versions`, or reconciliation reclaims old versions. Version
rows go with their file (trash purge, owner delete, workspace delete) and are covered by backup
and export.

As built (OO-17, `apps/server/src/office/versions.rs`, `OfficeFileVersions::save`):

- `saved_by` and `created_at` of a row are the save that **replaced** that blob: who sent the last
  changes, and when. So the author of the current bytes is on the newest row.
- `file_id` has no foreign key (it is an attachment or a page file, by `file_kind`). Two delete
  triggers, on `attachment_references` and `page_files`, remove the versions with their file on
  every path (direct delete, cascade, trash purge, workspace purge). No delete code changed.
- Upstream made `blob_id` immutable on both tables (triggers in migrations 0009 and 0025). The
  versions migration relaxes them: `blob_id` may change to a blob of the same workspace. Workspace,
  task, comment and page stay immutable. **If upstream recreates those triggers, saves break**;
  `apps/server/tests/office_versions.rs` catches it.
- A full backup holds the rows (it copies the database). The workspace export carries current
  files only, as it does for page history.
- An admin "delete file" (`UploadService::delete_blob`) is unchanged: it can remove the bytes of a
  blob that is also an older version.

Phase 1 keeps the last 20 versions per file (`MAX_VERSIONS_PER_FILE`). A version list and restore UI is Phase 2 (via the
editor's `onRequestHistory` events).

## 5. Web (OO-4, OO-11, OO-12)

- Route `/office/:kind/:fileId` in `apps/web/src/app/App.tsx`: full height; fetches the config,
  loads `api.js` from `ORBIT_ONLYOFFICE_URL`, mounts the editor, follows Orbit's theme and
  language, and shows a clear error when the Document Server cannot be reached or the integration
  is off.
- "Open in editor" on office files in `packages/editor/src/FilePreview.tsx`, task attachments and
  chat files, shown only when the integration is on. Read-only access opens view mode.
- CSP: add the Document Server origin to `frame-src` (and `script-src` for `api.js`) in
  `crates/platform/src/http/security.rs:296`, **only when the integration is on**. The policy is
  built per request, so derive it from the setting. As built (OO-18): the HTTP layer takes a
  shared `orbit_platform::CspOfficeOrigin` handle (`HttpPlatformLayer::with_csp_office_origin`).
  OO-7 creates one, gives it to the layer in `router.rs`, and calls `set("https://host[:port]")`
  or `clear()` when the settings load or change. Until then the policy is unchanged.
- Run `just api` to regenerate `packages/api` after the server routes land.

As built (OO-11, `apps/web/src/features/office/`):

- `/office/:kind/:fileId?back=/path` is inside the sign-in gate and outside the app shell, so the
  editor has the whole window. `back` is the page the file was opened from; "Back to Orbit" and
  the editor's "Open file location" lead there, or home.
- The page asks for the config with the current theme, loads `api.js` (`documentServer.ts`, once
  per address), and mounts `DocsAPI.DocEditor` with the config as the server gave it, plus
  `width`, `height` and `type: "desktop"`. Leaving the page destroys the editor.
- It asks for a new config on every visit and after "Try again": a config is for one version of
  the file and its download address lasts five minutes.
- Messages: office editing off, not an office file, file not found (no retry); the Document
  Server could not be reached, and other failures (with "Try again").
- No language is sent: the web app has no language setting, so the editor uses its default.
- Not yet run in a browser against a Document Server.

As built (OO-12):

- `GET /api/v1/office/status` answers `{ "enabled" }` to everyone who is signed in. The web app
  asks once per five minutes (`useOfficeEnabled`).
- "Open in editor" opens `/office/...` in a new tab. It shows on files the editor opens (the
  formats of section 4.3, by the extension of the name) in two places:
  - file blocks in the editors of Docs pages, task descriptions and comments
    (`packages/editor/src/FilePreview.tsx`). The web app provides `OfficeLinkContext`
    (`packages/editor/src/officeLink.ts`) around the app shell; without a provider the block is
    as upstream has it.
  - the file cards under a task and under a comment (`Attachments`, prop `editorHref`).
- The file and its kind come from the download address of the file
  (`features/office/officeFiles.ts`).
- **Not built: chat files** (the server has no `chat_file` kind, see 4.3) and mail attachments.
- The mobile app does not provide the context, so its file blocks are unchanged (section 6).
- The link in file blocks was tested through its helper functions and the file cards; the block
  itself was not rendered in a test or a browser.

## 6. Mobile

Phase 1 (fork): mobile keeps downloading the file (OO-4). **Upstreaming requires a mobile port**
(`AGENTS.md`); the port would open the same `/office/...` page in a WebView with
`type: "mobile"`.

## 7. Deployment

- **Repo:** an optional `documentserver` service in `deploy/compose.yaml` and a route in
  `deploy/Caddyfile` (OO-6).
- **Live (`orbit.appster.net`, Coolify):** the live server runs the stock `ghcr.io/coollabsio/orbit:0.3.0`
  image, which **refuses to start on an unknown `ORBIT_*` variable**. Turning this on means
  building and running the fork's own image. The Document Server runs as its own Coolify service
  (for example `office.appster.net`) with `JWT_ENABLED=true` and `JWT_SECRET` from Infisical.
  The image switch and server changes are the owner's call.

## 8. Tests (OO-14)

- Unit: office token (expiry, wrong file, wrong blob), JWT verify (bad secret, missing token),
  callback status table, SSRF guard, idempotent saves, version reference counting.
- Both databases (SQLite and Postgres), per `AGENTS.md`.
- End to end against a real `onlyoffice/documentserver` container: open, edit, close, check the
  new version; two editors at once; read-only user; bad JWT refused; reconciliation keeps old
  versions.

## 9. Keeping the fork mergeable

- Everything new lives in `apps/server/src/office/`, its own migrations, `apps/web/src/features/office/`
  and one Admin page. Changes to shared files are limited to: `router.rs` (one merge),
  `security.rs` (allowlist entry, CSP source), `upload.rs` (reference count), `instance.rs`
  (settings), `App.tsx` and the file-preview action.
- Work on a `feat/onlyoffice` branch; rebase on `upstream/main` before each release.
- If the integration proves itself, offer it upstream, which needs the mobile port.

## 10. Later phases

| Phase | Item |
|---|---|
| 2 | Create a document from a template inside a task or page. |
| 2 | A per-project "Files" view listing office files across tasks and pages. |
| 2 | Version history and restore in the editor (`onRequestHistory`, `onRequestRestore`). |
| 2 | Save a chat file as a copy on a task or page. |
| 3 | @mentions of Orbit members in documents (`onRequestUsers`), notifying through Orbit. |
| 3 | Document Builder: generate `.docx`/`.xlsx` from Orbit data (for example a roadmap report). |
| 3 | Conversion API: import a `.docx` into Orbit Docs as Markdown, and export a page to `.docx`. |
| 3 | MCP tools: list, create and read office files for agents. |

## 11. Open questions

1. Edition: is Community Edition's connection cap acceptable, or is a paid edition planned?
2. Chat files: view only in Phase 1 (as proposed), or editable?
3. Version retention: 20 per file, or a workspace setting?
4. Document Server hostname on Coolify: its own subdomain (proposed) or a path under Orbit's?
