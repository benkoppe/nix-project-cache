use std::fmt::Write as _;

use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use thiserror::Error;

use crate::narinfo::NarInfo;
use crate::nix::{NixHashError, StoreDir};

const EXPECTED_NAR_HASH_TEXT_LEN: usize = 59;

#[derive(Debug, Error)]
pub enum ParseSigningKeyError {
    #[error("sign key does not contain a ':'")]
    MissingSeparator,
    #[error("empty key name")]
    EmptyName,
    #[error("failed to decode base64: {0}")]
    InvalidBase64(String),
    #[error("invalid signing key length: expected 32 or 64 bytes, got {0}")]
    InvalidKeyLength(usize),
    #[error("invalid Ed25519 keypair bytes: {0}")]
    InvalidKeypair(String),
}

#[derive(Debug, Error)]
pub enum FingerprintError {
    #[error("store path is not inside configured store dir {store_dir}: {path}")]
    InvalidStorePath { store_dir: String, path: String },
    #[error("failed to normalize NAR hash: {0}")]
    InvalidNarHash(#[from] NixHashError),
    #[error("NAR hash must start with 'sha256:'")]
    InvalidNarHashPrefix,
    #[error("NAR hash has invalid length: expected {expected}, got {got}",
        expected = EXPECTED_NAR_HASH_TEXT_LEN,
        got = .0)]
    InvalidNarHashLength(usize),
    #[error("reference path is not inside configured store dir {store_dir}: {path}")]
    InvalidReferencePath { store_dir: String, path: String },
}

#[derive(Debug, Error)]
pub enum SignNarInfoError {
    #[error(transparent)]
    Fingerprint(#[from] FingerprintError),
}

#[derive(Debug, Clone)]
pub struct NamedSigningKey {
    name: String,
    key: SigningKey,
}

impl NamedSigningKey {
    pub fn parse(input: &str) -> Result<Self, ParseSigningKeyError> {
        let (name_part, key_part) = input
            .split_once(':')
            .ok_or(ParseSigningKeyError::MissingSeparator)?;

        let name = name_part.trim();
        let key_text = key_part.trim();

        if name.is_empty() {
            return Err(ParseSigningKeyError::EmptyName);
        }

        let key_bytes = decode_base64_standard_or_raw(key_text)?;

        let key = match key_bytes.len() {
            32 => {
                let mut seed = [0u8; 32];
                seed.copy_from_slice(&key_bytes);
                SigningKey::from_bytes(&seed)
            }
            64 => {
                let mut keypair = [0u8; 64];
                keypair.copy_from_slice(&key_bytes);
                SigningKey::from_keypair_bytes(&keypair)
                    .map_err(|error| ParseSigningKeyError::InvalidKeypair(error.to_string()))?
            }
            len => return Err(ParseSigningKeyError::InvalidKeyLength(len)),
        };

        Ok(Self {
            name: name.to_owned(),
            key,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn sign_message(&self, message: &[u8]) -> String {
        let signature = self.key.sign(message);
        let signature_text = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());

        format!("{}:{signature_text}", self.name)
    }

    pub fn public_key_text(&self) -> String {
        let public_key: VerifyingKey = self.key.verifying_key();
        let public_key_text =
            base64::engine::general_purpose::STANDARD.encode(public_key.as_bytes());

        format!("{}:{public_key_text}", self.name)
    }
}

fn decode_base64_standard_or_raw(input: &str) -> Result<Vec<u8>, ParseSigningKeyError> {
    match base64::engine::general_purpose::STANDARD.decode(input) {
        Ok(bytes) => Ok(bytes),
        Err(standard_error) => match base64::engine::general_purpose::STANDARD_NO_PAD.decode(input)
        {
            Ok(bytes) => Ok(bytes),
            Err(raw_error) => Err(ParseSigningKeyError::InvalidBase64(format!(
                "{standard_error}; raw no-pad decode also failed: {raw_error}"
            ))),
        },
    }
}

#[derive(Debug, Clone)]
pub struct NarInfoSigner {
    store_dir: StoreDir,
    keys: Vec<NamedSigningKey>,
}

impl NarInfoSigner {
    pub fn new(store_dir: StoreDir, keys: Vec<NamedSigningKey>) -> Self {
        Self { store_dir, keys }
    }

    pub fn store_dir(&self) -> &StoreDir {
        &self.store_dir
    }

    pub fn keys(&self) -> &[NamedSigningKey] {
        &self.keys
    }

    pub fn public_key_texts(&self) -> Vec<String> {
        self.keys
            .iter()
            .map(NamedSigningKey::public_key_text)
            .collect()
    }

    pub fn fingerprint(&self, narinfo: &NarInfo) -> Result<Vec<u8>, FingerprintError> {
        if !self.store_dir.contains_path(&narinfo.store_path) {
            return Err(FingerprintError::InvalidStorePath {
                store_dir: self.store_dir.to_string(),
                path: narinfo.store_path.clone(),
            });
        }

        let nar_hash = narinfo.nar_hash_nix32()?;
        if !nar_hash.starts_with("sha256:") {
            return Err(FingerprintError::InvalidNarHashPrefix);
        }
        if nar_hash.len() != EXPECTED_NAR_HASH_TEXT_LEN {
            return Err(FingerprintError::InvalidNarHashLength(nar_hash.len()));
        }

        for reference in &narinfo.references {
            if !self.store_dir.contains_path(reference) {
                return Err(FingerprintError::InvalidReferencePath {
                    store_dir: self.store_dir.to_string(),
                    path: reference.clone(),
                });
            }
        }

        let mut sorted_refs = narinfo.references.clone();
        sorted_refs.sort_unstable();

        let mut fingerprint = String::new();
        fingerprint.push_str("1;");
        fingerprint.push_str(&narinfo.store_path);
        fingerprint.push(';');
        fingerprint.push_str(&nar_hash);
        fingerprint.push(';');
        write!(&mut fingerprint, "{}", narinfo.nar_size).unwrap();
        fingerprint.push(';');

        for (index, reference) in sorted_refs.iter().enumerate() {
            if index > 0 {
                fingerprint.push(',');
            }
            fingerprint.push_str(reference);
        }

        Ok(fingerprint.into_bytes())
    }

    pub fn sign(&self, narinfo: &NarInfo) -> Result<Vec<String>, SignNarInfoError> {
        let fingerprint = self.fingerprint(narinfo)?;

        Ok(self
            .keys
            .iter()
            .map(|key| key.sign_message(&fingerprint))
            .collect())
    }
}
