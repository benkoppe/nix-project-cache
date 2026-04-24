CREATE TABLE project_retention_policies (
    project_id TEXT PRIMARY KEY NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    keep_latest_builds_per_ref INTEGER NOT NULL DEFAULT 2,
    object_delete_grace_seconds INTEGER NOT NULL DEFAULT 86400,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE project_ref_retention_rules (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    priority INTEGER NOT NULL,
    ref_pattern TEXT NOT NULL,
    ttl_seconds INTEGER,
    keep_builds INTEGER,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(project_id, priority)
);

CREATE INDEX project_ref_retention_rules_project_id_idx
    ON project_ref_retention_rules(project_id);

CREATE INDEX project_ref_retention_rules_project_priority_idx
    ON project_ref_retention_rules(project_id, priority);
