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

### 4.2 Office token (download authentication)

The Document Server has no Orbit session. Copy `socket_ticket.rs`: a stateless HMAC token with an
HKDF subkey (`"orbit office token v1"`) of the app key, binding **file kind, file id and blob id**,
with a short lifetime (5 minutes; the Document Server downloads once when the first editor opens a
given `document.key`). Binding the blob id means a token never serves a newer or older version.

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

### 4.4 Download (OO-9)

`GET /api/v1/office/{kind}/{file_id}/download?t=…` streams the blob through the existing
`BlobDownload` (local disk and S3). It checks the office token and, when present, the JWT in the
`Authorization` header. GET needs no change to the origin check.

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

Phase 1 keeps the last 20 versions per file. A version list and restore UI is Phase 2 (via the
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
  built per request, so derive it from the setting.
- Run `just api` to regenerate `packages/api` after the server routes land.

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
