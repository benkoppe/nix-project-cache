use anyhow::{Context as _, Result, anyhow};

use cache_api::RequiredUpload;
use cache_core::cache_path::{CacheObjectPath, parse_cache_object_path};
use cache_core::narinfo::NarInfo;
use cache_core::nix::StorePathHash;
use cache_core::storage::LocalBackendName;
use cache_store::local::LocalObjectStore;
use cache_store::upstream::{UpstreamCache, UpstreamCacheClient};

#[derive(Debug, Clone)]
pub struct PlannedUpload {
    pub store_path_hash: StorePathHash,
    pub object_path: String,
    pub storage_backend: String,
    pub storage_key: String,
    pub content_type: String,
}

impl PlannedUpload {
    pub fn to_api_required_upload(&self) -> RequiredUpload {
        RequiredUpload {
            store_path_hash: self.store_path_hash.as_str().to_owned(),
            object_path: self.object_path.clone(),
            storage_backend: self.storage_backend.clone(),
            storage_key: self.storage_key.clone(),
            content_type: self.content_type.clone(),
        }
    }
}

pub async fn plan_required_uploads(
    local_store: &dyn LocalObjectStore,
    writable_local_backend: Option<&LocalBackendName>,
    upstream_client: &dyn UpstreamCacheClient,
    upstreams: &[UpstreamCache],
    narinfos: &[NarInfo],
) -> Result<Vec<PlannedUpload>> {
    let mut planned = Vec::new();

    for narinfo in narinfos {
        let store_path_hash = StorePathHash::parse_from_store_path(&narinfo.store_path)
            .context("deriving store path hash for upload planning")?;

        let object_path = narinfo.url.clone();

        match parse_cache_object_path(&object_path) {
            Some(CacheObjectPath::Nar { .. }) => {}
            _ => continue,
        }

        if local_store.head(&object_path).await?.is_some() {
            continue;
        }

        let mut exists_upstream = false;
        for upstream in upstreams {
            if upstream_client
                .head_object(upstream, &object_path)
                .await?
                .is_some()
            {
                exists_upstream = true;
                break;
            }
        }

        if exists_upstream {
            continue;
        }

        let backend = writable_local_backend.ok_or_else(|| {
            anyhow!(
                "local upload required for {} but no writable local backend is configured",
                object_path
            )
        })?;

        planned.push(PlannedUpload {
            store_path_hash,
            object_path: object_path.clone(),
            storage_backend: backend.as_str().to_owned(),
            storage_key: object_path,
            content_type: "application/octet-stream".to_owned(),
        });
    }

    Ok(planned)
}
