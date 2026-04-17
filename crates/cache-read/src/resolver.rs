use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

use cache_core::narinfo::NarInfo;
use cache_core::nix::StorePathHash;
use cache_core::project::ProjectSlug;
use cache_core::view::CacheView;

#[async_trait]
pub trait NarInfoResolver: Send + Sync + 'static {
    async fn resolve_narinfo(
        &self,
        view: &CacheView,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<NarInfo>>;
}
