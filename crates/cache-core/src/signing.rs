use std::fmt::Write as _;

use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use thiserror::Error;

use crate::narinfo::NarInfo;
use crate::nix::{NixHashError, StoreDir};

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
pub enum GenerateSigningKeyError {
    #[error("empty key name")]
    EmptyName,
    #[error("failed to generate signing key seed: {0}")]
    Random(String),
}

#[derive(Debug, Error)]
pub enum FingerprintError {
    #[error("store path is not inside configured store dir {store_dir}: {path}")]
    InvalidStorePath { store_dir: String, path: String },
    #[error("failed to normalize NAR hash: {0}")]
    InvalidNarHash(#[from] NixHashError),
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

    pub fn generate(name: &str) -> Result<Self, GenerateSigningKeyError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(GenerateSigningKeyError::EmptyName);
        }

        let mut seed = [0_u8; 32];
        getrandom::fill(&mut seed)
            .map_err(|error| GenerateSigningKeyError::Random(error.to_string()))?;

        Ok(Self {
            name: name.to_owned(),
            key: SigningKey::from_bytes(&seed),
        })
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Result<Self, ParseSigningKeyError> {
        let name = name.into();
        let name = name.trim();

        if name.is_empty() {
            return Err(ParseSigningKeyError::EmptyName);
        }

        self.name = name.to_owned();
        Ok(self)
    }

    pub fn private_key_text(&self) -> String {
        let seed_text = base64::engine::general_purpose::STANDARD.encode(self.key.to_bytes());
        format!("{}:{seed_text}", self.name)
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

        let nar_hash = narinfo.normalized_nar_hash()?;

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
        write!(&mut fingerprint, "{nar_hash}").unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::narinfo::NarInfo;
    use crate::nix::{
        ContentAddressMethod, HashAlgorithm, HashTextEncoding, NixContentAddress, NixHash, StoreDir,
    };

    fn sample_narinfo() -> NarInfo {
        NarInfo {
            store_path: "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1".to_owned(),
            url: "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst".to_owned(),
            compression: "zstd".to_owned(),
            nar_hash: NixHash::Raw(
                "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz".to_owned(),
            ),
            nar_size: 226560,
            references: vec![],
            deriver: None,
            signatures: vec![],
            ca: None,
        }
    }

    fn sample_key() -> NamedSigningKey {
        NamedSigningKey::parse("test-key:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap()
    }

    #[test]
    fn parse_signing_key_accepts_valid_32_byte_key() {
        let key = NamedSigningKey::parse("test-key:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
            .unwrap();
        assert_eq!(key.name(), "test-key");
    }

    #[test]
    fn parse_signing_key_accepts_another_valid_32_byte_key() {
        let key = NamedSigningKey::parse("test-key:zFD7RJEU40VJzJvgT7h5xQwFm8FufXKH2CJPaKvh/xo=")
            .unwrap();

        assert_eq!(key.name(), "test-key");
    }

    #[test]
    fn parse_signing_key_accepts_raw_base64_without_padding() {
        let key =
            NamedSigningKey::parse("test-key:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();

        assert_eq!(key.name(), "test-key");
    }

    #[test]
    fn parse_signing_key_accepts_valid_64_byte_keypair() {
        let seed = base64::engine::general_purpose::STANDARD
            .decode("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
            .unwrap();
        let mut seed_bytes = [0u8; 32];
        seed_bytes.copy_from_slice(&seed);

        let key = SigningKey::from_bytes(&seed_bytes);
        let keypair_text = base64::engine::general_purpose::STANDARD.encode(key.to_keypair_bytes());
        let parsed = NamedSigningKey::parse(&format!("test-key:{keypair_text}")).unwrap();
        assert_eq!(parsed.name(), "test-key");
    }

    #[test]
    fn parse_signing_key_trims_name_and_key_text() {
        let key =
            NamedSigningKey::parse("  test-key  :  AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=  ")
                .unwrap();
        assert_eq!(key.name(), "test-key");
    }

    #[test]
    fn parse_signing_key_rejects_missing_separator() {
        let result = NamedSigningKey::parse("no-colon");
        assert!(matches!(
            result,
            Err(ParseSigningKeyError::MissingSeparator)
        ));
    }

    #[test]
    fn parse_signing_key_rejects_empty_name() {
        let result = NamedSigningKey::parse(":AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        assert!(matches!(result, Err(ParseSigningKeyError::EmptyName)));
    }

    #[test]
    fn parse_signing_key_rejects_invalid_base64() {
        let result = NamedSigningKey::parse("name:invalid-base64!!!");
        assert!(matches!(
            result,
            Err(ParseSigningKeyError::InvalidBase64(_))
        ));
    }

    #[test]
    fn parse_signing_key_rejects_wrong_length() {
        let result = NamedSigningKey::parse("name:aGVsbG8=");
        assert!(matches!(
            result,
            Err(ParseSigningKeyError::InvalidKeyLength(5))
        ));
    }

    #[test]
    fn parse_signing_key_rejects_invalid_64_byte_keypair() {
        let invalid_keypair = base64::engine::general_purpose::STANDARD.encode([0u8; 64]);
        let result = NamedSigningKey::parse(&format!("name:{invalid_keypair}"));

        assert!(matches!(
            result,
            Err(ParseSigningKeyError::InvalidKeypair(_))
        ));
    }

    #[test]
    fn signing_key_name_returns_name() {
        let key = sample_key();
        assert_eq!(key.name(), "test-key");
    }

    #[test]
    fn sign_message_prefixes_key_name() {
        let key = sample_key();
        let signature = key.sign_message(b"Hello, world!");

        assert!(signature.starts_with("test-key:"));
    }

    #[test]
    fn sign_message_is_deterministic() {
        let key = sample_key();

        let a = key.sign_message(b"Hello, world!");
        let b = key.sign_message(b"Hello, world!");

        assert_eq!(a, b);
    }

    #[test]
    fn sign_message_changes_with_message() {
        let key = sample_key();

        let a = key.sign_message(b"one");
        let b = key.sign_message(b"two");

        assert_ne!(a, b);
    }

    #[test]
    fn sign_message_produces_base64_signature_bytes() {
        let key = sample_key();
        let signature = key.sign_message(b"Hello, world!");

        let (_, encoded) = signature.split_once(':').unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();

        assert_eq!(bytes.len(), 64);
    }

    #[test]
    fn public_key_text_prefixes_key_name() {
        let key = sample_key();
        let public_key = key.public_key_text();

        assert!(public_key.starts_with("test-key:"));
    }

    #[test]
    fn public_key_text_is_deterministic() {
        let key = sample_key();

        let a = key.public_key_text();
        let b = key.public_key_text();

        assert_eq!(a, b);
    }

    #[test]
    fn public_key_text_decodes_to_32_bytes() {
        let key = sample_key();
        let public_key = key.public_key_text();
        let (_, encoded) = public_key.split_once(':').unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn narinfo_signer_new_preserves_store_dir_and_keys() {
        let key = sample_key();
        let store_dir = StoreDir::new("/custom/store").unwrap();
        let signer = NarInfoSigner::new(store_dir.clone(), vec![key.clone()]);

        assert_eq!(signer.store_dir(), &store_dir);
        assert_eq!(signer.keys().len(), 1);
        assert_eq!(signer.keys()[0].name(), key.name());
    }

    #[test]
    fn public_key_texts_preserve_key_order() {
        let key1 = NamedSigningKey::parse(
            "cache.example.com-1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .unwrap();
        let key2 = NamedSigningKey::parse(
            "cache.example.com-2:zFD7RJEU40VJzJvgT7h5xQwFm8FufXKH2CJPaKvh/xo=",
        )
        .unwrap();

        let signer = NarInfoSigner::new(StoreDir::default(), vec![key1, key2]);
        let public_keys = signer.public_key_texts();

        assert_eq!(public_keys.len(), 2);
        assert!(public_keys[0].starts_with("cache.example.com-1:"));
        assert!(public_keys[1].starts_with("cache.example.com-2:"));
    }

    #[test]
    fn public_key_texts_returns_empty_vec_for_empty_keyset() {
        let signer = NarInfoSigner::new(StoreDir::default(), vec![]);
        assert!(signer.public_key_texts().is_empty());
    }

    #[test]
    fn narinfo_fingerprint_matches_expected_output_with_references() {
        let signer = NarInfoSigner::new(StoreDir::default(), vec![]);
        let narinfo = NarInfo {
            store_path: "/nix/store/syd87l2rxw8cbsxmxl853h0r6pdwhwjr-curl-7.82.0-bin".to_owned(),
            url: "nar/unused".to_owned(),
            compression: "zstd".to_owned(),
            nar_hash: NixHash::Raw(
                "sha256:1b4sb93wp679q4zx9k1ignby1yna3z7c4c2ri3wphylbc2dwsys0".to_owned(),
            ),
            nar_size: 196040,
            references: vec![
                "/nix/store/0jqd0rlxzra1rs38rdxl43yh6rxchgc6-curl-7.82.0".to_owned(),
                "/nix/store/5dq2jj6d7k197p6fzqn8l5n0jfmhxmcg-glibc-2.33-59".to_owned(),
            ],
            deriver: None,
            signatures: vec![],
            ca: None,
        };

        let fingerprint = signer.fingerprint(&narinfo).unwrap();

        assert_eq!(
            String::from_utf8(fingerprint).unwrap(),
            "1;/nix/store/syd87l2rxw8cbsxmxl853h0r6pdwhwjr-curl-7.82.0-bin;sha256:1b4sb93wp679q4zx9k1ignby1yna3z7c4c2ri3wphylbc2dwsys0;196040;/nix/store/0jqd0rlxzra1rs38rdxl43yh6rxchgc6-curl-7.82.0,/nix/store/5dq2jj6d7k197p6fzqn8l5n0jfmhxmcg-glibc-2.33-59"
        );
    }

    #[test]
    fn narinfo_fingerprint_matches_expected_output_without_references() {
        let signer = NarInfoSigner::new(StoreDir::default(), vec![]);
        let narinfo = sample_narinfo();

        let fingerprint = signer.fingerprint(&narinfo).unwrap();

        assert_eq!(
            String::from_utf8(fingerprint).unwrap(),
            "1;/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1;sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz;226560;"
        );
    }

    #[test]
    fn narinfo_fingerprint_sorts_references() {
        let signer = NarInfoSigner::new(StoreDir::default(), vec![]);
        let mut narinfo = sample_narinfo();
        narinfo.references = vec![
            "/nix/store/zzz-package".to_owned(),
            "/nix/store/aaa-package".to_owned(),
            "/nix/store/mmm-package".to_owned(),
        ];

        let fingerprint = signer.fingerprint(&narinfo).unwrap();

        assert_eq!(
            String::from_utf8(fingerprint).unwrap(),
            "1;/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1;sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz;226560;/nix/store/aaa-package,/nix/store/mmm-package,/nix/store/zzz-package"
        );
    }

    #[test]
    fn narinfo_fingerprint_rejects_invalid_store_path_for_configured_store_dir() {
        let signer = NarInfoSigner::new(StoreDir::default(), vec![]);
        let mut narinfo = sample_narinfo();
        narinfo.store_path = "/usr/local/test".to_owned();

        let result = signer.fingerprint(&narinfo);
        assert!(matches!(
            result,
            Err(FingerprintError::InvalidStorePath { .. })
        ));
    }

    #[test]
    fn narinfo_fingerprint_rejects_invalid_reference_path() {
        let signer = NarInfoSigner::new(StoreDir::default(), vec![]);
        let mut narinfo = sample_narinfo();
        narinfo.references = vec!["/usr/local/invalid".to_owned()];

        let result = signer.fingerprint(&narinfo);
        assert!(matches!(
            result,
            Err(FingerprintError::InvalidReferencePath { .. })
        ));
    }

    #[test]
    fn narinfo_fingerprint_supports_custom_store_dir() {
        let signer = NarInfoSigner::new(StoreDir::new("/custom/store").unwrap(), vec![]);
        let narinfo = NarInfo {
            store_path: "/custom/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello".to_owned(),
            url: "nar/unused".to_owned(),
            compression: "zstd".to_owned(),
            nar_hash: NixHash::Raw(
                "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz".to_owned(),
            ),
            nar_size: 1,
            references: vec!["/custom/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-dep".to_owned()],
            deriver: None,
            signatures: vec![],
            ca: None,
        };

        let fingerprint = signer.fingerprint(&narinfo).unwrap();

        assert_eq!(
            String::from_utf8(fingerprint).unwrap(),
            "1;/custom/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello;sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz;1;/custom/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-dep"
        );
    }

    #[test]
    fn narinfo_fingerprint_rejects_non_sha256_hash() {
        let signer = NarInfoSigner::new(StoreDir::default(), vec![]);
        let mut narinfo = sample_narinfo();
        narinfo.nar_hash = NixHash::Structured {
            algorithm: HashAlgorithm::Sha512,
            encoding: Some(HashTextEncoding::Base64),
            digest: "abcdef".to_owned(),
        };

        let result = signer.fingerprint(&narinfo);
        assert!(matches!(
            result,
            Err(FingerprintError::InvalidNarHash(
                NixHashError::UnsupportedAlgorithm(_)
            ))
        ));
    }

    #[test]
    fn narinfo_fingerprint_ignores_deriver_signatures_and_ca() {
        let signer = NarInfoSigner::new(StoreDir::default(), vec![]);
        let base = sample_narinfo();

        let mut enriched = sample_narinfo();
        enriched.deriver = Some("/nix/store/some-deriver.drv".to_owned());
        enriched.signatures = vec!["cache.example.com-1:abc".to_owned()];
        enriched.ca = Some(NixContentAddress::Structured {
            method: ContentAddressMethod::Nar,
            hash: NixHash::Raw("sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=".to_owned()),
        });

        let base_fp = signer.fingerprint(&base).unwrap();
        let enriched_fp = signer.fingerprint(&enriched).unwrap();

        assert_eq!(base_fp, enriched_fp);
    }

    #[test]
    fn narinfo_fingerprint_is_deterministic() {
        let signer = NarInfoSigner::new(StoreDir::default(), vec![]);
        let narinfo = sample_narinfo();

        let a = signer.fingerprint(&narinfo).unwrap();
        let b = signer.fingerprint(&narinfo).unwrap();

        assert_eq!(a, b);
    }

    #[test]
    fn sign_returns_one_signature_per_key_in_input_order() {
        let key1 = NamedSigningKey::parse(
            "cache.example.com-1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .unwrap();
        let key2 = NamedSigningKey::parse(
            "cache.example.com-2:zFD7RJEU40VJzJvgT7h5xQwFm8FufXKH2CJPaKvh/xo=",
        )
        .unwrap();

        let signer = NarInfoSigner::new(StoreDir::default(), vec![key1, key2]);

        let narinfo = NarInfo {
            store_path: "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1".to_owned(),
            url: "nar/unused".to_owned(),
            compression: "zstd".to_owned(),
            nar_hash: NixHash::Raw(
                "sha256:1mkvday29m2qxg1fnbv8xh9s6151bh8a2xzhh0k86j7lqhyfwibh".to_owned(),
            ),
            nar_size: 226560,
            references: vec![
                "/nix/store/sl141d1g77wvhr050ah87lcyz2czdxa3-glibc-2.40-36".to_owned(),
            ],
            deriver: None,
            signatures: vec![],
            ca: None,
        };

        let signatures = signer.sign(&narinfo).unwrap();

        assert_eq!(signatures.len(), 2);
        assert!(signatures[0].starts_with("cache.example.com-1:"));
        assert!(signatures[1].starts_with("cache.example.com-2:"));
    }

    #[test]
    fn sign_is_deterministic() {
        let key1 = NamedSigningKey::parse(
            "cache.example.com-1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .unwrap();
        let key2 = NamedSigningKey::parse(
            "cache.example.com-2:zFD7RJEU40VJzJvgT7h5xQwFm8FufXKH2CJPaKvh/xo=",
        )
        .unwrap();

        let signer = NarInfoSigner::new(StoreDir::default(), vec![key1, key2]);
        let narinfo = sample_narinfo();

        let a = signer.sign(&narinfo).unwrap();
        let b = signer.sign(&narinfo).unwrap();

        assert_eq!(a, b);
    }

    #[test]
    fn sign_returns_empty_vec_for_empty_keyset() {
        let signer = NarInfoSigner::new(StoreDir::default(), vec![]);
        let narinfo = sample_narinfo();

        let signatures = signer.sign(&narinfo).unwrap();
        assert!(signatures.is_empty());
    }

    #[test]
    fn sign_propagates_fingerprint_errors() {
        let signer = NarInfoSigner::new(StoreDir::default(), vec![sample_key()]);
        let mut narinfo = sample_narinfo();
        narinfo.store_path = "/usr/local/test".to_owned();

        let result = signer.sign(&narinfo);

        assert!(matches!(
            result,
            Err(SignNarInfoError::Fingerprint(
                FingerprintError::InvalidStorePath { .. }
            ))
        ));
    }

    #[test]
    fn sign_supports_custom_store_dir() {
        let signer =
            NarInfoSigner::new(StoreDir::new("/custom/store").unwrap(), vec![sample_key()]);

        let narinfo = NarInfo {
            store_path: "/custom/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello".to_owned(),
            url: "nar/unused".to_owned(),
            compression: "zstd".to_owned(),
            nar_hash: NixHash::Raw(
                "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz".to_owned(),
            ),
            nar_size: 1,
            references: vec![],
            deriver: None,
            signatures: vec![],
            ca: None,
        };

        let signatures = signer.sign(&narinfo).unwrap();

        assert_eq!(signatures.len(), 1);
        assert!(signatures[0].starts_with("test-key:"));
    }

    #[test]
    fn normalized_nar_hash_returns_normalized_sha256_text() {
        let narinfo = sample_narinfo();
        let nar_hash = narinfo.normalized_nar_hash().unwrap();

        assert_eq!(nar_hash.algorithm(), &HashAlgorithm::Sha256);
        assert_eq!(nar_hash.digest().len(), 52);
        assert_eq!(nar_hash.text_len(), 59);
        assert_eq!(
            nar_hash.to_string(),
            "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz"
        );
    }
}
