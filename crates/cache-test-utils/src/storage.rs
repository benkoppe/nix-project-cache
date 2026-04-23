use std::sync::Arc;

use tempfile::TempDir;

use cache_core::storage::LocalBackendName;
use cache_store::local::{FilesystemLocalObjectBackend, LocalObjectBackendRegistry};

pub fn filesystem_backends_in(temp_dir: &TempDir) -> LocalObjectBackendRegistry {
    let mut backends = LocalObjectBackendRegistry::new();
    backends.register(
        LocalBackendName::fs(),
        Arc::new(FilesystemLocalObjectBackend::new(
            temp_dir.path().join("objects"),
        )),
    );
    backends
}
