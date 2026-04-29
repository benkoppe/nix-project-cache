use std::collections::BTreeMap;
use std::sync::Arc;

use tempfile::TempDir;

use depot_core::storage::StorageId;
use depot_store::{DepotStorage, FilesystemStorage, StorageCatalog};

pub fn filesystem_storage_in(temp_dir: &TempDir) -> StorageCatalog {
    let storage_id = StorageId::main();
    let storage: Arc<dyn DepotStorage> =
        Arc::new(FilesystemStorage::new(temp_dir.path().join("objects")));

    StorageCatalog::new(storage_id.clone(), BTreeMap::from([(storage_id, storage)])).unwrap()
}
