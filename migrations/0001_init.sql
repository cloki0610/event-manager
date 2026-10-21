CREATE TABLE events (
    id          TEXT PRIMARY KEY NOT NULL,
    title       TEXT NOT NULL,
    description TEXT,
    location    TEXT,
    starts_at   TEXT NOT NULL,
    ends_at     TEXT,
    capacity    INTEGER,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE INDEX idx_events_starts_at ON events (starts_at);

-- Guests are identities asserted by the SSO provider, keyed by (issuer, subject).
CREATE TABLE guests (
    id         TEXT PRIMARY KEY NOT NULL,
    issuer     TEXT NOT NULL,
    subject    TEXT NOT NULL,
    name       TEXT NOT NULL,
    email      TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (issuer, subject)
);

-- API sessions issued after a successful SSO login. Only token hashes are stored.
CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY NOT NULL,
    guest_id   TEXT NOT NULL REFERENCES guests (id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE INDEX idx_sessions_guest ON sessions (guest_id);

-- In-flight authorization-code logins (single use, short lived).
CREATE TABLE login_states (
    state         TEXT PRIMARY KEY NOT NULL,
    nonce         TEXT NOT NULL,
    pkce_verifier TEXT NOT NULL,
    created_at    TEXT NOT NULL
);

CREATE TABLE attendances (
    event_id  TEXT NOT NULL REFERENCES events (id) ON DELETE CASCADE,
    guest_id  TEXT NOT NULL REFERENCES guests (id) ON DELETE CASCADE,
    joined_at TEXT NOT NULL,
    PRIMARY KEY (event_id, guest_id)
);

CREATE INDEX idx_attendances_guest ON attendances (guest_id);
