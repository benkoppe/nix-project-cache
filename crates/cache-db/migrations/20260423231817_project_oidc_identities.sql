CREATE TABLE project_oidc_identities (
    project_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    repository TEXT NOT NULL,
    ref_pattern TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (project_id, provider, repository, ref_pattern),
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

CREATE INDEX idx_project_oidc_identities_lookup
    ON project_oidc_identities(provider, repository);

CREATE INDEX idx_project_oidc_identities_project
    ON project_oidc_identities(project_id);
