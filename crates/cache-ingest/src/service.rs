use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use uuid::Uuid;

use cache_api::{
    BeginBuildRequest, BeginBuildResponse, FinalizeBuildRequest, FinalizeBuildResponse,
    RegisterPathsRequest, RegisterPathsResponse, UploadMethod,
};
use cache_core::cache_path::{CacheObjectPath, parse_cache_object_path};
use cache_core::narinfo::NarInfo;
use cache_core::nix::{StoreDir, StorePathHash};
use cache_core::project::ProjectSlug;
use cache_core::storage::{PathObjectKind, StorageId};
use cache_core::validation::validate_publish_narinfo;
use cache_db::{BuildStatus, SqliteDatabase};
use cache_store::blob::BlobMetadata;
use cache_store::upstream::{UpstreamCache, UpstreamCacheClient};
use cache_store::{StorageCatalog, UploadReader};

use crate::planner::plan_required_uploads;

const PRESIGNED_PUT_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone)]
pub struct IngestService {
    db: SqliteDatabase,
    store_dir: StoreDir,
    storage_catalog: StorageCatalog,
    upstream_client: Arc<dyn UpstreamCacheClient>,
}

impl IngestService {
    pub fn new(
        db: SqliteDatabase,
        store_dir: StoreDir,
        storage_catalog: StorageCatalog,
        upstream_client: Arc<dyn UpstreamCacheClient>,
    ) -> Self {
        Self {
            db,
            store_dir,
            storage_catalog,
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
            let narinfo = NarInfo::try_from(path)?;
            let store_path_hash = validate_publish_narinfo(&self.store_dir, &narinfo)?;

            self.db.upsert_path_info(&narinfo).await?;
            self.db
                .attach_build_path(build_id, &store_path_hash)
                .await?;

            narinfos.push(narinfo);
        }

        let storage_id = self
            .storage_id_for_project(&build_context.project_slug)
            .await?;
        let storage = self.storage_catalog.storage(&storage_id)?;

        let upstreams = self.project_upstreams(&build_context.project_slug).await?;

        let planned = plan_required_uploads(
            storage.as_ref(),
            self.upstream_client.as_ref(),
            &upstreams,
            &narinfos,
        )
        .await?;

        for narinfo in &narinfos {
            let store_path_hash = StorePathHash::parse_from_store_path(&narinfo.store_path)
                .context("deriving store path hash for path object linking")?;
            let object_path = narinfo.url.clone();

            match parse_cache_object_path(&object_path) {
                Some(CacheObjectPath::Nar { .. }) => {}
                _ => continue,
            }

            if let Some(metadata) = storage.head(&object_path).await? {
                self.db
                    .upsert_storage_object(&storage_id, &object_path, &metadata)
                    .await?;
                self.db
                    .link_path_object(&store_path_hash, &object_path, PathObjectKind::Nar)
                    .await?;
            }
        }

        let mut required_uploads = Vec::with_capacity(planned.len());

        for planned_upload in planned {
            let method = match storage
                .presigned_put_url(&planned_upload.object_path, PRESIGNED_PUT_TTL)
                .await?
            {
                Some(url) => UploadMethod::PresignedPut {
                    url: url.url,
                    expires_at: url.expires_at.to_string(),
                },
                None => UploadMethod::Proxy,
            };

            required_uploads.push(planned_upload.to_api_required_upload(method));
        }

        Ok(RegisterPathsResponse { required_uploads })
    }

    pub async fn upload_object(
        &self,
        build_id: Uuid,
        store_path_hash: &StorePathHash,
        object_path: &str,
        body: UploadReader,
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

        let Some(narinfo) = self
            .db
            .get_build_path_narinfo(build_id, store_path_hash)
            .await?
        else {
            return Err(anyhow!(
                "build {} does not include path {}",
                build_id,
                store_path_hash.as_str()
            ));
        };

        let expected_object_path = narinfo.url.as_str();

        if object_path != expected_object_path {
            return Err(anyhow!(
                "object path {} does not match registered nar {} for path {}",
                object_path,
                expected_object_path,
                store_path_hash.as_str(),
            ));
        }

        let storage_id = self
            .storage_id_for_project(&build_context.project_slug)
            .await?;
        let storage = self.storage_catalog.storage(&storage_id)?;
        let written = storage.put_stream(object_path, body).await?;

        let metadata = BlobMetadata::new("application/octet-stream", Some(written), None, None);

        let persist_result = async {
            self.db
                .upsert_storage_object(&storage_id, object_path, &metadata)
                .await?;
            self.db
                .link_path_object(store_path_hash, object_path, PathObjectKind::Nar)
                .await?;

            Ok::<(), anyhow::Error>(())
        }
        .await;

        if persist_result.is_err() {
            let _ = storage.delete(object_path).await;
        }

        persist_result
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

        let storage_id = self
            .storage_id_for_project(&build_context.project_slug)
            .await?;
        let storage = self.storage_catalog.storage(&storage_id)?;

        for (store_path_hash, object_path) in build_paths {
            if let Some(metadata) = storage.head(&object_path).await? {
                self.db
                    .upsert_storage_object(&storage_id, &object_path, &metadata)
                    .await?;
                self.db
                    .link_path_object(&store_path_hash, &object_path, PathObjectKind::Nar)
                    .await?;
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

    async fn storage_id_for_project(&self, project_slug: &ProjectSlug) -> Result<StorageId> {
        self.db.get_project_storage_id(project_slug).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::TempDir;
    use uuid::Uuid;

    use cache_api::{BeginBuildRequest, FinalizeBuildRequest, RegisterPathsRequest};
    use cache_core::nix::StoreDir;
    use cache_core::storage::PathObjectKind;
    use cache_db::{BuildStatus, SqliteDatabase};
    use cache_store::blob::{BlobBytes, BlobMetadata};
    use cache_store::upstream::InMemoryUpstreamCacheClient;
    use cache_test_utils::{
        EXAMPLE_PROJECT_SLUG, TestDatabase, duplex_reader, example_project, filesystem_storage_in,
        hello_path, sample_upstream,
    };

    use super::*;

    async fn build_service(
        upstream_client: InMemoryUpstreamCacheClient,
    ) -> (IngestService, SqliteDatabase, TempDir) {
        let fixture = TestDatabase::new().await.unwrap();
        fixture.insert_example_project().await.unwrap();

        let storage_catalog = filesystem_storage_in(&fixture.temp_dir);

        let service = IngestService::new(
            fixture.db.clone(),
            StoreDir::default(),
            storage_catalog,
            Arc::new(upstream_client),
        );

        (service, fixture.db, fixture.temp_dir)
    }

    async fn begin_example_build(service: &IngestService) -> String {
        service
            .begin_build(BeginBuildRequest {
                project: EXAMPLE_PROJECT_SLUG.to_owned(),
                ref_name: "refs/heads/main".to_owned(),
                revision: Some("deadbeef".to_owned()),
            })
            .await
            .unwrap()
            .build_id
    }

    async fn register_single_path_error(
        service: &IngestService,
        payload: cache_api::NarInfoPayload,
    ) -> anyhow::Error {
        let build_id = begin_example_build(service).await;
        service
            .register_paths(RegisterPathsRequest {
                build_id,
                paths: vec![payload],
            })
            .await
            .unwrap_err()
    }

    #[tokio::test]
    async fn upload_links_path_object_and_finalize_succeeds() {
        let (service, db, _tmp) = build_service(InMemoryUpstreamCacheClient::new()).await;

        let path = hello_path();
        let payload = path.payload();
        let hash = path.hash();

        let begin = service
            .begin_build(BeginBuildRequest {
                project: EXAMPLE_PROJECT_SLUG.to_owned(),
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
            .upload_object(build_id, &hash, &payload.url, duplex_reader(b"nar-bytes"))
            .await
            .unwrap();

        assert!(
            db.path_has_object(&hash, &payload.url, PathObjectKind::Nar)
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
        let payload = hello_path().payload();

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
        let path = hello_path();
        let payload = path.payload();
        let hash = path.hash();

        let mut upstream_client = InMemoryUpstreamCacheClient::new();
        let upstream = sample_upstream("https://cache.nixos.org");

        upstream_client.insert_object(
            upstream.id,
            payload.url.clone(),
            BlobMetadata::new("application/octet-stream", Some(12), None, None),
            BlobBytes::from_static(b"upstream-nar"),
        );

        let (service, db, _tmp) = build_service(upstream_client).await;
        let project = example_project();

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
            !db.path_has_object(&hash, &payload.url, PathObjectKind::Nar)
                .await
                .unwrap()
        );

        let build_id = Uuid::parse_str(&begin.build_id).unwrap();
        let build = db.get_build(build_id).await.unwrap().unwrap();
        assert_eq!(build.status, BuildStatus::Finalized);
    }

    #[tokio::test]
    async fn register_paths_rejects_store_path_outside_store_dir() {
        let (service, _db, _tmp) = build_service(InMemoryUpstreamCacheClient::new()).await;

        let mut payload = hello_path().payload();
        payload.store_path = "/tmp/not-in-store".to_owned();

        let error = register_single_path_error(&service, payload).await;

        assert!(
            error.to_string().contains("not inside store dir"),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn register_paths_rejects_url_that_does_not_match_nar_hash() {
        let (service, _db, _tmp) = build_service(InMemoryUpstreamCacheClient::new()).await;

        let mut payload = hello_path().payload();
        payload.url = "nar/1111111111111111111111111111111111111111111111111111.nar.zst".to_owned();

        let error = register_single_path_error(&service, payload).await;

        assert!(
            error.to_string().contains("does not match expected URL"),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn register_paths_rejects_compression_that_does_not_match_url() {
        let (service, _db, _tmp) = build_service(InMemoryUpstreamCacheClient::new()).await;

        let mut payload = hello_path().payload();
        payload.compression = "xz".to_owned();

        let error = register_single_path_error(&service, payload).await;

        assert!(
            error.to_string().contains("does not match URL compression"),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn register_paths_rejects_reference_outside_store_dir() {
        let (service, _db, _tmp) = build_service(InMemoryUpstreamCacheClient::new()).await;

        let mut payload = hello_path().payload();
        payload.references = vec!["/tmp/not-in-store".to_owned()];

        let error = register_single_path_error(&service, payload).await;

        assert!(error.to_string().contains("reference"), "{error:?}");
    }

    #[tokio::test]
    async fn register_paths_rejects_deriver_outside_store_dir() {
        let (service, _db, _tmp) = build_service(InMemoryUpstreamCacheClient::new()).await;

        let mut payload = hello_path().payload();
        payload.deriver = Some("/tmp/not-in-store.drv".to_owned());

        let error = register_single_path_error(&service, payload).await;

        assert!(error.to_string().contains("deriver"), "{error:?}");
    }
}
