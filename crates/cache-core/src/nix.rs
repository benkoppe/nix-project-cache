use std::collections::BTreeMap;
use std::fmt;

use base64::Engine as _;
use serde::{Deserialize, Deserializer};
use thiserror::Error;

pub const NIX_BASE32_ALPHABET: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";

#[derive(Debug, Error)]
pub enum NixHashError {
    #[error(
        "unsupported hash text format: {0} (expected <algorithm>-<digest> or <algorithm>:<digest>)"
    )]
    UnsupportedTextFormat(String),
    #[error("unsupported hash algorithm for nix-base32 conversion: {0}")]
    UnsupportedAlgorithm(String),
    #[error("unsupported hash encoding for nix-base32 conversion: {0}")]
    UnsupportedEncoding(String),
    #[error("invalid nix-base32 digest for {algorithm}: {digest}")]
    InvalidNixBase32Digest { algorithm: String, digest: String },
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

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StorePathHashError {
    #[error("invalid store path format (missing hyphen): {0}")]
    MissingHyphen(String),
    #[error("invalid hash length {actual} (expected {expected}): {store_path}")]
    InvalidLength {
        store_path: String,
        actual: usize,
        expected: usize,
    },
    #[error("invalid character {character:?} at position {position} in hash: {store_path}")]
    InvalidCharacter {
        store_path: String,
        character: char,
        position: usize,
    },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StoreDirError {
    #[error("store directory cannot be empty")]
    Empty,
    #[error("store directory must be absolute: {0}")]
    NotAbsolute(String),
}

pub const DEFAULT_STORE_DIR: &str = "/nix/store";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoreDir(String);

impl StoreDir {
    pub fn new(path: impl Into<String>) -> Result<Self, StoreDirError> {
        let mut path = path.into();

        if path.is_empty() {
            return Err(StoreDirError::Empty);
        }

        if !path.starts_with('/') {
            return Err(StoreDirError::NotAbsolute(path));
        }

        while path.len() > 1 && path.ends_with('/') {
            path.pop();
        }

        Ok(Self(path))
    }

    pub fn default_nix() -> Self {
        Self(DEFAULT_STORE_DIR.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn contains_path(&self, path: &str) -> bool {
        if path == self.0 {
            return true;
        }

        path.strip_prefix(&self.0)
            .is_some_and(|suffix| suffix.starts_with('/'))
    }

    pub fn relative_path<'a>(&self, path: &'a str) -> Option<&'a str> {
        if path == self.0 {
            return Some("");
        }

        path.strip_prefix(&self.0)
            .and_then(|suffix| suffix.strip_prefix('/'))
    }

    pub fn display_path<'a>(&self, path: &'a str) -> &'a str {
        self.relative_path(path).unwrap_or(path)
    }
}

impl Default for StoreDir {
    fn default() -> Self {
        Self::default_nix()
    }
}

impl fmt::Display for StoreDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorePathHash(String);

impl StorePathHash {
    pub const LENGTH: usize = 32;

    pub fn parse_from_store_path(store_path: &str) -> Result<Self, StorePathHashError> {
        let base = store_path.rsplit('/').next().unwrap_or(store_path);

        let (hash, _) = base
            .split_once('-')
            .ok_or_else(|| StorePathHashError::MissingHyphen(store_path.to_owned()))?;

        if hash.len() != Self::LENGTH {
            return Err(StorePathHashError::InvalidLength {
                store_path: store_path.to_owned(),
                actual: hash.len(),
                expected: Self::LENGTH,
            });
        }

        for (position, character) in hash.chars().enumerate() {
            if !NIX_BASE32_ALPHABET.contains(&(character as u8)) {
                return Err(StorePathHashError::InvalidCharacter {
                    store_path: store_path.to_owned(),
                    character,
                    position,
                });
            }
        }

        Ok(Self(hash.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for StorePathHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashAlgorithm {
    Sha256,
    Sha512,
    Other(String),
}

impl HashAlgorithm {
    pub fn parse(value: &str) -> Self {
        match value {
            "sha256" => Self::Sha256,
            "sha512" => Self::Sha512,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
            Self::Other(other) => other,
        }
    }

    pub fn supports_nix_base32_conversion(&self) -> bool {
        matches!(self, Self::Sha256)
    }

    pub fn expected_nix_base32_len(&self) -> Option<usize> {
        match self {
            Self::Sha256 => Some(52),
            Self::Sha512 | Self::Other(_) => None,
        }
    }
}

impl fmt::Display for HashAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashTextEncoding {
    Base64,
    NixBase32,
    Other(String),
}

impl HashTextEncoding {
    fn parse(value: &str) -> Self {
        match value {
            "base64" => Self::Base64,
            "nix32" | "base32" => Self::NixBase32,
            other => Self::Other(other.to_owned()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashTextSyntax {
    Sri,
    ColonSeparated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHashText<'a> {
    pub algorithm: HashAlgorithm,
    pub form: HashTextSyntax,
    pub digest: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NixHash {
    Raw(String),
    Structured {
        algorithm: HashAlgorithm,
        encoding: Option<HashTextEncoding>,
        digest: String,
    },
}

impl<'de> Deserialize<'de> for NixHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct StructuredHash {
            algorithm: String,
            #[serde(default, rename = "format")]
            format_name: Option<String>,
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
                algorithm: HashAlgorithm::parse(&value.algorithm),
                encoding: value.format_name.as_deref().map(HashTextEncoding::parse),
                digest: value.hash,
            }),
        }
    }
}

impl NixHash {
    pub fn algorithm(&self) -> Option<HashAlgorithm> {
        match self {
            Self::Raw(raw) => parse_hash_text(raw).map(|parsed| parsed.algorithm),
            Self::Structured { algorithm, .. } => Some(algorithm.clone()),
        }
    }

    pub fn render_text(&self) -> String {
        match self {
            Self::Raw(raw) => raw.clone(),
            Self::Structured {
                algorithm,
                encoding,
                digest,
            } => match encoding {
                Some(HashTextEncoding::NixBase32) => format!("{algorithm}:{digest}"),
                _ => format!("{algorithm}-{digest}"),
            },
        }
    }

    pub fn render_nix32_text(&self) -> Result<String, NixHashError> {
        match self {
            Self::Raw(raw) => normalize_hash_text_to_nix32(raw),
            Self::Structured {
                algorithm,
                encoding: Some(HashTextEncoding::NixBase32),
                digest,
            } => {
                validate_nix32_text_digest(algorithm, digest)?;
                Ok(format!("{algorithm}:{digest}"))
            }
            Self::Structured {
                algorithm,
                encoding: Some(HashTextEncoding::Base64),
                digest,
            }
            | Self::Structured {
                algorithm,
                encoding: None,
                digest,
            } => encode_base64_digest_as_nix32_text(algorithm, digest),
            Self::Structured {
                encoding: Some(HashTextEncoding::Other(name)),
                ..
            } => Err(NixHashError::UnsupportedEncoding(name.clone())),
        }
    }
}

pub fn parse_hash_text(input: &str) -> Option<ParsedHashText<'_>> {
    if let Some((algorithm, digest)) = input.split_once(':') {
        return Some(ParsedHashText {
            algorithm: HashAlgorithm::parse(algorithm),
            form: HashTextSyntax::ColonSeparated,
            digest,
        });
    }

    if let Some((algorithm, digest)) = input.split_once('-') {
        return Some(ParsedHashText {
            algorithm: HashAlgorithm::parse(algorithm),
            form: HashTextSyntax::Sri,
            digest,
        });
    }

    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentAddressMethod {
    Text,
    Flat,
    Nar,
    Git,
    Other(String),
}

impl ContentAddressMethod {
    fn parse(value: &str) -> Self {
        match value {
            "text" => Self::Text,
            "flat" => Self::Flat,
            "nar" => Self::Nar,
            "git" => Self::Git,
            other => Self::Other(other.to_owned()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NixContentAddress {
    Raw(String),
    Structured {
        method: ContentAddressMethod,
        hash: NixHash,
    },
}

impl<'de> Deserialize<'de> for NixContentAddress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct StructuredContentAddress {
            method: String,
            hash: NixHash,
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
                method: ContentAddressMethod::parse(&value.method),
                hash: value.hash,
            }),
        }
    }
}

impl NixContentAddress {
    pub fn format_for_narinfo(&self) -> String {
        match self {
            Self::Raw(raw) => raw.clone(),
            Self::Structured { method, hash } => {
                let hash_text = hash.render_text();
                let normalized_hash = hash.render_nix32_text().unwrap_or(hash_text);

                match method {
                    ContentAddressMethod::Text => format!("text:{normalized_hash}"),
                    ContentAddressMethod::Flat => format!("fixed:{normalized_hash}"),
                    ContentAddressMethod::Nar => format!("fixed:r:{normalized_hash}"),
                    ContentAddressMethod::Git => format!("fixed:git:{normalized_hash}"),
                    ContentAddressMethod::Other(other) => format!("{other}:{normalized_hash}"),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathInfo {
    pub path: String,
    pub nar_hash: NixHash,
    pub nar_size: u64,
    pub references: Vec<String>,
    pub deriver: Option<String>,
    pub signatures: Vec<String>,
    pub ca: Option<NixContentAddress>,
}

impl PathInfo {
    pub fn store_path_hash(&self) -> Result<StorePathHash, StorePathHashError> {
        StorePathHash::parse_from_store_path(&self.path)
    }

    pub fn reference_hashes(&self) -> Result<Vec<StorePathHash>, StorePathHashError> {
        self.references
            .iter()
            .map(|reference| StorePathHash::parse_from_store_path(reference))
            .collect()
    }
}

pub fn parse_store_path_hash(store_path: &str) -> Result<StorePathHash, StorePathHashError> {
    StorePathHash::parse_from_store_path(store_path)
}

#[derive(Debug, Clone, serde::Deserialize)]
struct PathInfoPayload {
    #[serde(rename = "narHash")]
    nar_hash: NixHash,
    #[serde(rename = "narSize")]
    nar_size: u64,
    #[serde(default)]
    references: Vec<String>,
    #[serde(default)]
    deriver: Option<String>,
    #[serde(default)]
    signatures: Vec<String>,
    #[serde(default)]
    ca: Option<NixContentAddress>,
}

impl PathInfoPayload {
    fn into_path_info(self, path: String) -> PathInfo {
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
                result.insert(path.clone(), payload.into_path_info(path));
            }
            Ok(result)
        }
        Err(object_error) => match serde_json::from_slice::<Vec<ArrayPathInfoPayload>>(input) {
            Ok(array_form) => {
                let mut result = BTreeMap::new();
                for entry in array_form {
                    result.insert(entry.path.clone(), entry.payload.into_path_info(entry.path));
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

fn is_valid_nix_base32_alphabet(digest: &str) -> bool {
    !digest.is_empty()
        && digest
            .bytes()
            .all(|byte| NIX_BASE32_ALPHABET.contains(&byte))
}

fn validate_nix32_text_digest(algorithm: &HashAlgorithm, digest: &str) -> Result<(), NixHashError> {
    if !algorithm.supports_nix_base32_conversion() {
        return Err(NixHashError::UnsupportedAlgorithm(algorithm.to_string()));
    }
    if !is_valid_nix_base32_alphabet(digest) {
        return Err(NixHashError::InvalidNixBase32Digest {
            algorithm: algorithm.to_string(),
            digest: digest.to_owned(),
        });
    }
    if let Some(expected_len) = algorithm.expected_nix_base32_len()
        && digest.len() != expected_len
    {
        return Err(NixHashError::InvalidNixBase32Digest {
            algorithm: algorithm.to_string(),
            digest: digest.to_owned(),
        });
    }
    Ok(())
}

fn encode_base64_digest_as_nix32_text(
    algorithm: &HashAlgorithm,
    digest: &str,
) -> Result<String, NixHashError> {
    if !algorithm.supports_nix_base32_conversion() {
        return Err(NixHashError::UnsupportedAlgorithm(algorithm.to_string()));
    }

    let hash_bytes = base64::engine::general_purpose::STANDARD.decode(digest)?;
    Ok(format!("{algorithm}:{}", encode_nix_base32(&hash_bytes)))
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
        if i + 1 < input.len() && j != 0 {
            c |= input[i + 1] << (8 - j);
        }

        result.push(NIX_BASE32_ALPHABET[(c & 0x1f) as usize] as char);
    }

    result
}

pub fn normalize_hash_text_to_nix32(hash: &str) -> Result<String, NixHashError> {
    let parsed = parse_hash_text(hash)
        .ok_or_else(|| NixHashError::UnsupportedTextFormat(hash.to_owned()))?;

    if !parsed.algorithm.supports_nix_base32_conversion() {
        return Err(NixHashError::UnsupportedAlgorithm(
            parsed.algorithm.to_string(),
        ));
    }

    match parsed.form {
        HashTextSyntax::Sri => encode_base64_digest_as_nix32_text(&parsed.algorithm, parsed.digest),
        HashTextSyntax::ColonSeparated => {
            validate_nix32_text_digest(&parsed.algorithm, parsed.digest)?;
            Ok(format!("{}:{}", parsed.algorithm, parsed.digest))
        }
    }
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
    fn convert_text_hash_to_nix_base32_accepts_sri() {
        let result =
            normalize_hash_text_to_nix32("sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=")
                .unwrap();

        assert_eq!(
            result,
            "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz"
        );
    }

    #[test]
    fn convert_text_hash_to_nix_base32_accepts_existing_nix32() {
        let input = "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz";
        let result = normalize_hash_text_to_nix32(input).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn parse_text_hash_accepts_colon_form_even_if_not_nix_base32() {
        let parsed =
            parse_hash_text("sha256:FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc=").unwrap();

        assert_eq!(parsed.algorithm, HashAlgorithm::Sha256);
        assert_eq!(parsed.form, HashTextSyntax::ColonSeparated);
        assert_eq!(
            parsed.digest,
            "FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc="
        );
    }

    #[test]
    fn nix_hash_raw_sha256_sri_round_trips() {
        let hash: NixHash =
            serde_json::from_str(r#""sha256-FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc=""#)
                .unwrap();

        assert_eq!(
            hash.render_text(),
            "sha256-FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc="
        );
    }

    #[test]
    fn nix_hash_raw_sha256_colon_round_trips() {
        let hash: NixHash =
            serde_json::from_str(r#""sha256:FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc=""#)
                .unwrap();

        assert_eq!(
            hash.render_text(),
            "sha256:FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc="
        );
    }

    #[test]
    fn nix_hash_structured_sha256_base64_to_text() {
        let hash: NixHash = serde_json::from_str(
            r#"{"algorithm":"sha256","format":"base64","hash":"FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc="}"#,
        )
        .unwrap();

        assert_eq!(
            hash.render_text(),
            "sha256-FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc="
        );
    }

    #[test]
    fn nix_hash_structured_sha512_base64_to_text() {
        let hash: NixHash = serde_json::from_str(
            r#"{"algorithm":"sha512","format":"base64","hash":"abcdef123456"}"#,
        )
        .unwrap();

        assert_eq!(hash.render_text(), "sha512-abcdef123456");
    }

    #[test]
    fn content_address_raw_text_round_trips() {
        let ca: NixContentAddress = serde_json::from_str(r#""text:sha256:1abc2def3ghi""#).unwrap();

        assert_eq!(ca.format_for_narinfo(), "text:sha256:1abc2def3ghi");
    }

    #[test]
    fn content_address_raw_fixed_recursive_round_trips() {
        let ca: NixContentAddress = serde_json::from_str(r#""fixed:r:sha256:1abc2def""#).unwrap();

        assert_eq!(ca.format_for_narinfo(), "fixed:r:sha256:1abc2def");
    }

    #[test]
    fn content_address_structured_text_formats_like_go() {
        let ca = NixContentAddress::Structured {
            method: ContentAddressMethod::Text,
            hash: NixHash::Structured {
                algorithm: HashAlgorithm::Sha256,
                encoding: Some(HashTextEncoding::Base64),
                digest: "h1JyyIYA".to_owned(),
            },
        };

        assert_eq!(ca.format_for_narinfo(), "text:sha256:00hv474ll7");
    }

    #[test]
    fn content_address_structured_nar_formats_like_go() {
        let ca = NixContentAddress::Structured {
            method: ContentAddressMethod::Nar,
            hash: NixHash::Structured {
                algorithm: HashAlgorithm::Sha256,
                encoding: Some(HashTextEncoding::Base64),
                digest: "abcd1234".to_owned(),
            },
        };

        assert_eq!(ca.format_for_narinfo(), "fixed:r:sha256:7qdpbivdv9");
    }

    #[test]
    fn content_address_structured_unknown_method_falls_back() {
        let ca = NixContentAddress::Structured {
            method: ContentAddressMethod::Other("weird".to_owned()),
            hash: NixHash::Structured {
                algorithm: HashAlgorithm::Sha256,
                encoding: Some(HashTextEncoding::Base64),
                digest: "h1JyyIYA".to_owned(),
            },
        };

        assert_eq!(ca.format_for_narinfo(), "weird:sha256:00hv474ll7");
    }

    #[test]
    fn content_address_structured_sha512_falls_back_to_text_form() {
        let ca = NixContentAddress::Structured {
            method: ContentAddressMethod::Text,
            hash: NixHash::Structured {
                algorithm: HashAlgorithm::Sha512,
                encoding: Some(HashTextEncoding::Base64),
                digest: "abcdef".to_owned(),
            },
        };

        assert_eq!(ca.format_for_narinfo(), "text:sha512-abcdef");
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
        assert_eq!(info.nar_hash.algorithm(), Some(HashAlgorithm::Sha256));
    }

    #[test]
    fn parse_path_info_json_rejects_empty_input() {
        let result = parse_path_info_json(b"");
        assert!(result.is_err());
    }

    #[test]
    fn parse_path_info_json_rejects_whitespace_only() {
        let result = parse_path_info_json(b"   \n\t  ");
        assert!(result.is_err());
    }

    #[test]
    fn parse_path_info_json_rejects_invalid_json() {
        let result = parse_path_info_json(b"not json");
        assert!(result.is_err());
    }

    #[test]
    fn parse_path_info_json_accepts_multiple_object_entries() {
        let json = br#"{
            "/nix/store/aaaa-foo": {"narHash": "sha256-abc=", "narSize": 100, "references": []},
            "/nix/store/bbbb-bar": {"narHash": "sha256-def=", "narSize": 200, "references": []}
        }"#;

        let result = parse_path_info_json(json).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result["/nix/store/aaaa-foo"].path, "/nix/store/aaaa-foo");
        assert_eq!(result["/nix/store/aaaa-foo"].nar_size, 100);
        assert_eq!(result["/nix/store/bbbb-bar"].path, "/nix/store/bbbb-bar");
        assert_eq!(result["/nix/store/bbbb-bar"].nar_size, 200);
    }

    #[test]
    fn parse_path_info_json_accepts_multiple_array_entries() {
        let json = br#"[
            {"path": "/nix/store/aaaa-foo", "narHash": "sha256-abc=", "narSize": 100, "references": []},
            {"path": "/nix/store/bbbb-bar", "narHash": "sha256-def=", "narSize": 200, "references": []}
        ]"#;

        let result = parse_path_info_json(json).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result["/nix/store/aaaa-foo"].path, "/nix/store/aaaa-foo");
        assert_eq!(result["/nix/store/aaaa-foo"].nar_size, 100);
        assert_eq!(result["/nix/store/bbbb-bar"].path, "/nix/store/bbbb-bar");
        assert_eq!(result["/nix/store/bbbb-bar"].nar_size, 200);
    }

    #[test]
    fn parse_path_info_json_accepts_null_ca() {
        let json = br#"{
            "/nix/store/aaaa-foo": {
                "narHash": "sha256-abc=",
                "narSize": 100,
                "references": [],
                "ca": null
            }
        }"#;

        let result = parse_path_info_json(json).unwrap();

        assert!(result["/nix/store/aaaa-foo"].ca.is_none());
    }

    #[test]
    fn store_path_hash_parses_valid_store_path() {
        let hash = StorePathHash::parse_from_store_path(
            "/nix/store/8ha1dhmx807czjczmwy078s4r9s254il-hello-2.12.2",
        )
        .unwrap();

        assert_eq!(hash.as_str(), "8ha1dhmx807czjczmwy078s4r9s254il");
    }

    #[test]
    fn store_path_hash_rejects_missing_hyphen() {
        let result = StorePathHash::parse_from_store_path("/nix/store/badhash");
        assert!(matches!(result, Err(StorePathHashError::MissingHyphen(_))));
    }

    #[test]
    fn store_path_hash_rejects_invalid_characters() {
        let result = StorePathHash::parse_from_store_path(
            "/nix/store/8ha1dhmx807czjczmwy078s4r9s254ie-package",
        );
        assert!(matches!(
            result,
            Err(StorePathHashError::InvalidCharacter { .. })
        ));
    }

    #[test]
    fn store_path_hash_rejects_wrong_length() {
        let result = StorePathHash::parse_from_store_path("/nix/store/tooshort-package");
        assert!(matches!(
            result,
            Err(StorePathHashError::InvalidLength { .. })
        ));
    }

    #[test]
    fn convert_text_hash_to_nix_base32_rejects_invalid_format() {
        let result = normalize_hash_text_to_nix32("md5:abc123");
        assert!(result.is_err());
    }

    #[test]
    fn parse_path_info_json_preserves_deriver_and_signatures_in_object_form() {
        let json = br#"{
            "/nix/store/8ha1dhmx807czjczmwy078s4r9s254il-hello-2.12.2": {
                "narHash": "sha256-FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc=",
                "narSize": 226560,
                "references": [
                    "/nix/store/3n58xw4373jp0ljirf06d8077j15pc4j-glibc-2.37-8",
                    "/nix/store/8ha1dhmx807czjczmwy078s4r9s254il-hello-2.12.2"
                ],
                "deriver": "/nix/store/abc-hello.drv",
                "signatures": ["cache.nixos.org-1:sig"]
            }
        }"#;

        let result = parse_path_info_json(json).unwrap();
        let info = &result["/nix/store/8ha1dhmx807czjczmwy078s4r9s254il-hello-2.12.2"];

        assert_eq!(info.deriver.as_deref(), Some("/nix/store/abc-hello.drv"));
        assert_eq!(info.signatures, vec!["cache.nixos.org-1:sig"]);
    }

    #[test]
    fn parse_path_info_json_preserves_deriver_and_signatures_in_array_form() {
        let json = br#"[
            {
                "path": "/nix/store/8ha1dhmx807czjczmwy078s4r9s254il-hello-2.12.2",
                "narHash": "sha256-FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc=",
                "narSize": 226560,
                "references": [
                    "/nix/store/3n58xw4373jp0ljirf06d8077j15pc4j-glibc-2.37-8",
                    "/nix/store/8ha1dhmx807czjczmwy078s4r9s254il-hello-2.12.2"
                ],
                "deriver": "/nix/store/abc-hello.drv",
                "signatures": ["cache.nixos.org-1:sig"]
            }
        ]"#;

        let result = parse_path_info_json(json).unwrap();
        let info = &result["/nix/store/8ha1dhmx807czjczmwy078s4r9s254il-hello-2.12.2"];

        assert_eq!(info.deriver.as_deref(), Some("/nix/store/abc-hello.drv"));
        assert_eq!(info.signatures, vec!["cache.nixos.org-1:sig"]);
    }

    #[test]
    fn nix_hash_structured_nix32_renders_text_with_colon() {
        let hash = NixHash::Structured {
            algorithm: HashAlgorithm::Sha256,
            encoding: Some(HashTextEncoding::NixBase32),
            digest: "020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz".to_owned(),
        };

        assert_eq!(
            hash.render_text(),
            "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz"
        );
    }

    #[test]
    fn nix_hash_structured_nix32_renders_nix32_text_directly() {
        let hash = NixHash::Structured {
            algorithm: HashAlgorithm::Sha256,
            encoding: Some(HashTextEncoding::NixBase32),
            digest: "020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz".to_owned(),
        };

        assert_eq!(
            hash.render_nix32_text().unwrap(),
            "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz"
        );
    }

    #[test]
    fn normalize_hash_text_to_nix32_rejects_invalid_nix32_alphabet() {
        let result = normalize_hash_text_to_nix32(
            "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11le",
        );

        assert!(matches!(
            result,
            Err(NixHashError::InvalidNixBase32Digest { .. })
        ));
    }

    #[test]
    fn normalize_hash_text_to_nix32_rejects_wrong_nix32_length() {
        let result = normalize_hash_text_to_nix32("sha256:short");

        assert!(matches!(
            result,
            Err(NixHashError::InvalidNixBase32Digest { .. })
        ));
    }

    #[test]
    fn store_dir_normalizes_trailing_slash() {
        let store_dir = StoreDir::new("/nix/store/").unwrap();
        assert_eq!(store_dir.as_str(), "/nix/store");
    }

    #[test]
    fn store_dir_rejects_relative_path() {
        let result = StoreDir::new("nix/store");
        assert!(matches!(result, Err(StoreDirError::NotAbsolute(_))));
    }

    #[test]
    fn store_dir_contains_member_paths() {
        let store_dir = StoreDir::new("/nix/store").unwrap();

        assert!(store_dir.contains_path("/nix/store/abc"));
        assert!(!store_dir.contains_path("/nix/storeish/abc"));
        assert!(!store_dir.contains_path("/other/store/abc"));
    }

    #[test]
    fn store_dir_relative_path_strips_prefix() {
        let store_dir = StoreDir::new("/nix/store").unwrap();

        assert_eq!(
            store_dir.relative_path("/nix/store/abc-hello"),
            Some("abc-hello")
        );
        assert_eq!(store_dir.relative_path("/other/store/abc-hello"), None);
    }

    #[test]
    fn store_dir_display_path_falls_back_for_non_member_paths() {
        let store_dir = StoreDir::new("/nix/store").unwrap();

        assert_eq!(store_dir.display_path("/nix/store/abc-hello"), "abc-hello");
        assert_eq!(
            store_dir.display_path("/other/store/abc-hello"),
            "/other/store/abc-hello"
        );
    }
}
