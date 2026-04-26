use serde::{Deserialize, Serialize};

use cache_core::narinfo::NarInfo;
use cache_core::nix::{NixContentAddress, NixHash};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeginBuildRequest {
    pub project: String,
    pub ref_name: String,
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeginBuildResponse {
    pub build_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarInfoPayload {
    pub store_path: String,
    pub url: String,
    pub compression: String,
    pub nar_hash: String,
    pub nar_size: u64,
    pub references: Vec<String>,
    pub deriver: Option<String>,
    pub signatures: Vec<String>,
    pub ca: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterPathsRequest {
    pub build_id: String,
    pub paths: Vec<NarInfoPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredUpload {
    pub store_path_hash: String,
    pub object_path: String,
    pub content_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterPathsResponse {
    pub required_uploads: Vec<RequiredUpload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalizeBuildRequest {
    pub build_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalizeBuildResponse {}
impl TryFrom<NarInfoPayload> for NarInfo {
    type Error = anyhow::Error;

    fn try_from(value: NarInfoPayload) -> Result<Self, Self::Error> {
        Ok(NarInfo {
            store_path: value.store_path,
            url: value.url,
            compression: value.compression,
            nar_hash: NixHash::Raw(value.nar_hash),
            nar_size: value.nar_size,
            references: value.references,
            deriver: value.deriver,
            signatures: value.signatures,
            ca: value.ca.map(NixContentAddress::Raw),
        })
    }
}

impl From<&NarInfo> for NarInfoPayload {
    fn from(value: &NarInfo) -> Self {
        Self {
            store_path: value.store_path.clone(),
            url: value.url.clone(),
            compression: value.compression.clone(),
            nar_hash: value.nar_hash.render_text(),
            nar_size: value.nar_size,
            references: value.references.clone(),
            deriver: value.deriver.clone(),
            signatures: value.signatures.clone(),
            ca: value.ca.as_ref().map(NixContentAddress::format_for_narinfo),
        }
    }
}
