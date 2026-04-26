CREATE TABLE projects (
    id TEXT NOT NULL PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    public INTEGER NOT NULL DEFAULT 1,
    storage_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE upstream_caches (
    id TEXT NOT NULL PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    base_url TEXT NOT NULL,
    priority INTEGER NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE project_upstreams (
    project_id TEXT NOT NULL,
    upstream_id TEXT NOT NULL,
    PRIMARY KEY (project_id, upstream_id),
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE,
    FOREIGN KEY (upstream_id) REFERENCES upstream_caches(id) ON DELETE CASCADE
);

CREATE TABLE path_infos (
    store_path_hash TEXT NOT NULL PRIMARY KEY,
    store_path TEXT NOT NULL UNIQUE,
    url TEXT NOT NULL,
    compression TEXT NOT NULL,
    nar_hash TEXT NOT NULL,
    nar_size INTEGER NOT NULL,
    deriver TEXT NULL,
    ca TEXT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE path_references (
    store_path_hash TEXT NOT NULL,
    reference_store_path TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (store_path_hash, ordinal),
    FOREIGN KEY (store_path_hash) REFERENCES path_infos(store_path_hash) ON DELETE CASCADE
);

CREATE TABLE path_signatures (
    store_path_hash TEXT NOT NULL,
    signature TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (store_path_hash, ordinal),
    FOREIGN KEY (store_path_hash) REFERENCES path_infos(store_path_hash) ON DELETE CASCADE
);

CREATE TABLE storage_objects (
    storage_id TEXT NOT NULL,
    object_path TEXT NOT NULL,
    content_type TEXT NOT NULL,
    content_length INTEGER NULL,
    etag TEXT NULL,
    last_modified TEXT NULL,
    deleted_at TEXT NULL,
    first_deleted_at TEXT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (storage_id, object_path)
);

CREATE INDEX idx_projects_slug ON projects(slug);
CREATE INDEX idx_project_upstreams_upstream_id ON project_upstreams(upstream_id);
CREATE INDEX idx_path_references_store_path_hash ON path_references(store_path_hash);
CREATE INDEX idx_path_signatures_store_path_hash ON path_signatures(store_path_hash);
CREATE INDEX idx_upstream_caches_enabled_priority ON upstream_caches(enabled, priority);
CREATE INDEX idx_storage_objects_object_path ON storage_objects(object_path);
CREATE INDEX idx_storage_objects_deleted_at ON storage_objects(deleted_at, first_deleted_at);
