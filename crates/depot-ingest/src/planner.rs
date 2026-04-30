use anyhow::{Context as _, Result};

use depot_api::{RequiredUpload, UploadMethod};
use depot_core::depot_path::{DepotObjectPath, parse_depot_object_path};
use depot_core::narinfo::NarInfo;
use depot_core::nix::StorePathHash;
use depot_store::DepotStorage;
use depot_store::upstream::{UpstreamCache, UpstreamCacheClient};

#[derive(Debug, Clone)]
pub struct PlannedUpload {
    pub store_path_hash: StorePathHash,
    pub object_path: String,
    pub content_type: String,
}

impl PlannedUpload {
    pub fn to_api_required_upload(&self, method: UploadMethod) -> RequiredUpload {
        RequiredUpload {
            store_path_hash: self.store_path_hash.as_str().to_owned(),
            object_path: self.object_path.clone(),
            content_type: self.content_type.clone(),
            method,
        }
    }
}

pub async fn plan_required_uploads(
    storage: &dyn DepotStorage,
    upstream_client: &dyn UpstreamCacheClient,
    upstreams: &[UpstreamCache],
    narinfos: &[NarInfo],
) -> Result<Vec<PlannedUpload>> {
    let mut planned = Vec::new();

    for narinfo in narinfos {
        let store_path_hash = StorePathHash::parse_from_store_path(&narinfo.store_path)
            .context("deriving store path hash for upload planning")?;

        let object_path = narinfo.url.clone();

        match parse_depot_object_path(&object_path) {
            Some(DepotObjectPath::Nar { .. }) => {}
            _ => continue,
        }

        if storage.head(&object_path).await?.is_some() {
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

        planned.push(PlannedUpload {
            store_path_hash,
            object_path: object_path.clone(),
            content_type: "application/octet-stream".to_owned(),
        });
    }

    Ok(planned)
}
