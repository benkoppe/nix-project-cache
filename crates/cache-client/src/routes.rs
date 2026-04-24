use reqwest::Url;

use cache_core::nix::StorePathHash;
use cache_core::project::ProjectSlug;

use crate::error::CacheClientError;

pub fn begin_build(base_url: &Url) -> Result<Url, CacheClientError> {
    join(base_url, "api/builds")
}

pub fn register_paths(base_url: &Url, build_id: &str) -> Result<Url, CacheClientError> {
    join(base_url, &format!("api/builds/{build_id}/paths"))
}

pub fn finalize_build(base_url: &Url, build_id: &str) -> Result<Url, CacheClientError> {
    join(base_url, &format!("api/builds/{build_id}/finalize"))
}

pub fn upload_object(
    base_url: &Url,
    build_id: &str,
    store_path_hash: &StorePathHash,
    object_path: &str,
) -> Result<Url, CacheClientError> {
    let prefix = format!(
        "api/builds/{build_id}/paths/{}/objects/",
        store_path_hash.as_str()
    );

    join(base_url, &format!("{prefix}{object_path}"))
}

pub fn list_pins(base_url: &Url, project: Option<&ProjectSlug>) -> Result<Url, CacheClientError> {
    let mut url = join(base_url, "api/pins")?;
    if let Some(project) = project {
        url.query_pairs_mut()
            .append_pair("project", project.as_str());
    }
    Ok(url)
}

pub fn create_pin(base_url: &Url, name: &str) -> Result<Url, CacheClientError> {
    join(base_url, &format!("api/pins/{name}"))
}

pub fn delete_pin(
    base_url: &Url,
    name: &str,
    project: Option<&ProjectSlug>,
) -> Result<Url, CacheClientError> {
    let mut url = join(base_url, &format!("api/pins/{name}"))?;
    if let Some(project) = project {
        url.query_pairs_mut()
            .append_pair("project", project.as_str());
    }
    Ok(url)
}

pub fn run_gc(base_url: &Url) -> Result<Url, CacheClientError> {
    join(base_url, "api/gc")
}

pub fn list_projects(base_url: &Url) -> Result<Url, CacheClientError> {
    join(base_url, "api/projects")
}

pub fn upsert_project(base_url: &Url) -> Result<Url, CacheClientError> {
    join(base_url, "api/projects")
}

pub fn project_oidc_identities(
    base_url: &Url,
    project: &ProjectSlug,
) -> Result<Url, CacheClientError> {
    join(
        base_url,
        &format!("api/projects/{}/oidc-identities", project.as_str()),
    )
}

pub fn project_retention(base_url: &Url, project: &ProjectSlug) -> Result<Url, CacheClientError> {
    join(
        base_url,
        &format!("api/projects/{}/retention", project.as_str()),
    )
}

pub fn project_signing_key(base_url: &Url, project: &ProjectSlug) -> Result<Url, CacheClientError> {
    join(
        base_url,
        &format!("api/projects/{}/signing-key", project.as_str()),
    )
}

pub fn generate_project_signing_key(
    base_url: &Url,
    project: &ProjectSlug,
) -> Result<Url, CacheClientError> {
    join(
        base_url,
        &format!("api/projects/{}/signing-key/generate", project.as_str()),
    )
}

pub fn import_project_signing_key(
    base_url: &Url,
    project: &ProjectSlug,
) -> Result<Url, CacheClientError> {
    join(
        base_url,
        &format!("api/projects/{}/signing-key/import", project.as_str()),
    )
}

pub fn upstreams(base_url: &Url) -> Result<Url, CacheClientError> {
    join(base_url, "api/upstreams")
}

pub fn upstream_enabled(
    base_url: &Url,
    upstream: &str,
    enabled: bool,
) -> Result<Url, CacheClientError> {
    let action = if enabled { "enable" } else { "disable" };
    join(base_url, &format!("api/upstreams/{upstream}/{action}"))
}

pub fn project_upstreams(base_url: &Url, project: &ProjectSlug) -> Result<Url, CacheClientError> {
    join(
        base_url,
        &format!("api/projects/{}/upstreams", project.as_str()),
    )
}

pub fn project_upstream(
    base_url: &Url,
    project: &ProjectSlug,
    upstream: &str,
) -> Result<Url, CacheClientError> {
    join(
        base_url,
        &format!("api/projects/{}/upstreams/{upstream}", project.as_str()),
    )
}

pub fn access_tokens(
    base_url: &Url,
    project: Option<&ProjectSlug>,
) -> Result<Url, CacheClientError> {
    let mut url = join(base_url, "api/access-tokens")?;
    if let Some(project) = project {
        url.query_pairs_mut()
            .append_pair("project", project.as_str());
    }
    Ok(url)
}

pub fn revoke_access_token(base_url: &Url, token_id: &str) -> Result<Url, CacheClientError> {
    join(base_url, &format!("api/access-tokens/{token_id}"))
}

fn join(base_url: &Url, path: &str) -> Result<Url, CacheClientError> {
    base_url
        .join(path)
        .map_err(|error| CacheClientError::InvalidEndpointUrl {
            message: format!("base={} path={} error={}", base_url.as_str(), path, error),
        })
}
