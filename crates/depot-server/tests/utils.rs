use std::sync::Arc;

use anyhow::Result;
use tempfile::TempDir;

use depot_client::CacheClient;
use depot_core::nix::StoreDir;
use depot_db::SqliteDatabase;
use depot_server::{AppMode, AppParts, build_app_with_parts};
use depot_store::upstream::{
    InMemoryUpstreamCacheClient, ReqwestUpstreamCacheClient, UpstreamCacheClient,
};
use depot_test_utils::{
    TestDatabase, TestServer, filesystem_storage_in, fixtures::test_signing_key,
};

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
            AppParts {
                db,
                mode: AppMode::ReadWrite,
                store_dir: StoreDir::default(),
                aggregate_signing_key: Some(test_signing_key()),
                key_encryption_key: None,
                storage_catalog: filesystem_storage_in(&temp_dir),
                upstream_client,
                cache_priority: 30,
            },
            Some(WRITE_TOKEN.to_owned()),
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

    pub fn depot_client(&self) -> CacheClient {
        CacheClient::new(self.base_url(), WRITE_TOKEN).unwrap()
    }

    pub fn http_client(&self) -> reqwest::Client {
        reqwest::Client::new()
    }
}
