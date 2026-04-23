use std::sync::Arc;

use anyhow::Result;
use tempfile::TempDir;

use cache_app::build_app_with_parts;
use cache_client::CacheClient;
use cache_core::nix::StoreDir;
use cache_core::storage::LocalBackendName;
use cache_db::SqliteDatabase;
use cache_store::upstream::{
    InMemoryUpstreamCacheClient, ReqwestUpstreamCacheClient, UpstreamCacheClient,
};
use cache_test_utils::{TestDatabase, TestServer, filesystem_backends_in, test_signing_keys};

pub const WRITE_TOKEN: &str = "secret-token";

pub struct TestApp {
    _temp_dir: TempDir,
    server: TestServer,
}

impl TestApp {
    pub async fn spawn() -> Result<Self> {
        let fixture = TestDatabase::new().await?;
        Self::spawn_with_upstream_client(
            fixture.temp_dir,
            fixture.db,
            Arc::new(ReqwestUpstreamCacheClient::default()),
        )
        .await
    }

    pub async fn spawn_with_prepared_upstream(
        temp_dir: TempDir,
        db: SqliteDatabase,
        upstream_client: InMemoryUpstreamCacheClient,
    ) -> Result<Self> {
        Self::spawn_with_upstream_client(temp_dir, db, Arc::new(upstream_client)).await
    }

    async fn spawn_with_upstream_client(
        temp_dir: TempDir,
        db: SqliteDatabase,
        upstream_client: Arc<dyn UpstreamCacheClient>,
    ) -> Result<Self> {
        let app = build_app_with_parts(
            db,
            StoreDir::default(),
            test_signing_keys(),
            filesystem_backends_in(&temp_dir),
            Some(LocalBackendName::fs()),
            Some(WRITE_TOKEN.to_owned()),
            upstream_client,
        );

        let server = TestServer::spawn(app).await?;

        Ok(Self {
            _temp_dir: temp_dir,
            server,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.server.base_url
    }

    pub fn url(&self, path: impl AsRef<str>) -> String {
        self.server.url(path)
    }

    pub fn cache_client(&self) -> CacheClient {
        CacheClient::new(self.base_url(), WRITE_TOKEN).unwrap()
    }

    pub fn http_client(&self) -> reqwest::Client {
        reqwest::Client::new()
    }
}
