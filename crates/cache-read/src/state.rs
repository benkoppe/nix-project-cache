use std::sync::Arc;

use cache_core::narinfo::NarInfoRenderer;
use cache_core::signing::NarInfoSigner;

use crate::resolver::NarInfoResolver;

#[derive(Clone)]
pub struct AppState {
    pub resolver: Arc<dyn NarInfoResolver>,
    pub renderer: NarInfoRenderer,
    pub signer: NarInfoSigner,
    pub priority: u32,
}

impl AppState {
    pub fn new(
        resolver: Arc<dyn NarInfoResolver>,
        renderer: NarInfoRenderer,
        signer: NarInfoSigner,
        priority: u32,
    ) -> Self {
        Self {
            resolver,
            renderer,
            signer,
            priority,
        }
    }

    pub fn nix_cache_info_text(&self) -> String {
        format!(
            "StoreDir: {}\nWantMassQuery: 1\nPriority: {}\n",
            self.renderer.store_dir(),
            self.priority
        )
    }
}
