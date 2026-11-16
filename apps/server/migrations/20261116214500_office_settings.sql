-- SQLite
-- The office Document Server (ONLYOFFICE Docs) that the root user saves in Admin → Office editing. One row.
-- `ORBIT_ONLYOFFICE_*` variables replace these values while they are set. The integration is off until a URL
-- and a secret are saved and `enabled` is 1.
CREATE TABLE office_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    -- The Document Server as browsers reach it.
    url TEXT CHECK (url IS NULL OR length(url) BETWEEN 1 AND 2048),
    -- The Document Server as Orbit reaches it; NULL: `url`.
    internal_url TEXT CHECK (internal_url IS NULL OR length(internal_url) BETWEEN 1 AND 2048),
    -- Orbit as the Document Server reaches it; NULL: Orbit's public origin.
    orbit_url TEXT CHECK (orbit_url IS NULL OR length(orbit_url) BETWEEN 1 AND 2048),
    -- The shared JWT secret. ChaCha20-Poly1305 under the server app key (secret_box.rs), never plain text.
    jwt_secret BLOB,
    updated_at INTEGER NOT NULL
);

INSERT INTO office_settings (id, updated_at) VALUES (1, 0);
