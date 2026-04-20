CREATE TABLE pins (
    scope_key TEXT NOT NULL,
    name TEXT NOT NULL,
    project_id TEXT NULL,
    store_path_hash TEXT NOT NULL,
    store_path TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (scope_key, name),
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE,
    FOREIGN KEY (store_path_hash) REFERENCES path_infos(store_path_hash) ON DELETE CASCADE
);

CREATE INDEX idx_pins_project_id
    ON pins(project_id);

CREATE INDEX idx_pins_store_path_hash
    ON pins(store_path_hash);
