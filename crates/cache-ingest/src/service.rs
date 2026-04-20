use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use bytes::Bytes;
use uuid::Uuid;

use cache_api::{
    BeginBuildRequest, BeginBuildResponse, FinalizeBuildRequest, FinalizeBuildResponse,
    RegisterPathsRequest, RegisterPathsResponse,
};
use cache_core::cache_path::{CacheObjectPath, parse_cache_object_path};
use cache_core::nix::StorePathHash;
use cache_core::project::ProjectSlug;
use cache_db::{BuildStatus, SqliteDatabase};
use cache_store::blob::BlobMetadata;
use cache_store::local::{LocalObjectBackendRegistry, LocalObjectStore};
use cache_store::upstream::{UpstreamCache, UpstreamCacheClient};

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

        let upstreams = self.project_upstreams(&build_context.project_slug).await?;

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
        store_path_hash: &StorePathHash,
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

        let Some(expected_object_path) = self
            .db
            .get_build_path_nar_object_path(build_id, store_path_hash)
            .await?
        else {
            return Err(anyhow!(
                "build {} does not include path {}",
                build_id,
                store_path_hash.as_str()
            ));
        };

        if object_path != expected_object_path {
            return Err(anyhow!(
                "object path {} does not match registered nar {} for path {}",
                object_path,
                expected_object_path,
                store_path_hash.as_str(),
            ));
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
        self.db
            .link_path_object(store_path_hash, object_path, "nar")
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

        let build_paths = self.db.list_build_path_nar_objects(build_id).await?;
        if build_paths.is_empty() {
            return Err(anyhow!("build {} has no registered paths", build_id));
        }

        let upstreams = self.project_upstreams(&build_context.project_slug).await?;

        for (store_path_hash, object_path) in build_paths {
            let locally_available = self
                .db
                .path_has_object(&store_path_hash, &object_path, "nar")
                .await?
                && self.local_store.head(&object_path).await?.is_some();

            if locally_available {
                continue;
            }

            if self
                .object_exists_upstream(&upstreams, &object_path)
                .await?
            {
                continue;
            }

            return Err(anyhow!(
                "cannot finalize build {}: required nar object {} for path {} is not available locally or upstream",
                build_id,
                object_path,
                store_path_hash.as_str()
            ));
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

    async fn project_upstreams(&self, project_slug: &ProjectSlug) -> Result<Vec<UpstreamCache>> {
        Ok(self
            .db
            .list_enabled_upstreams_for_project(project_slug)
            .await?
            .into_iter()
            .map(|record| record.into_runtime_config())
            .collect())
    }

    async fn object_exists_upstream(
        &self,
        upstreams: &[UpstreamCache],
        object_path: &str,
    ) -> Result<bool> {
        for upstream in upstreams {
            if self
                .upstream_client
                .head_object(upstream, object_path)
                .await?
                .is_some()
            {
                return Ok(true);
            }
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::Bytes;
    use tempfile::TempDir;
    use uuid::Uuid;

    use cache_api::{BeginBuildRequest, NarInfoPayload, RegisterPathsRequest};
    use cache_core::nix::{NixHash, StorePathHash};
    use cache_core::project::ProjectSlug;
    use cache_db::{BuildStatus, SqliteDatabase};
    use cache_read::DbBackedLocalObjectStore;
    use cache_store::blob::BlobMetadata;
    use cache_store::local::{FilesystemLocalObjectBackend, LocalObjectBackendRegistry};
    use cache_store::upstream::{InMemoryUpstreamCacheClient, UpstreamCache};

    use super::*;

    fn sample_payload() -> NarInfoPayload {
        NarInfoPayload {
            store_path: "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1".to_owned(),
            url: "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst".to_owned(),
            compression: "zstd".to_owned(),
            nar_hash: NixHash::Raw(
                "sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=".to_owned(),
            )
            .render_text(),
            nar_size: 226560,
            references: vec![],
            deriver: None,
            signatures: vec![],
            ca: None,
        }
    }

    fn sample_hash() -> StorePathHash {
        StorePathHash::parse_from_store_path(
            "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1",
        )
        .unwrap()
    }

    async fn build_service(
        upstream_client: InMemoryUpstreamCacheClient,
    ) -> (IngestService, SqliteDatabase, TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("cache.db");
        let objects_root = temp_dir.path().join("objects");

        let db = SqliteDatabase::open(&db_path).await.unwrap();
        let project = ProjectSlug::parse("example_repo").unwrap();
        db.insert_project(&project, "Example Repo", true)
            .await
            .unwrap();

        let fs_backend = Arc::new(FilesystemLocalObjectBackend::new(&objects_root));
        let mut backends = LocalObjectBackendRegistry::new();
        backends.register("fs", fs_backend);

        let local_store = DbBackedLocalObjectStore::new(db.clone(), backends.clone());

        let service = IngestService::new(
            db.clone(),
            Arc::new(local_store),
            backends,
            Arc::new(upstream_client),
        );

        (service, db, temp_dir)
    }

    #[tokio::test]
    async fn upload_links_path_object_and_finalize_succeeds() {
        let (service, db, _tmp) = build_service(InMemoryUpstreamCacheClient::new()).await;
        let payload = sample_payload();
        let hash = sample_hash();

        let begin = service
            .begin_build(BeginBuildRequest {
                project: "example_repo".to_owned(),
                ref_name: "main".to_owned(),
                revision: Some("deadbeef".to_owned()),
            })
            .await
            .unwrap();

        let build_id = Uuid::parse_str(&begin.build_id).unwrap();

        let register = service
            .register_paths(RegisterPathsRequest {
                build_id: begin.build_id.clone(),
                paths: vec![payload.clone()],
            })
            .await
            .unwrap();

        assert_eq!(register.required_uploads.len(), 1);
        assert_eq!(register.required_uploads[0].store_path_hash, hash.as_str());
        assert_eq!(register.required_uploads[0].object_path, payload.url);

        service
            .upload_object(
                build_id,
                &hash,
                &payload.url,
                Bytes::from_static(b"nar-bytes"),
            )
            .await
            .unwrap();

        assert!(
            db.path_has_object(&hash, &payload.url, "nar")
                .await
                .unwrap()
        );

        service
            .finalize_build(FinalizeBuildRequest {
                build_id: begin.build_id.clone(),
            })
            .await
            .unwrap();

        let build = db.get_build(build_id).await.unwrap().unwrap();
        assert_eq!(build.status, BuildStatus::Finalized);
    }

    #[tokio::test]
    async fn finalize_rejects_missing_required_nar() {
        let (service, _db, _tmp) = build_service(InMemoryUpstreamCacheClient::new()).await;
        let payload = sample_payload();

        let begin = service
            .begin_build(BeginBuildRequest {
                project: "example_repo".to_owned(),
                ref_name: "main".to_owned(),
                revision: None,
            })
            .await
            .unwrap();

        service
            .register_paths(RegisterPathsRequest {
                build_id: begin.build_id.clone(),
                paths: vec![payload],
            })
            .await
            .unwrap();

        let error = service
            .finalize_build(FinalizeBuildRequest {
                build_id: begin.build_id,
            })
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("is not available locally or upstream")
        );
    }

    #[tokio::test]
    async fn finalize_allows_upstream_backed_path_without_local_upload() {
        let payload = sample_payload();
        let hash = sample_hash();

        let mut upstream_client = InMemoryUpstreamCacheClient::new();
        let upstream = UpstreamCache::new(
            Uuid::now_v7(),
            "cache.nixos.org",
            "https://cache.nixos.org",
            10,
        );
        upstream_client.insert_object(
            upstream.id,
            payload.url.clone(),
            BlobMetadata::new("application/octet-stream", Some(12), None, None),
            Bytes::from_static(b"upstream-nar"),
        );

        let (service, db, _tmp) = build_service(upstream_client).await;
        let project = ProjectSlug::parse("example_repo").unwrap();

        db.insert_upstream_cache(&upstream, true).await.unwrap();
        db.link_project_upstream(&project, upstream.id)
            .await
            .unwrap();

        let begin = service
            .begin_build(BeginBuildRequest {
                project: "example_repo".to_owned(),
                ref_name: "main".to_owned(),
                revision: None,
            })
            .await
            .unwrap();

        service
            .register_paths(RegisterPathsRequest {
                build_id: begin.build_id.clone(),
                paths: vec![payload.clone()],
            })
            .await
            .unwrap();

        service
            .finalize_build(FinalizeBuildRequest {
                build_id: begin.build_id.clone(),
            })
            .await
            .unwrap();

        assert!(
            !db.path_has_object(&hash, &payload.url, "nar")
                .await
                .unwrap()
        );

        let build_id = Uuid::parse_str(&begin.build_id).unwrap();
        let build = db.get_build(build_id).await.unwrap().unwrap();
        assert_eq!(build.status, BuildStatus::Finalized);
    }
}
