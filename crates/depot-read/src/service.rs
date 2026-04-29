use std::sync::Arc;

use anyhow::Result;

use depot_core::narinfo::{NarInfo, NarInfoRenderer, parse_narinfo};
use depot_core::nix::{StoreDir, StorePathHash};
use depot_core::project::ProjectSlug;
use depot_core::signing::{NamedSigningKey, NarInfoSigner};
use depot_core::view::CacheView;
use depot_store::blob::{BlobBytes, BlobMetadata};
use depot_store::upstream::UpstreamCacheClient;

use crate::object_provider::CacheObjectProvider;
use crate::resolver::NarInfoResolver;
use crate::signing_keys::DbProjectSigningKeys;
use crate::upstreams::UpstreamSelector;

#[derive(Clone)]
pub struct ReadService {
    local_resolver: Arc<dyn NarInfoResolver>,
    object_provider: Arc<dyn CacheObjectProvider>,
    upstream_client: Arc<dyn UpstreamCacheClient>,
    upstream_selector: Arc<dyn UpstreamSelector>,
    renderer: NarInfoRenderer,
    aggregate_signing_key: Option<NamedSigningKey>,
    project_signing_keys: Option<DbProjectSigningKeys>,
}

impl ReadService {
    pub fn new(
        local_resolver: Arc<dyn NarInfoResolver>,
        object_provider: Arc<dyn CacheObjectProvider>,
        upstream_client: Arc<dyn UpstreamCacheClient>,
        upstream_selector: Arc<dyn UpstreamSelector>,
        renderer: NarInfoRenderer,
        aggregate_signing_key: Option<NamedSigningKey>,
        project_signing_keys: Option<DbProjectSigningKeys>,
    ) -> Self {
        Self {
            local_resolver,
            object_provider,
            upstream_client,
            upstream_selector,
            renderer,
            aggregate_signing_key,
            project_signing_keys,
        }
    }

    pub fn store_dir(&self) -> &StoreDir {
        self.renderer.store_dir()
    }

    pub async fn public_key_texts_for_view(&self, view: &CacheView) -> Result<Vec<String>> {
        match view {
            CacheView::Aggregate => Ok(self.aggregate_public_keys()),
            CacheView::Project(project) => self.project_public_keys(project).await,
        }
    }

    pub async fn render_narinfo(
        &self,
        view: &CacheView,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<String>> {
        if let Some(narinfo) = self
            .local_resolver
            .resolve_narinfo(view, store_path_hash)
            .await?
        {
            return Ok(Some(self.render_signed_narinfo(view, narinfo).await?));
        }

        for upstream in self.upstream_selector.upstreams_for_view(view).await? {
            if let Some(narinfo_text) = self
                .upstream_client
                .fetch_narinfo_text(&upstream, store_path_hash)
                .await?
            {
                let narinfo = parse_narinfo(&narinfo_text, self.renderer.store_dir())?;
                return Ok(Some(self.render_signed_narinfo(view, narinfo).await?));
            }
        }

        Ok(None)
    }

    pub async fn get_object(
        &self,
        view: &CacheView,
        object_path: &str,
    ) -> Result<Option<(BlobMetadata, BlobBytes)>> {
        if let Some(result) = self.object_provider.get_object(view, object_path).await? {
            return Ok(Some(result));
        }

        for upstream in &self.upstream_selector.upstreams_for_view(view).await? {
            if let Some(result) = self
                .upstream_client
                .get_object(upstream, object_path)
                .await?
            {
                return Ok(Some(result));
            }
        }

        Ok(None)
    }

    async fn render_signed_narinfo(&self, view: &CacheView, narinfo: NarInfo) -> Result<String> {
        let signing_keys = self.signing_keys_for_view(view).await?;
        let signer = NarInfoSigner::new(self.renderer.store_dir().clone(), signing_keys);
        let local_signatures = signer.sign(&narinfo)?;

        let mut signatures = narinfo.signatures.clone();
        signatures.extend(local_signatures);

        Ok(self
            .renderer
            .render_with_signatures(&narinfo, &signatures)?)
    }

    async fn signing_keys_for_view(&self, view: &CacheView) -> Result<Vec<NamedSigningKey>> {
        match view {
            CacheView::Aggregate => Ok(self.aggregate_signing_keys()),
            CacheView::Project(project) => self.project_signing_keys_for_project(project).await,
        }
    }

    fn aggregate_signing_keys(&self) -> Vec<NamedSigningKey> {
        self.aggregate_signing_key.iter().cloned().collect()
    }

    fn aggregate_public_keys(&self) -> Vec<String> {
        self.aggregate_signing_key
            .as_ref()
            .map(|key| vec![key.public_key_text()])
            .unwrap_or_default()
    }

    async fn project_signing_keys_for_project(
        &self,
        project: &ProjectSlug,
    ) -> Result<Vec<NamedSigningKey>> {
        let mut keys = Vec::new();

        if let Some(project_signing_keys) = &self.project_signing_keys
            && let Some(project_key) = project_signing_keys.signing_key(project).await?
        {
            keys.push(project_key);
        }

        if let Some(aggregate_key) = &self.aggregate_signing_key {
            keys.push(aggregate_key.clone());
        }

        Ok(keys)
    }

    async fn project_public_keys(&self, project: &ProjectSlug) -> Result<Vec<String>> {
        let mut keys = Vec::new();

        if let Some(project_signing_keys) = &self.project_signing_keys
            && let Some(public_key) = project_signing_keys.public_key(project).await?
        {
            keys.push(public_key);
        }

        if let Some(aggregate_key) = &self.aggregate_signing_key {
            keys.push(aggregate_key.public_key_text());
        }

        Ok(keys)
    }
}
