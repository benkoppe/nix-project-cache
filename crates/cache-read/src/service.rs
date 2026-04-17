use std::sync::Arc;

use anyhow::Result;

use cache_core::narinfo::{NarInfo, NarInfoRenderer, parse_narinfo};
use cache_core::nix::{StoreDir, StorePathHash};
use cache_core::signing::NarInfoSigner;
use cache_core::view::CacheView;
use cache_store::blob::{BlobBytes, BlobMetadata};
use cache_store::local::LocalObjectStore;
use cache_store::upstream::{UpstreamCache, UpstreamCacheClient};

use crate::resolver::NarInfoResolver;

#[derive(Clone)]
pub struct ReadService {
    local_resolver: Arc<dyn NarInfoResolver>,
    local_objects: Arc<dyn LocalObjectStore>,
    upstream_client: Arc<dyn UpstreamCacheClient>,
    upstreams: Vec<UpstreamCache>,
    renderer: NarInfoRenderer,
    signer: NarInfoSigner,
}

impl ReadService {
    pub fn new(
        local_resolver: Arc<dyn NarInfoResolver>,
        local_objects: Arc<dyn LocalObjectStore>,
        upstream_client: Arc<dyn UpstreamCacheClient>,
        mut upstreams: Vec<UpstreamCache>,
        renderer: NarInfoRenderer,
        signer: NarInfoSigner,
    ) -> Self {
        upstreams.sort_by_key(|upstream| upstream.priority);

        Self {
            local_resolver,
            local_objects,
            upstream_client,
            upstreams,
            renderer,
            signer,
        }
    }

    pub fn store_dir(&self) -> &StoreDir {
        self.renderer.store_dir()
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

        for upstream in &self.upstreams {
            if let Some(narinfo_text) = self
                .upstream_client
                .fetch_narinfo_text(upstream, store_path_hash)
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
        _view: &CacheView,
        object_path: &str,
    ) -> Result<Option<(BlobMetadata, BlobBytes)>> {
        if let Some(result) = self.local_objects.get(object_path).await? {
            return Ok(Some(result));
        }

        for upstream in &self.upstreams {
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
