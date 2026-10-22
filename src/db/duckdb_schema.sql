-- DuckDB schema, applied at startup. Mirrors migrations/0001_init.sql, minus foreign keys:
-- DuckDB does not support ON DELETE CASCADE, so src/db/duckdb.rs cascades deletes itself.
-- Timestamps are UTC.

CREATE TABLE IF NOT EXISTS events (
    id          VARCHAR PRIMARY KEY,
    title       VARCHAR NOT NULL,
    description VARCHAR,
    location    VARCHAR,
    starts_at   TIMESTAMP NOT NULL,
    ends_at     TIMESTAMP,
    capacity    BIGINT,
    created_at  TIMESTAMP NOT NULL,
    updated_at  TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS guests (
    id         VARCHAR PRIMARY KEY,
    issuer     VARCHAR NOT NULL,
    subject    VARCHAR NOT NULL,
    name       VARCHAR NOT NULL,
    email      VARCHAR,
    created_at TIMESTAMP NOT NULL,
    updated_at TIMESTAMP NOT NULL,
    UNIQUE (issuer, subject)
);

CREATE TABLE IF NOT EXISTS sessions (
    token_hash VARCHAR PRIMARY KEY,
    guest_id   VARCHAR NOT NULL,
    created_at TIMESTAMP NOT NULL,
    expires_at TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS login_states (
    state         VARCHAR PRIMARY KEY,
    nonce         VARCHAR NOT NULL,
    pkce_verifier VARCHAR NOT NULL,
    created_at    TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS attendances (
    event_id  VARCHAR NOT NULL,
    guest_id  VARCHAR NOT NULL,
    joined_at TIMESTAMP NOT NULL,
    PRIMARY KEY (event_id, guest_id)
);
