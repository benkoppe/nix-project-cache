use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::info;

use cache_api::BeginBuildRequest;
use cache_client::CacheClient;
use cache_core::nix::StorePathHash;
use cache_core::project::ProjectSlug;

use crate::nix;

#[derive(Debug, Clone)]
pub struct PushOptions {
    pub project: ProjectSlug,
    pub ref_name: String,
    pub revision: String,
    pub pin: Option<String>,
    pub max_concurrent_uploads: usize,
    pub paths: Vec<String>,
}

pub async fn push_paths(client: &CacheClient, options: PushOptions) -> Result<()> {
    let store_dir = nix::get_store_dir().await?;
    let resolved_paths = nix::resolve_symlinks(&options.paths, &store_dir).await?;
    let path_infos = nix::get_path_infos_recursive(&resolved_paths).await?;
    let narinfos = nix::narinfos_from_path_infos(&path_infos)?;

    let mut path_by_hash = HashMap::with_capacity(path_infos.len());
    for path_info in path_infos.values() {
        let hash = path_info
            .store_path_hash()
            .with_context(|| format!("deriving store path hash for {}", path_info.path))?;
        path_by_hash.insert(hash.as_str().to_owned(), path_info.path.clone());
    }

    info!(
        top_level_paths = resolved_paths.len(),
        closure_paths = narinfos.len(),
        "registering build paths"
    );

    let build = client
        .begin_build(BeginBuildRequest {
            project: options.project.as_str().to_owned(),
            ref_name: options.ref_name.clone(),
            revision: Some(options.revision.clone()),
        })
        .await
        .context("beginning build")?;

    let register = client
        .register_paths(&build.build_id, narinfos)
        .await
        .context("registering build paths")?;

    info!(
        build_id = %build.build_id,
        required_uploads = register.required_uploads.len(),
        "upload plan ready"
    );

    let semaphore = Arc::new(Semaphore::new(options.max_concurrent_uploads.max(1)));
    let mut uploads = JoinSet::new();

    for required_upload in register.required_uploads {
        let build_id = build.build_id.clone();
        let client = client.clone();
        let semaphore = semaphore.clone();

        let store_path = path_by_hash
            .get(&required_upload.store_path_hash)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "required upload references unknown store path hash {}",
                    required_upload.store_path_hash
                )
            })?;

        uploads.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| anyhow!("upload semaphore closed"))?;

            let store_path_hash = StorePathHash::from_hash(&required_upload.store_path_hash)
                .with_context(|| {
                    format!(
                        "parsing store path hash {}",
                        required_upload.store_path_hash
                    )
                })?;

            let reader = nix::compressed_nar_reader_for_path(&store_path)
                .await
                .with_context(|| format!("streaming NAR for {}", store_path))?;

            client
                .upload_object_reader(
                    &build_id,
                    &store_path_hash,
                    &required_upload.object_path,
                    reader,
                )
                .await
                .with_context(|| {
                    format!(
                        "uploading {} for {}",
                        required_upload.object_path, store_path
                    )
                })?;

            Ok::<(), anyhow::Error>(())
        });
    }

    while let Some(result) = uploads.join_next().await {
        result.context("upload task panicked")??;
    }

    client
        .finalize_build(&build.build_id)
        .await
        .context("finalizing build")?;

    if let Some(pin_name) = options.pin.as_deref() {
        let top_level_store_path = resolved_paths
            .first()
            .ok_or_else(|| anyhow!("pin requested but no top-level paths were resolved"))?;

        client
            .create_pin(pin_name, Some(&options.project), top_level_store_path)
            .await
            .with_context(|| format!("creating pin {}", pin_name))?;
    }

    info!(
        build_id = %build.build_id,
        project = %options.project.as_str(),
        ref_name = %options.ref_name,
        "push completed"
    );

    Ok(())
}
