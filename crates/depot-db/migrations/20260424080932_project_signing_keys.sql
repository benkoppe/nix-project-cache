CREATE TABLE project_signing_keys (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    public_key TEXT NOT NULL,
    encrypted_private_key TEXT NOT NULL,
    nonce TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    retired_at TEXT
);

CREATE UNIQUE INDEX project_signing_keys_one_active_idx
    ON project_signing_keys(project_id)
    WHERE retired_at IS NULL;

CREATE INDEX project_signing_keys_project_id_idx
    ON project_signing_keys(project_id);
