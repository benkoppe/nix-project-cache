use std::sync::Arc;

use crate::service::ReadService;

#[derive(Clone)]
pub struct ReadAppState {
    pub read_service: Arc<ReadService>,
    pub priority: u32,
    public_keys: Vec<String>,
}

impl ReadAppState {
    pub fn new(read_service: Arc<ReadService>, priority: u32) -> Self {
        let public_keys = read_service.public_key_texts();

        Self {
            read_service,
            priority,
            public_keys,
        }
    }

    pub fn nix_cache_info_text(&self) -> String {
        let mut text = format!(
            "StoreDir: {}\nWantMassQuery: 1\nPriority: {}\n",
            self.read_service.store_dir(),
            self.priority
        );

        for public_key in &self.public_keys {
            text.push_str("PublicKey: ");
            text.push_str(public_key);
            text.push('\n');
        }

        text
    }
}
