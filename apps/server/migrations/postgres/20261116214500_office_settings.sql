-- Postgres: the same change as apps/server/migrations/20261116214500_office_settings.sql
CREATE TABLE office_settings (
    id BIGINT PRIMARY KEY CHECK (id = 1),
    enabled BIGINT NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    url TEXT CHECK (url IS NULL OR length(url) BETWEEN 1 AND 2048),
    internal_url TEXT CHECK (internal_url IS NULL OR length(internal_url) BETWEEN 1 AND 2048),
    orbit_url TEXT CHECK (orbit_url IS NULL OR length(orbit_url) BETWEEN 1 AND 2048),
    jwt_secret BYTEA,
    updated_at BIGINT NOT NULL
);

INSERT INTO office_settings (id, updated_at) VALUES (1, 0);
