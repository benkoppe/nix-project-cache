use anyhow::{Context as _, Result, bail};
use reqwest::Url;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct GitHubOidcTokenResponse {
    value: String,
}

pub async fn request_github_actions_oidc_token(audience: &str) -> Result<String> {
    let request_url = std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL")
        .context("CACHE_WRITE_TOKEN is unset and ACTIONS_ID_TOKEN_REQUEST_URL is unavailable")?;
    let request_token = std::env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
        .context("CACHE_WRITE_TOKEN is unset and ACTIONS_ID_TOKEN_REQUEST_TOKEN is unavailable")?;

    let mut url = Url::parse(&request_url)
        .with_context(|| format!("parsing ACTIONS_ID_TOKEN_REQUEST_URL {request_url:?}"))?;
    url.query_pairs_mut().append_pair("audience", audience);

    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(request_token)
        .send()
        .await
        .context("requesting GitHub Actions OIDC token")?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("reading GitHub Actions OIDC token response")?;

    if !status.is_success() {
        bail!("GitHub Actions OIDC token request failed with {status}: {body}");
    }

    let token = serde_json::from_str::<GitHubOidcTokenResponse>(&body)
        .context("parsing GitHub Actions OIDC token response")?
        .value;

    if token.trim().is_empty() {
        bail!("GitHub Actions OIDC token response did not contain a token");
    }

    Ok(token)
}
