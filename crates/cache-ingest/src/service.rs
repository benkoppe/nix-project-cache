use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use bytes::Bytes;
use uuid::Uuid;

use cache_api::{
    BeginBuildRequest, BeginBuildResponse, FinalizeBuildRequest, FinalizeBuildResponse,
    RegisterPathsRequest, RegisterPathsResponse,
};
use cache_core::cache_path::{CacheObjectPath, parse_cache_object_path};
use cache_core::project::ProjectSlug;
use cache_db::{BuildStatus, SqliteDatabase};
use cache_store::blob::BlobMetadata;
use cache_store::local::{LocalObjectBackendRegistry, LocalObjectStore};
use cache_store::upstream::UpstreamCacheClient;

use crate::planner::plan_required_uploads;

#[derive(Clone)]
pub struct IngestService {
    db: SqliteDatabase,
    local_store: Arc<dyn LocalObjectStore>,
    local_backends: LocalObjectBackendRegistry,
    upstream_client: Arc<dyn UpstreamCacheClient>,
}

impl IngestService {
    pub fn new(
        db: SqliteDatabase,
        local_store: Arc<dyn LocalObjectStore>,
        local_backends: LocalObjectBackendRegistry,
        upstream_client: Arc<dyn UpstreamCacheClient>,
    ) -> Self {
        Self {
            db,
            local_store,
            local_backends,
            upstream_client,
        }
    }

    pub async fn begin_build(&self, request: BeginBuildRequest) -> Result<BeginBuildResponse> {
        let project = ProjectSlug::parse(&request.project)
            .map_err(|_| anyhow!("invalid project slug {}", request.project))?;

        let build = self
            .db
            .begin_build(&project, &request.ref_name, request.revision.as_deref())
            .await?;

        Ok(BeginBuildResponse {
            build_id: build.id.to_string(),
        })
    }

    pub async fn register_paths(
        &self,
        request: RegisterPathsRequest,
    ) -> Result<RegisterPathsResponse> {
        let build_id = Uuid::parse_str(&request.build_id).context("parsing build_id")?;
        let build_context = self
            .db
            .get_build_context(build_id)
            .await?
            .ok_or_else(|| anyhow!("build {} not found", build_id))?;

        if build_context.status != BuildStatus::Pending {
            return Err(anyhow!("build {} is not pending", build_id));
        }

        let mut narinfos = Vec::with_capacity(request.paths.len());

        for path in request.paths {
            let narinfo = cache_core::narinfo::NarInfo::try_from(path)?;
            let store_path_hash =
                cache_core::nix::StorePathHash::parse_from_store_path(&narinfo.store_path)
                    .context("deriving store_path_hash from registered path")?;

            self.db.upsert_path_info(&narinfo).await?;
            self.db
                .attach_build_path(build_id, &store_path_hash)
                .await?;

            narinfos.push(narinfo);
        }

        let upstreams = self
            .db
            .list_enabled_upstreams_for_project(&build_context.project_slug)
            .await?
            .into_iter()
            .map(|record| record.into_runtime_config())
            .collect::<Vec<_>>();

        let planned = plan_required_uploads(
            self.local_store.as_ref(),
            self.upstream_client.as_ref(),
            &upstreams,
            &narinfos,
        )
        .await?;

        Ok(RegisterPathsResponse {
            required_uploads: planned
                .iter()
                .map(|planned| planned.to_api_required_upload())
                .collect(),
        })
    }

    pub async fn upload_object(
        &self,
        build_id: Uuid,
        object_path: &str,
        body: Bytes,
    ) -> Result<()> {
        let build_context = self
            .db
            .get_build_context(build_id)
            .await?
            .ok_or_else(|| anyhow!("build {} not found", build_id))?;

        if build_context.status != BuildStatus::Pending {
            return Err(anyhow!("build {} is not pending", build_id));
        }

        match parse_cache_object_path(object_path) {
            Some(CacheObjectPath::Nar { .. }) => {}
            Some(CacheObjectPath::NarInfo { .. }) => {
                return Err(anyhow!("clients must not upload narinfo objects directly"));
            }
            _ => return Err(anyhow!("invalid upload object path {}", object_path)),
        }

        let backend = self.local_backends.require("fs")?;
        backend.put_bytes(object_path, body.clone()).await?;

        let metadata = BlobMetadata::new(
            "application/octet-stream",
            Some(u64::try_from(body.len()).context("converting body length")?),
            None,
            None,
        );

        self.db
            .upsert_local_object(object_path, &metadata, "fs", object_path)
            .await?;

        Ok(())
    }

    pub async fn finalize_build(
        &self,
        request: FinalizeBuildRequest,
    ) -> Result<FinalizeBuildResponse> {
        let build_id = Uuid::parse_str(&request.build_id).context("parsing build_id")?;
        let build_context = self
            .db
            .get_build_context(build_id)
            .await?
            .ok_or_else(|| anyhow!("build {} not found", build_id))?;

        if build_context.status != BuildStatus::Pending {
            return Err(anyhow!("build {} is not pending", build_id));
        }

        self.db
            .publish_build_to_ref(
                &build_context.project_slug,
                &build_context.ref_name,
                build_id,
            )
            .await?;

        Ok(FinalizeBuildResponse {})
    }
}
