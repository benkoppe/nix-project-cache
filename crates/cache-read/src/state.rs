use std::sync::Arc;

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

    pub fn nix_cache_info_text(&self) -> String {
        format!(
            "StoreDir: {}\nWantMassQuery: 1\nPriority: {}\n",
            self.read_service.store_dir(),
            self.priority
        )
    }
}
