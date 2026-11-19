# Office editing (ONLYOFFICE)

This is a feature of the `evolv3ai/orbit` fork. Orbit opens Word, Excel, PowerPoint and PDF files in the browser with an [ONLYOFFICE Docs](https://www.onlyoffice.com/docs) Document Server that you run. The files stay in Orbit: the Document Server gets a file from Orbit when someone opens it, and sends the edited file back when they are done.

People open a file with **Open in editor** on a file of a task, a comment, a Docs page or a chat message. The editor opens in a new tab.

- `docx`, `xlsx` and `pptx` open for editing, for people who can edit the task or page. Other people get a read-only view.
- `pdf`, `doc`, `docm`, `dotx`, `odt`, `rtf`, `xls`, `xlsm`, `ods`, `ppt`, `ppsx` and `odp` open read-only.
- A file of a chat message always opens read-only, for everyone who can read the conversation: a message stays as it was sent. To change such a file, download it and add it to a task or a Docs page.
- Mail attachments and the mobile app have no **Open in editor**.

Several people can edit one file at the same time. Orbit stores the file when the last person closes the editor. Orbit keeps the bytes it replaces: the last 20 versions of each file. There is no screen to list or restore versions yet.

## Run the Document Server

Run the `onlyoffice/documentserver` image with JWT on and a secret of your own:

```bash
docker run -d --name documentserver --restart unless-stopped \
  -p 127.0.0.1:8085:80 \
  -e JWT_ENABLED=true \
  -e JWT_SECRET="$(openssl rand -hex 32)" \
  onlyoffice/documentserver
```

Read the secret back with `docker exec documentserver printenv JWT_SECRET`, or generate it first and keep it in your secret store. The first start takes about a minute. `curl http://127.0.0.1:8085/healthcheck` answers `true` when it is ready.

This guide was checked with Document Server 10.0.0.

In production, put the Document Server behind your reverse proxy on its own HTTPS host name, for example `https://office.example.com`. When Orbit is on HTTPS, the Document Server must be on HTTPS too: browsers do not load an HTTP editor into an HTTPS page. The proxy must pass WebSocket connections. The ONLYOFFICE documentation describes HTTPS and running behind a proxy.

### With the Docker deployment of this repository

`deploy/compose.office.yaml` adds a Document Server to the [Docker deployment](../deploy/README.md), behind the same Caddy, on a host name of its own. It also sets `ORBIT_ONLYOFFICE_URL` and `ORBIT_ONLYOFFICE_JWT_SECRET` for Orbit, so Admin shows the settings read-only.

1. Point a second DNS name at the host, for example `office.example.com`.
2. Add two lines to `deploy/.env`: `ORBIT_OFFICE_DOMAIN` (the name, without `https://`) and `ORBIT_ONLYOFFICE_JWT_SECRET` (`openssl rand -hex 32`).
3. Start with both files. Use both in every later `docker compose` command too:

```sh
docker compose --env-file deploy/.env -f deploy/compose.yaml -f deploy/compose.office.yaml up -d
```

The Document Server has no published port, and Caddy gets its certificate. Inside the Docker network, the two host names point at Caddy, so Orbit and the Document Server reach each other without leaving the host and you set neither of the two optional addresses. The Document Server keeps its files, logs and database in four `documentserver-*` volumes. They hold only files that are open and its cache: your files are in Orbit, and Orbit's backups do not include these volumes.

The file pins the Document Server version. Set `ONLYOFFICE_IMAGE` in `deploy/.env` to run another one. Without the second file, the deployment is as before.

## Network paths

Three connections must work. Each one fails in its own way, so check them one at a time.

| From | To | What for |
|---|---|---|
| Browser | Document Server URL | Loads the editor and keeps a WebSocket open while the file is open. |
| Document Server | Orbit | Downloads the file (`GET /api/v1/office/…/download`) and reports the save (`POST /api/v1/office/…/callback`). |
| Orbit | Document Server | The connection test (`GET /healthcheck`), and the download of the edited file after a save. |

By default, each side uses the address that browsers use: the Document Server URL, and `ORBIT_PUBLIC_ORIGIN`. Set the two optional addresses when the servers reach each other on other addresses, for example on a private network:

- **Internal Document Server URL**: the Document Server as Orbit reaches it. Used for the connection test.
- **Orbit URL for the Document Server**: Orbit as the Document Server reaches it. The Document Server downloads files from this address and sends saves to it.

After a save, the Document Server gives Orbit the address of the edited file. Orbit fetches it only when its origin is the Document Server URL or the internal URL, and follows no redirect. The Document Server builds that address from the address the browser used, so **Orbit must be able to reach the Document Server URL itself**. If it cannot, opening works and saving fails.

Orbit must not be reachable only on `localhost` from the Document Server's point of view: inside a container, `localhost` is the container. With Docker Desktop, use `http://host.docker.internal:<port>` as the Orbit URL for the Document Server. On Linux, add `--add-host host.docker.internal:host-gateway` to `docker run`, or put both on one Docker network and use the service name.

## Settings

An instance admin sets the Document Server in **Admin → Office editing**:

1. Enter the **Document Server URL** and the **JWT secret** (the `JWT_SECRET` of the Document Server).
2. If needed, enter the **Internal Document Server URL** and the **Orbit URL for the Document Server**.
3. Select **Test connection**. Orbit asks the Document Server for `/healthcheck`.
4. Turn on **Open office files in the editor** and save.

Orbit stores the secret encrypted, so the server needs `ORBIT_APP_KEY`. A change applies at once, without a restart. Turning the switch off keeps the settings and removes **Open in editor** for everyone.

The variables set the same values. When `ORBIT_ONLYOFFICE_URL` is set, Admin shows the settings read-only.

| Variable | Default | Notes |
|---|---|---|
| `ORBIT_ONLYOFFICE_URL` | saved value | The Document Server as browsers reach it. Requires `ORBIT_ONLYOFFICE_JWT_SECRET`. |
| `ORBIT_ONLYOFFICE_JWT_SECRET`, `ORBIT_ONLYOFFICE_JWT_SECRET_FILE` | saved value | The `JWT_SECRET` of the Document Server, at most 512 characters. |
| `ORBIT_ONLYOFFICE_ENABLED` | `true` | `false` keeps the other values and turns office editing off. |
| `ORBIT_ONLYOFFICE_INTERNAL_URL` | `ORBIT_ONLYOFFICE_URL` | The Document Server as Orbit reaches it. |
| `ORBIT_ONLYOFFICE_ORBIT_URL` | `ORBIT_PUBLIC_ORIGIN` | Orbit as the Document Server reaches it. |

Only a build of this fork knows these variables. The upstream Orbit image refuses to start when one of them is set.

While office editing is on, Orbit's content security policy lets pages load scripts and frames from the origin of the Document Server URL.

## Check that it works

1. Attach a `.docx` file to a task. **Open in editor** shows under the file.
2. Open it, type a word, and close the tab.
3. After about 10 seconds, reload the task. The file size has changed, and the workspace audit log has an `office.file_saved` event.

## Common problems

**There is no "Open in editor".** Office editing is off, the file is not one of the formats above, or the file is a mail attachment. The web app asks for the switch once per five minutes, so a change can take that long to show without a reload.

**"The Document Server could not be reached."** The browser could not load the editor from the Document Server URL. Open `{Document Server URL}/web-apps/apps/api/documents/api.js` in the same browser. Check the URL, the proxy and the certificate, and that an HTTPS Orbit has an HTTPS Document Server.

**The editor shows "The document security token is not correctly formed."** The JWT secret in Orbit is not the `JWT_SECRET` of the Document Server. The Document Server log says `checkJwt error … invalid signature`. Save the right secret in Orbit, or restart the Document Server with it. The connection test does not find this: `/healthcheck` needs no secret.

**The editor opens and then shows a download error.** The Document Server cannot reach Orbit. Run this in the Document Server container, with the Orbit URL for the Document Server:

```bash
docker exec documentserver curl -s -o /dev/null -w '%{http_code}\n' http://host.docker.internal:8080/health/ready
```

It must print `200`. If it does not, fix the address, the firewall or the Docker network. Some Document Server setups refuse private addresses; if the log says so, see `ALLOW_PRIVATE_IP_ADDRESS` in the ONLYOFFICE documentation.

**Edits are lost, or the editor says the file could not be saved.** The save did not reach Orbit, or Orbit could not fetch the edited file. Look at the Orbit log for a warning from `office::callback`; it gives the reason. The usual reasons:

- Orbit cannot reach the Document Server URL (see [Network paths](#network-paths)). Orbit answers the Document Server with 502.
- The address of the edited file is not on the Document Server URL or the internal URL. Orbit answers 403. Check that people open the editor from the same address that is saved in Orbit.
- The edited file is over `ORBIT_UPLOAD_MAX_FILE_BYTES`. Orbit answers 413.

The file in Orbit is unchanged when a save fails. Orbit tells the person who edited last with a notification in their inbox, and the file shows **Last save failed** next to **Open in editor** until a later save works. The workspace audit log has an `office.save_failed` event.

**After a backup restore, a file says "The file cannot be opened yet".** The file was edited after the backup, and the restore brought its earlier content back. The Document Server refuses such a file until Orbit gives it a new key, which happens when someone who can edit the file opens it. Ask one of them to open it once. For people who can edit, the file opens after a short reload.

**The connection test fails.** Orbit cannot reach `{internal URL or Document Server URL}/healthcheck` within 10 seconds, or the answer is not `true`. Run the same request from the Orbit host.

## Limits

- The Document Server is a separate service with its own updates, memory use (plan for 4 GB or more) and license. The free Community Edition allows 20 connections at the same time.
- A person who has a file open keeps editing rights until they close it, also when their access to the task or page is removed in the meantime.
- An admin who deletes a stored file removes its bytes, also where they are an earlier version of an office file.
- Force save is not supported. Leave it off in the Document Server.
