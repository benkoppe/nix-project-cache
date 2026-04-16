use std::collections::BTreeMap;

use base64::Engine as _;
use serde::{Deserialize, Deserializer};
use thiserror::Error;

const NIX_BASE32_ALPHABET: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";

#[derive(Debug, Error)]
pub enum NixHashError {
    #[error("unsupported hash format: {0} (expected sha256-... or sha256:...)")]
    UnsupportedHashFormat(String),
    #[error("unsupported structured hash format: {0}")]
    UnsupportedStructuredFormat(String),
    #[error("failed to decode base64 hash: {0}")]
    InvalidBase64(#[from] base64::DecodeError),
}

#[derive(Debug, Error)]
pub enum PathInfoJsonError {
    #[error(
        "failed to parse path-info json as object form ({object_error}) or array form ({array_error})"
    )]
    UnsupportedShape {
        object_error: String,
        array_error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hash {
    Raw(String),
    Structured {
        algorithm: String,
        format: Option<String>,
        hash: String,
    },
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct StructuredHash {
            algorithm: String,
            #[serde(default)]
            format: Option<String>,
            hash: String,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum HashRepr {
            Raw(String),
            Structured(StructuredHash),
        }
        match HashRepr::deserialize(deserializer)? {
            HashRepr::Raw(raw) => Ok(Self::Raw(raw)),
            HashRepr::Structured(value) => Ok(Self::Structured {
                algorithm: value.algorithm,
                format: value.format,
                hash: value.hash,
            }),
        }
    }
}

impl Hash {
    pub fn algorithm(&self) -> Option<&str> {
        match self {
            Self::Raw(raw) => {
                if let Some((algorithm, _)) = raw.split_once(':') {
                    Some(algorithm)
                } else if let Some((algorithm, _)) = raw.split_once('-') {
                    Some(algorithm)
                } else {
                    None
                }
            }
            Self::Structured { algorithm, .. } => Some(algorithm.as_str()),
        }
    }

    pub fn to_input_string(&self) -> String {
        match self {
            Self::Raw(raw) => raw.clone(),
            Self::Structured {
                algorithm,
                format,
                hash,
            } => match format.as_deref() {
                Some("nix32") | Some("base32") => format!("{algorithm}:{hash}"),
                _ => format!("{algorithm}-{hash}"),
            },
        }
    }

    pub fn to_nix32_string(&self) -> Result<String, NixHashError> {
        match self {
            Self::Raw(raw) => convert_hash_to_nix32(raw),
            Self::Structured {
                algorithm,
                format,
                hash,
            } => match format.as_deref() {
                Some("nix32") | Some("base32") => Ok(format!("{algorithm}:{hash}")),
                Some("base64") | Some("sri") | None => {
                    convert_hash_to_nix32(&format!("{algorithm}-{hash}"))
                }
                Some(other) => Err(NixHashError::UnsupportedStructuredFormat(other.to_owned())),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentAddress {
    Raw(String),
    Structured { method: String, hash: Hash },
}

impl<'de> Deserialize<'de> for ContentAddress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct StructuredContentAddress {
            method: String,
            hash: Hash,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ContentAddressRepr {
            Raw(String),
            Structured(StructuredContentAddress),
        }
        match ContentAddressRepr::deserialize(deserializer)? {
            ContentAddressRepr::Raw(raw) => Ok(Self::Raw(raw)),
            ContentAddressRepr::Structured(value) => Ok(Self::Structured {
                method: value.method,
                hash: value.hash,
            }),
        }
    }
}

impl ContentAddress {
    pub fn to_narinfo_string(&self) -> Result<String, NixHashError> {
        match self {
            Self::Raw(raw) => Ok(raw.clone()),
            Self::Structured { method, hash } => {
                let nix32_hash = hash.to_nix32_string()?;
                let prefix = match method.as_str() {
                    "text" => "text:",
                    "flat" => "fixed:",
                    "nar" => "fixed:r:",
                    "git" => "fixed:git:",
                    other => {
                        return Ok(format!("{other}:{nix32_hash}"));
                    }
                };

                Ok(format!("{prefix}{nix32_hash}"))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathInfo {
    pub path: String,
    pub nar_hash: Hash,
    pub nar_size: u64,
    pub references: Vec<String>,
    pub deriver: Option<String>,
    pub signatures: Vec<String>,
    pub ca: Option<ContentAddress>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct PathInfoPayload {
    #[serde(rename = "narHash")]
    nar_hash: Hash,
    #[serde(rename = "narSize")]
    nar_size: u64,
    #[serde(default)]
    references: Vec<String>,
    #[serde(default)]
    deriver: Option<String>,
    #[serde(default)]
    signatures: Vec<String>,
    #[serde(default)]
    ca: Option<ContentAddress>,
}

impl PathInfoPayload {
    fn with_path(self, path: String) -> PathInfo {
        PathInfo {
            path,
            nar_hash: self.nar_hash,
            nar_size: self.nar_size,
            references: self.references,
            deriver: self.deriver,
            signatures: self.signatures,
            ca: self.ca,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ArrayPathInfoPayload {
    path: String,
    #[serde(flatten)]
    payload: PathInfoPayload,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RealisationInfo {
    pub id: String,
    #[serde(rename = "outPath")]
    pub out_path: String,
    #[serde(default)]
    pub signatures: Vec<String>,
    #[serde(rename = "dependentRealisations", default)]
    pub dependent_realisations: BTreeMap<String, String>,
}

pub fn parse_path_info_json(input: &[u8]) -> Result<BTreeMap<String, PathInfo>, PathInfoJsonError> {
    match serde_json::from_slice::<BTreeMap<String, PathInfoPayload>>(input) {
        Ok(object_form) => {
            let mut result = BTreeMap::new();
            for (path, payload) in object_form {
                result.insert(path.clone(), payload.with_path(path));
            }
            Ok(result)
        }
        Err(object_error) => match serde_json::from_slice::<Vec<ArrayPathInfoPayload>>(input) {
            Ok(array_form) => {
                let mut result = BTreeMap::new();
                for entry in array_form {
                    result.insert(entry.path.clone(), entry.payload.with_path(entry.path));
                }
                Ok(result)
            }
            Err(array_error) => Err(PathInfoJsonError::UnsupportedShape {
                object_error: object_error.to_string(),
                array_error: array_error.to_string(),
            }),
        },
    }
}

pub fn encode_nix_base32(input: &[u8]) -> String {
    if input.is_empty() {
        return String::new();
    }

    let length = (input.len() * 8 - 1) / 5 + 1;
    let mut result = String::with_capacity(length);

    for n in (0..length).rev() {
        let bit_offset = n * 5;
        let i = bit_offset / 8;
        let j = bit_offset % 8;

        let mut c = 0u8;
        if i < input.len() {
            c = input[i] >> j;
        }
        if i + 1 < input.len() {
            c |= input[i + 1] << (8 - j);
        }

        result.push(NIX_BASE32_ALPHABET[(c & 0x1f) as usize] as char);
    }

    result
}

pub fn convert_hash_to_nix32(hash: &str) -> Result<String, NixHashError> {
    if hash.starts_with("sha256:")
        && !hash.contains('-')
        && !hash.contains('+')
        && !hash.contains('/')
        && !hash.contains('=')
    {
        return Ok(hash.to_owned());
    }

    let base64_hash = hash
        .strip_prefix("sha256-")
        .ok_or_else(|| NixHashError::UnsupportedHashFormat(hash.to_owned()))?;

    let hash_bytes = base64::engine::general_purpose::STANDARD.decode(base64_hash)?;
    Ok(format!("sha256:{}", encode_nix_base32(&hash_bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encode_nix_base32_matches_go_test_vector() {
        let input = hex::decode("9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08")
            .unwrap();
        let result = encode_nix_base32(&input);
        assert_eq!(
            result,
            "020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz"
        );
    }
    #[test]
    fn convert_hash_to_nix32_accepts_sri() {
        let result =
            convert_hash_to_nix32("sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=").unwrap();
        assert_eq!(
            result,
            "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz"
        );
    }
    #[test]
    fn convert_hash_to_nix32_accepts_existing_nix32() {
        let input = "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz";
        let result = convert_hash_to_nix32(input).unwrap();
        assert_eq!(result, input);
    }
    #[test]
    fn content_address_formats_nar_method() {
        let ca = ContentAddress::Structured {
            method: "nar".to_owned(),
            hash: Hash::Raw("sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=".to_owned()),
        };
        let result = ca.to_narinfo_string().unwrap();
        assert_eq!(
            result,
            "fixed:r:sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz"
        );
    }
    #[test]
    fn parse_path_info_json_accepts_object_form() {
        let json = br#"{
            "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello": {
                "narHash": "sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=",
                "narSize": 123,
                "references": ["/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-dep"],
                "signatures": [],
                "ca": {
                    "method": "nar",
                    "hash": {
                        "algorithm": "sha256",
                        "format": "base64",
                        "hash": "n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg="
                    }
                }
            }
        }"#;
        let result = parse_path_info_json(json).unwrap();
        let info = result
            .get("/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello")
            .unwrap();
        assert_eq!(
            info.path,
            "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello"
        );
        assert_eq!(info.nar_size, 123);
        assert_eq!(info.references.len(), 1);
        assert!(info.ca.is_some());
    }
    #[test]
    fn parse_path_info_json_accepts_array_form() {
        let json = br#"[
            {
                "path": "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello",
                "narHash": {
                    "algorithm": "sha256",
                    "format": "base64",
                    "hash": "n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg="
                },
                "narSize": 123,
                "references": [],
                "signatures": []
            }
        ]"#;
        let result = parse_path_info_json(json).unwrap();
        let info = result
            .get("/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello")
            .unwrap();
        assert_eq!(
            info.path,
            "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello"
        );
        assert_eq!(info.nar_hash.algorithm(), Some("sha256"));
    }
}
