CREATE TABLE access_tokens (
    id TEXT PRIMARY KEY NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    expires_at TEXT,
    revoked_at TEXT
);

CREATE INDEX access_tokens_project_id_idx ON access_tokens(project_id);
CREATE INDEX access_tokens_token_hash_idx ON access_tokens(token_hash);

CREATE TABLE access_token_ref_patterns (
    token_id TEXT NOT NULL REFERENCES access_tokens(id) ON DELETE CASCADE,
    ref_pattern TEXT NOT NULL,
    PRIMARY KEY (token_id, ref_pattern)
);
