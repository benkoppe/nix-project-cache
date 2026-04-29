CREATE TABLE builds (
    id TEXT NOT NULL PRIMARY KEY,
    project_id TEXT NOT NULL,
    ref_name TEXT NOT NULL,
    revision TEXT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    finalized_at TEXT NULL,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

CREATE INDEX idx_builds_project_ref_created_at
    ON builds(project_id, ref_name, created_at DESC);

CREATE INDEX idx_builds_project_created_at
    ON builds(project_id, created_at DESC);

CREATE TABLE build_paths (
    build_id TEXT NOT NULL,
    store_path_hash TEXT NOT NULL,
    PRIMARY KEY (build_id, store_path_hash),
    FOREIGN KEY (build_id) REFERENCES builds(id) ON DELETE CASCADE,
    FOREIGN KEY (store_path_hash) REFERENCES path_infos(store_path_hash) ON DELETE CASCADE
);

CREATE INDEX idx_build_paths_store_path_hash
    ON build_paths(store_path_hash);

CREATE TABLE project_refs (
    project_id TEXT NOT NULL,
    ref_name TEXT NOT NULL,
    build_id TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (project_id, ref_name),
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE,
    FOREIGN KEY (build_id) REFERENCES builds(id) ON DELETE CASCADE
);

CREATE INDEX idx_project_refs_build_id
    ON project_refs(build_id);

CREATE TABLE path_objects (
    store_path_hash TEXT NOT NULL,
    object_path TEXT NOT NULL,
    kind TEXT NOT NULL,
    PRIMARY KEY (store_path_hash, object_path, kind),
    FOREIGN KEY (store_path_hash) REFERENCES path_infos(store_path_hash) ON DELETE CASCADE
);

CREATE INDEX idx_path_objects_object_path
    ON path_objects(object_path);

CREATE VIEW project_visible_paths AS
SELECT DISTINCT
    pr.project_id,
    bp.store_path_hash
FROM project_refs pr
JOIN build_paths bp ON bp.build_id = pr.build_id;

CREATE VIEW aggregate_visible_paths AS
SELECT DISTINCT
    bp.store_path_hash
FROM project_refs pr
JOIN build_paths bp ON bp.build_id = pr.build_id
JOIN projects p ON p.id = pr.project_id
WHERE p.public = 1;
