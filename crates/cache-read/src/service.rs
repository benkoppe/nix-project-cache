use std::sync::Arc;

use anyhow::Result;

use cache_core::narinfo::{NarInfo, NarInfoRenderer, parse_narinfo};
use cache_core::nix::{StoreDir, StorePathHash};
use cache_core::signing::NarInfoSigner;
use cache_core::view::CacheView;
use cache_store::blob::{BlobBytes, BlobMetadata};
use cache_store::upstream::UpstreamCacheClient;

use crate::local_objects::ViewLocalObjectStore;
use crate::resolver::NarInfoResolver;
use crate::upstreams::UpstreamSelector;

#[derive(Clone)]
pub struct ReadService {
    local_resolver: Arc<dyn NarInfoResolver>,
    local_objects: Arc<dyn ViewLocalObjectStore>,
    upstream_client: Arc<dyn UpstreamCacheClient>,
    upstream_selector: Arc<dyn UpstreamSelector>,
    renderer: NarInfoRenderer,
    signer: NarInfoSigner,
}

impl ReadService {
    pub fn new(
        local_resolver: Arc<dyn NarInfoResolver>,
        local_objects: Arc<dyn ViewLocalObjectStore>,
        upstream_client: Arc<dyn UpstreamCacheClient>,
        upstream_selector: Arc<dyn UpstreamSelector>,
        renderer: NarInfoRenderer,
        signer: NarInfoSigner,
    ) -> Self {
        Self {
            local_resolver,
            local_objects,
            upstream_client,
            upstream_selector,
            renderer,
            signer,
        }
    }

    pub fn store_dir(&self) -> &StoreDir {
        self.renderer.store_dir()
    }

    pub fn public_key_texts(&self) -> Vec<String> {
        self.signer.public_key_texts()
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
            return Ok(Some(self.render_signed_narinfo(narinfo)?));
        }

        for upstream in self.upstream_selector.upstreams_for_view(view).await? {
            if let Some(narinfo_text) = self
                .upstream_client
                .fetch_narinfo_text(&upstream, store_path_hash)
                .await?
            {
                let narinfo = parse_narinfo(&narinfo_text, self.renderer.store_dir())?;
                return Ok(Some(self.render_signed_narinfo(narinfo)?));
            }
        }

        Ok(None)
    }

    pub async fn get_object(
        &self,
        view: &CacheView,
        object_path: &str,
    ) -> Result<Option<(BlobMetadata, BlobBytes)>> {
        if let Some(result) = self.local_objects.get_visible(view, object_path).await? {
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

    fn render_signed_narinfo(&self, narinfo: NarInfo) -> Result<String> {
        let local_signatures = self.signer.sign(&narinfo)?;
        let mut signatures = narinfo.signatures.clone();
        signatures.extend(local_signatures);

        Ok(self
            .renderer
            .render_with_signatures(&narinfo, &signatures)?)
    }
}
