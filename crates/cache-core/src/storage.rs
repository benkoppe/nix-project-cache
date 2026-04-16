use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StorageRef {
    LocalBlob {
        backend: String,
        key: String,
    },
    Upstream {
        upstream_id: Uuid,
        object_path: String,
    },
    CasManifest {
        backend: String,
        manifest_id: String,
    },
}
