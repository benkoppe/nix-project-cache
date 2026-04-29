use std::sync::Arc;

use anyhow::Result;

use depot_core::view::CacheView;

use crate::service::ReadService;

#[derive(Clone)]
pub struct ReadAppState {
    pub read_service: Arc<ReadService>,
    pub priority: u32,
}

impl ReadAppState {
    pub fn new(read_service: Arc<ReadService>, priority: u32) -> Self {
        Self {
            read_service,
            priority,
        }
    }

    pub async fn nix_cache_info_text(&self, view: &CacheView) -> Result<String> {
        let mut text = format!(
            "StoreDir: {}\nWantMassQuery: 1\nPriority: {}\n",
            self.read_service.store_dir(),
            self.priority
        );

        for public_key in self.read_service.public_key_texts_for_view(view).await? {
            text.push_str("PublicKey: ");
            text.push_str(&public_key);
            text.push('\n');
        }

        Ok(text)
    }
}
