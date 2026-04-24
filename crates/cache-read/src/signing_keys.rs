use anyhow::Result;

use cache_core::key_crypto::KeyEncryptionKey;
use cache_core::project::ProjectSlug;
use cache_core::signing::NamedSigningKey;
use cache_db::SqliteDatabase;

#[derive(Clone)]
pub struct DbProjectSigningKeys {
    db: SqliteDatabase,
    key_encryption_key: KeyEncryptionKey,
}

impl DbProjectSigningKeys {
    pub fn new(db: SqliteDatabase, key_encryption_key: KeyEncryptionKey) -> Self {
        Self {
            db,
            key_encryption_key,
        }
    }

    pub async fn public_key(&self, project: &ProjectSlug) -> Result<Option<String>> {
        self.db.active_project_signing_key_public(project).await
    }

    pub async fn signing_key(&self, project: &ProjectSlug) -> Result<Option<NamedSigningKey>> {
        self.db
            .active_project_signing_key(project, &self.key_encryption_key)
            .await
    }
}
